//! Windows session timer. The pure state contract mirrors sleep_timer.rs;
//! one worker owns the deadline, cancellation and the synchronous stop effect.
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
pub const EVENT: &str = "sleep-timer://changed";
const REPLACED: &str = "SLEEP_TIMER_REPLACED";
const UNAVAILABLE: &str = "SLEEP_TIMER_UNAVAILABLE";
pub type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Idle,
    Armed,
    Expired,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    revision: u64,
    phase: Phase,
    remaining_ms: u64,
}
struct Timer {
    revision: u64,
    generation: u64,
    phase: Phase,
    deadline: Option<Instant>,
}
impl Default for Timer {
    fn default() -> Self {
        Self {
            revision: 0,
            generation: 0,
            phase: Phase::Idle,
            deadline: None,
        }
    }
}
impl Timer {
    fn set(
        &mut self,
        minutes: Option<u32>,
        now: Instant,
    ) -> std::result::Result<Option<(u64, Duration)>, String> {
        if minutes.is_some_and(|m| ![15, 30, 60].contains(&m)) {
            return Err("SLEEP_TIMER_INVALID".into());
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("SLEEP_TIMER_UNAVAILABLE")?;
        self.revision = revision;
        self.generation = revision;
        self.phase = if minutes.is_some() {
            Phase::Armed
        } else {
            Phase::Idle
        };
        self.deadline = minutes.map(|m| now + Duration::from_secs(u64::from(m) * 60));
        Ok(self
            .deadline
            .map(|end| (self.generation, end.saturating_duration_since(now))))
    }
    fn due(&self, generation: u64, now: Instant) -> std::result::Result<(), String> {
        if self.generation == generation
            && self.phase == Phase::Armed
            && self.deadline.is_some_and(|end| now >= end)
        {
            Ok(())
        } else {
            Err(REPLACED.into())
        }
    }
    fn finish(&mut self, generation: u64, ok: bool) -> bool {
        if self.generation != generation || self.phase != Phase::Armed {
            return false;
        }
        self.phase = if ok { Phase::Expired } else { Phase::Failed };
        self.deadline = None;
        self.revision = self.revision.saturating_add(1);
        true
    }
    fn snapshot(&self, now: Instant) -> Snapshot {
        Snapshot {
            revision: self.revision,
            phase: self.phase,
            remaining_ms: self
                .deadline
                .map(|end| end.saturating_duration_since(now).as_millis() as u64)
                .unwrap_or(0),
        }
    }
}

#[derive(Clone, Copy)]
enum Request {
    Get,
    Set(Option<u32>),
    Shutdown,
}
struct Message {
    request: Request,
    reply: mpsc::Sender<Result<Snapshot>>,
}
#[derive(Clone)]
pub struct Controller {
    sender: mpsc::SyncSender<Message>,
    stopping: Arc<AtomicBool>,
}
impl Controller {
    pub fn new(
        effect: impl FnMut() -> Result<()> + Send + 'static,
        publish: impl FnMut(Snapshot) + Send + 'static,
    ) -> Result<Self> {
        Self::with_clock(effect, publish, Instant::now)
    }
    fn with_clock(
        mut effect: impl FnMut() -> Result<()> + Send + 'static,
        mut publish: impl FnMut(Snapshot) + Send + 'static,
        now: impl Fn() -> Instant + Send + 'static,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Message>(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = stopping.clone();
        std::thread::Builder::new()
            .name("sonante-windows-sleep".into())
            .spawn(move || {
                let mut timer = Timer::default();
                loop {
                    if !worker_stopping.load(Ordering::Acquire)
                        && timer.due(timer.generation, now()).is_ok()
                    {
                        // No timer lock spans I/O. This worker is the cancellation
                        // lease: no setter can ACK while an admitted STOP is in flight.
                        // The effect admits its playback intent BEFORE queuing MPD work.
                        let result = effect();
                        if let Err(code) = result {
                            eprintln!("Windows sleep timer: {code}");
                        }
                        if timer.finish(timer.generation, result.is_ok()) {
                            publish(timer.snapshot(now()));
                        }
                    }
                    let message = if let Some(end) = timer
                        .deadline
                        .filter(|_| !worker_stopping.load(Ordering::Acquire))
                    {
                        match receiver.recv_timeout(end.saturating_duration_since(now())) {
                            Ok(message) => message,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match receiver.recv() {
                            Ok(message) => message,
                            Err(_) => break,
                        }
                    };
                    let shutdown = matches!(message.request, Request::Shutdown);
                    let result = if shutdown {
                        timer
                            .set(None, now())
                            .map_err(|_| UNAVAILABLE)
                            .map(|_| timer.snapshot(now()))
                    } else if worker_stopping.load(Ordering::Acquire) {
                        Err(UNAVAILABLE)
                    } else {
                        match message.request {
                            Request::Get => Ok(timer.snapshot(now())),
                            Request::Set(minutes) => timer
                                .set(minutes, now())
                                .map_err(|error| {
                                    if error == "SLEEP_TIMER_INVALID" {
                                        "SLEEP_TIMER_INVALID"
                                    } else {
                                        UNAVAILABLE
                                    }
                                })
                                .map(|_| timer.snapshot(now())),
                            Request::Shutdown => unreachable!(),
                        }
                    };
                    if !matches!(message.request, Request::Get) {
                        if let Ok(value) = &result {
                            publish(value.clone());
                        }
                    }
                    if message.reply.send(result).is_err() {
                        eprintln!("Windows sleep timer: receiver_closed");
                    }
                    if shutdown {
                        break;
                    }
                }
            })
            .map_err(|_| UNAVAILABLE)?;
        Ok(Self { sender, stopping })
    }
    fn enqueue(&self, request: Request) -> Result<mpsc::Receiver<Result<Snapshot>>> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message { request, reply })
            .map_err(|_| UNAVAILABLE)?;
        Ok(receiver)
    }
    fn call(&self, request: Request) -> Result<Snapshot> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(UNAVAILABLE);
        }
        self.enqueue(request)?.recv().map_err(|_| UNAVAILABLE)?
    }
    pub fn get(&self) -> Result<Snapshot> {
        self.call(Request::Get)
    }
    pub fn set(&self, minutes: Option<u32>) -> Result<Snapshot> {
        self.call(Request::Set(minutes))
    }
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        self.enqueue(Request::Shutdown)?
            .recv_timeout(Duration::from_secs(6))
            .map_err(|_| UNAVAILABLE)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    fn next(events: &mpsc::Receiver<Snapshot>, phase: Phase) -> Snapshot {
        loop {
            let value = events.recv_timeout(Duration::from_secs(2)).unwrap();
            if value.phase == phase {
                return value;
            }
        }
    }
    fn clock() -> (Arc<AtomicU64>, impl Fn() -> Instant + Send + 'static) {
        let elapsed = Arc::new(AtomicU64::new(0));
        let value = elapsed.clone();
        let start = Instant::now();
        (elapsed, move || {
            start + Duration::from_millis(value.load(Ordering::Acquire))
        })
    }
    #[test]
    fn original_deadline_validation_cancel_replace_and_finish_contract() {
        let now = Instant::now();
        let mut timer = Timer::default();
        for minutes in [0, 1, 14, 16, 61, u32::MAX] {
            assert!(timer.set(Some(minutes), now).is_err());
        }
        assert_eq!(timer.revision, 0);
        for minutes in [15, 30, 60] {
            let (generation, delay) = timer.set(Some(minutes), now).unwrap().unwrap();
            assert_eq!(delay.as_secs(), u64::from(minutes) * 60);
            assert!(timer
                .due(generation, now + delay - Duration::from_millis(1))
                .is_err());
            assert!(timer.due(generation, now + delay).is_ok());
            timer.set(None, now).unwrap();
            assert!(timer.due(generation, now + delay).is_err());
            assert!(!timer.finish(generation, true));
            assert_eq!(timer.phase, Phase::Idle);
        }
        let (old, _) = timer.set(Some(15), now).unwrap().unwrap();
        let (current, _) = timer.set(Some(60), now).unwrap().unwrap();
        assert!(timer.due(old, now + Duration::from_secs(3600)).is_err());
        assert!(!timer.finish(old, true));
        assert!(timer.finish(current, false));
        assert!(!timer.finish(current, true));
        assert_eq!(timer.phase, Phase::Failed);
        assert_eq!(timer.snapshot(now).remaining_ms, 0);
        assert_eq!(Timer::default().phase, Phase::Idle);
    }
    #[test]
    fn worker_has_one_expiry_publishes_native_revision_and_does_not_retry() {
        for success in [true, false] {
            let (time, now) = clock();
            let count = Arc::new(AtomicU64::new(0));
            let calls = count.clone();
            let (tx, rx) = mpsc::channel();
            let timer = Controller::with_clock(
                move || {
                    calls.fetch_add(1, Ordering::AcqRel);
                    if success {
                        Ok(())
                    } else {
                        Err("mpd_ack")
                    }
                },
                move |s| {
                    tx.send(s).unwrap();
                },
                now,
            )
            .unwrap();
            let armed = timer.set(Some(15)).unwrap();
            assert_eq!(armed.remaining_ms, 900_000);
            time.store(899_999, Ordering::Release);
            assert_eq!(timer.get().unwrap().phase, Phase::Armed);
            assert_eq!(count.load(Ordering::Acquire), 0);
            time.store(900_000, Ordering::Release);
            let _ = timer.get();
            let done = next(
                &rx,
                if success {
                    Phase::Expired
                } else {
                    Phase::Failed
                },
            );
            assert_eq!(done.revision, armed.revision + 1);
            assert_eq!(done.remaining_ms, 0);
            time.store(3_600_000, Ordering::Release);
            assert_eq!(timer.get().unwrap().revision, done.revision);
            assert_eq!(count.load(Ordering::Acquire), 1);
            timer.shutdown().unwrap();
        }
    }
    #[test]
    fn acknowledged_cancellation_and_replacement_fence_old_deadlines() {
        let (time, now) = clock();
        let count = Arc::new(AtomicU64::new(0));
        let calls = count.clone();
        let (tx, rx) = mpsc::channel();
        let timer = Controller::with_clock(
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
                Ok(())
            },
            move |s| {
                tx.send(s).unwrap();
            },
            now,
        )
        .unwrap();
        timer.set(Some(15)).unwrap();
        timer.set(None).unwrap();
        time.store(900_000, Ordering::Release);
        assert_eq!(timer.get().unwrap().phase, Phase::Idle);
        assert_eq!(count.load(Ordering::Acquire), 0);
        timer.set(Some(15)).unwrap();
        timer.set(Some(60)).unwrap();
        time.store(1_800_000, Ordering::Release);
        assert_eq!(timer.get().unwrap().phase, Phase::Armed);
        assert_eq!(count.load(Ordering::Acquire), 0);
        time.store(4_500_000, Ordering::Release);
        let _ = timer.get();
        next(&rx, Phase::Expired);
        assert_eq!(count.load(Ordering::Acquire), 1);
        timer.shutdown().unwrap();
    }
    #[test]
    fn cancel_ack_waits_for_inflight_stop_and_no_late_effect_follows() {
        let (time, now) = clock();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let count = Arc::new(AtomicU64::new(0));
        let calls = count.clone();
        let timer = Controller::with_clock(
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            },
            |_| {},
            now,
        )
        .unwrap();
        timer.set(Some(15)).unwrap();
        time.store(900_000, Ordering::Release);
        let _ = timer.get();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // Queue the exact setter message, proving it reached the worker boundary.
        let cancel = timer.enqueue(Request::Set(None)).unwrap();
        assert!(matches!(cancel.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        let cancelled = cancel
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.phase, Phase::Idle);
        time.store(3_600_000, Ordering::Release);
        assert_eq!(timer.get().unwrap().phase, Phase::Idle);
        assert_eq!(count.load(Ordering::Acquire), 1);
        timer.shutdown().unwrap();
    }
    #[test]
    fn shutdown_fences_expiry_and_new_arms_without_starting_effect() {
        let (time, now) = clock();
        let timer = Controller::with_clock(|| panic!("late stop"), |_| {}, now).unwrap();
        timer.set(Some(15)).unwrap();
        timer.begin_shutdown();
        time.store(900_000, Ordering::Release);
        assert!(timer.set(Some(30)).is_err());
        assert!(timer.get().is_err());
        timer.shutdown().unwrap();
    }
    #[test]
    fn invalid_set_preserves_armed_deadline_and_worker_reset_is_idle() {
        let (time, now) = clock();
        let timer = Controller::with_clock(|| Ok(()), |_| {}, now).unwrap();
        let before = timer.set(Some(30)).unwrap();
        time.store(1000, Ordering::Release);
        assert_eq!(timer.set(Some(1)).err(), Some("SLEEP_TIMER_INVALID"));
        let after = timer.get().unwrap();
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.remaining_ms, 1_799_000);
        timer.shutdown().unwrap();
        let fresh = Controller::new(|| panic!("startup effect"), |_| {}).unwrap();
        assert_eq!(fresh.get().unwrap().phase, Phase::Idle);
        fresh.shutdown().unwrap();
    }
}
