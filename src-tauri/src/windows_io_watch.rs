//! Idle-I/O supervision without joining a possibly blocked OS reader.
//! The actual read worker independently retains its physical admission permit.
use super::Error;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
enum Phase {
    Running(Instant),
    Suspended,
    Done,
    Expired,
}
struct Progress {
    phase: Mutex<Phase>,
    changed: Condvar,
    alive: Arc<AtomicBool>,
}
struct Capacity(AtomicUsize, usize);
struct Permit(Arc<Capacity>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(crate) struct Watch {
    progress: Arc<Progress>,
    timeout: Duration,
}
impl Watch {
    pub(crate) fn start(
        timeout: Duration,
        expire: impl FnOnce() + Send + 'static,
    ) -> Result<Self, Error> {
        static CAPACITY: OnceLock<Arc<Capacity>> = OnceLock::new();
        Self::start_with(
            CAPACITY
                .get_or_init(|| Arc::new(Capacity(AtomicUsize::new(0), 16)))
                .clone(),
            timeout,
            expire,
        )
    }
    fn start_with(
        capacity: Arc<Capacity>,
        timeout: Duration,
        expire: impl FnOnce() + Send + 'static,
    ) -> Result<Self, Error> {
        if timeout.is_zero() {
            return Err(Error::Timeout);
        }
        let deadline = Instant::now().checked_add(timeout).ok_or(Error::Timeout)?;
        capacity
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < capacity.1).then_some(n + 1)
            })
            .map_err(|_| Error::Busy)?;
        let permit = Permit(capacity);
        let progress = Arc::new(Progress {
            phase: Mutex::new(Phase::Running(deadline)),
            changed: Condvar::new(),
            alive: Arc::new(AtomicBool::new(true)),
        });
        let state = progress.clone();
        std::thread::Builder::new()
            .name("sonante-io-watch".into())
            .spawn(move || {
                let _permit = permit;
                let expired = loop {
                    let Ok(mut phase) = state.phase.lock() else {
                        break true;
                    };
                    match *phase {
                        Phase::Done => break false,
                        Phase::Expired => break true,
                        Phase::Suspended => {
                            if state.changed.wait(phase).is_err() {
                                break true;
                            }
                        }
                        Phase::Running(deadline) => {
                            let remaining = deadline.saturating_duration_since(Instant::now());
                            if remaining.is_zero() {
                                *phase = Phase::Expired;
                                break true;
                            }
                            if state.changed.wait_timeout(phase, remaining).is_err() {
                                break true;
                            }
                        }
                    }
                };
                state.alive.store(false, Ordering::Release);
                if expired {
                    expire();
                } else {
                    drop(expire);
                }
                // Also release the unused callback's socket/captures on normal
                // completion BEFORE returning physical watcher capacity.
                drop(state);
            })
            .map_err(|_| Error::WorkerFailed)?;
        Ok(Self { progress, timeout })
    }
    pub(crate) fn pulse(&self) -> bool {
        self.pulse_at(Instant::now())
    }
    fn pulse_at(&self, now: Instant) -> bool {
        let Ok(mut phase) = self.progress.phase.lock() else {
            self.progress.alive.store(false, Ordering::Release);
            self.progress.changed.notify_all();
            return false;
        };
        let good = match *phase {
            Phase::Suspended => {
                if let Some(next) = now.checked_add(self.timeout) {
                    *phase = Phase::Running(next);
                    true
                } else {
                    *phase = Phase::Expired;
                    false
                }
            }
            Phase::Running(deadline) if now < deadline => {
                if let Some(next) = now.checked_add(self.timeout) {
                    *phase = Phase::Running(next);
                    true
                } else {
                    *phase = Phase::Expired;
                    false
                }
            }
            Phase::Running(_) => {
                *phase = Phase::Expired;
                false
            }
            _ => false,
        };
        if !good {
            self.progress.alive.store(false, Ordering::Release);
        }
        self.progress.changed.notify_all();
        good
    }
    pub(crate) fn park(&self) -> bool {
        self.park_at(Instant::now())
    }
    fn park_at(&self, now: Instant) -> bool {
        let Ok(mut phase) = self.progress.phase.lock() else {
            self.progress.alive.store(false, Ordering::Release);
            self.progress.changed.notify_all();
            return false;
        };
        let good = match *phase {
            Phase::Running(deadline) if now < deadline => {
                *phase = Phase::Suspended;
                true
            }
            Phase::Suspended => true,
            Phase::Running(_) => {
                *phase = Phase::Expired;
                false
            }
            _ => false,
        };
        if !good {
            self.progress.alive.store(false, Ordering::Release);
        }
        self.progress.changed.notify_all();
        good
    }
    pub(crate) fn current(&self) -> bool {
        let Ok(mut phase) = self.progress.phase.lock() else {
            self.progress.alive.store(false, Ordering::Release);
            self.progress.changed.notify_all();
            return false;
        };
        let good = match *phase {
            Phase::Suspended => true,
            Phase::Running(deadline) if Instant::now() < deadline => true,
            Phase::Running(_) => {
                *phase = Phase::Expired;
                false
            }
            _ => false,
        };
        if !good {
            self.progress.alive.store(false, Ordering::Release);
            self.progress.changed.notify_all();
        }
        good
    }
    pub(crate) fn signal(&self) -> Arc<AtomicBool> {
        self.progress.alive.clone()
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        if let Ok(mut phase) = self.progress.phase.lock() {
            if matches!(*phase, Phase::Running(_) | Phase::Suspended) {
                *phase = Phase::Done;
            }
        }
        self.progress.alive.store(false, Ordering::Release);
        self.progress.changed.notify_all();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    fn capacity() -> Arc<Capacity> {
        Arc::new(Capacity(AtomicUsize::new(0), 1))
    }
    fn idle(c: &Capacity) {
        let end = Instant::now() + Duration::from_secs(1);
        while c.0.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
    }
    #[test]
    fn stalled_io_is_flagged_without_waiting_for_its_reader() {
        let c = capacity();
        let (tx, rx) = mpsc::channel();
        let w = Watch::start_with(c.clone(), Duration::from_millis(20), move || {
            tx.send(()).unwrap();
        })
        .unwrap();
        let alive = w.signal();
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!alive.load(Ordering::Acquire));
        assert!(!w.pulse());
        drop(w);
        idle(&c);
    }
    #[test]
    fn completion_does_not_report_a_timeout() {
        let c = capacity();
        let (tx, rx) = mpsc::channel();
        let w = Watch::start_with(c.clone(), Duration::from_secs(1), move || {
            tx.send(()).unwrap();
        })
        .unwrap();
        drop(w);
        idle(&c);
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }
    #[test]
    fn late_progress_cannot_revive_an_expired_operation() {
        let c = capacity();
        let (tx, rx) = mpsc::channel();
        let w = Watch::start_with(c.clone(), Duration::from_secs(1), move || {
            tx.send(()).unwrap();
        })
        .unwrap();
        assert!(!w.pulse_at(Instant::now() + Duration::from_secs(2)));
        assert!(!w.pulse());
        drop(w);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        idle(&c);
    }
    #[test]
    fn a_busy_timeout_callback_retains_its_physical_watcher_permit() {
        let c = capacity();
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let w = Watch::start_with(c.clone(), Duration::from_millis(20), move || {
            tx.send(()).unwrap();
            wait.recv().unwrap();
        })
        .unwrap();
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(w);
        assert!(matches!(
            Watch::start_with(c.clone(), Duration::from_secs(1), || {}),
            Err(Error::Busy)
        ));
        release.send(()).unwrap();
        idle(&c);
    }
    #[test]
    fn consumer_backpressure_is_not_a_filesystem_timeout() {
        let c = capacity();
        let (tx, rx) = mpsc::channel();
        let w = Watch::start_with(c.clone(), Duration::from_millis(20), move || {
            tx.send(()).unwrap();
        })
        .unwrap();
        assert!(w.park());
        std::thread::sleep(Duration::from_millis(60));
        assert!(w.current());
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(w.pulse());
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        drop(w);
        idle(&c);
    }
    #[test]
    fn returning_from_a_slow_read_cannot_hide_expiry_by_parking() {
        let c = capacity();
        let (tx, rx) = mpsc::channel();
        let w = Watch::start_with(c.clone(), Duration::from_secs(1), move || {
            tx.send(()).unwrap();
        })
        .unwrap();
        assert!(!w.park_at(Instant::now() + Duration::from_secs(2)));
        assert!(!w.current());
        drop(w);
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        idle(&c);
    }
}
