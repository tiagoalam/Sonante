//! Accepted network files must not run a potentially slow CloseHandle on the
//! playback/control actor. Reserve their bounded lifetime BEFORE opening them.
use super::Error;
use std::{
    collections::VecDeque,
    fs::File,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    },
};
struct Task<T: Send + 'static> {
    value: T,
    reservation: Reservation<T>,
}
struct State<T: Send + 'static> {
    queue: Mutex<VecDeque<Task<T>>>,
    changed: Condvar,
    stopping: AtomicBool,
    active: AtomicUsize,
    limit: usize,
}
struct Pool<T: Send + 'static> {
    state: Arc<State<T>>,
}
pub(crate) struct Reservation<T: Send + 'static> {
    pool: Arc<Pool<T>>,
}
impl<T: Send + 'static> Drop for Reservation<T> {
    fn drop(&mut self) {
        self.pool.state.active.fetch_sub(1, Ordering::AcqRel);
    }
}
impl<T: Send + 'static> Pool<T> {
    fn new(limit: usize, workers: usize) -> Result<Arc<Self>, Error> {
        if limit == 0 || workers == 0 {
            return Err(Error::WorkerFailed);
        }
        let state = Arc::new(State {
            queue: Mutex::new(VecDeque::with_capacity(limit)),
            changed: Condvar::new(),
            stopping: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            limit,
        });
        let pool = Arc::new(Self {
            state: state.clone(),
        });
        for _ in 0..workers {
            let state = state.clone();
            // No reservation/file can exist before all workers were created.
            // Failure drops Pool, waking any already-created empty workers.
            std::thread::Builder::new()
                .name("sonante-network-close".into())
                .spawn(move || {
                    loop {
                        let task = {
                            let mut queue = state.queue.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                if let Some(task) = queue.pop_front() {
                                    break Some(task);
                                }
                                if state.stopping.load(Ordering::Acquire) {
                                    break None;
                                }
                                queue =
                                    state.changed.wait(queue).unwrap_or_else(|e| e.into_inner());
                            }
                        };
                        let Some(Task { value, reservation }) = task else {
                            break;
                        };
                        // The actual file destructor performs no Rust callback.
                        // Catching unwinds also keeps a fixture's failing Drop from
                        // taking capacity or one of the fixed workers with it.
                        let _ =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(value)));
                        drop(reservation);
                    }
                })
                .map_err(|_| Error::WorkerFailed)?;
        }
        Ok(pool)
    }
    fn reserve(self: &Arc<Self>) -> Result<Reservation<T>, Error> {
        self.state
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < self.state.limit).then_some(n + 1)
            })
            .map_err(|_| Error::Busy)?;
        Ok(Reservation { pool: self.clone() })
    }
}
impl<T: Send + 'static> Drop for Pool<T> {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::Release);
        self.state.changed.notify_all();
    }
}
impl<T: Send + 'static> Reservation<T> {
    fn retire(self, value: T) {
        let pool = self.pool.clone();
        let mut queue = pool.state.queue.lock().unwrap_or_else(|e| e.into_inner());
        // Every item owns one UNIQUE reservation from before open. Including
        // this item, queued+open+in-close <= limit; the preallocated queue can
        // never fill beyond capacity. No allocation, send/disconnect fallback,
        // OS close or worker join happens while holding this tiny gate.
        queue.push_back(Task {
            value,
            reservation: self,
        });
        drop(queue);
        pool.state.changed.notify_one();
    }
}
pub(crate) struct Owned<T: Send + 'static> {
    value: Option<T>,
    reservation: Option<Reservation<T>>,
}
impl<T: Send + 'static> Owned<T> {
    pub(crate) fn deferred(&self)->bool{self.reservation.is_some()}
    pub(crate) fn new(value: T, reservation: Option<Reservation<T>>) -> Self {
        Self {
            value: Some(value),
            reservation,
        }
    }
}
impl<T: Send + 'static> Deref for Owned<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value
            .as_ref()
            .expect("owned file remains present before Drop")
    }
}
impl<T: Send + 'static> Drop for Owned<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            if let Some(reservation) = self.reservation.take() {
                reservation.retire(value);
            } else {
                drop(value);
            }
        }
    }
}
pub(crate) fn reserve_file() -> Result<Reservation<File>, Error> {
    static POOL: OnceLock<Result<Arc<Pool<File>>, Error>> = OnceLock::new();
    match POOL.get_or_init(|| Pool::new(4096, 8)) {
        Ok(pool) => pool.reserve(),
        Err(error) => Err(*error),
    }
}
/// Only newly created private preview storage. A/B reserves before creation;
/// the whole immutable owner (file then empty directory) retires in order.
/// Separate tiny bounded pool leaves all existing SMB file capacity unchanged.
pub(crate) fn reserve_private_preview()->Result<Reservation<Box<dyn Send+Sync>>,Error>{
    static POOL:OnceLock<Result<Arc<Pool<Box<dyn Send+Sync>>>,Error>>=OnceLock::new();
    match POOL.get_or_init(||Pool::new(4,1)){Ok(pool)=>pool.reserve(),Err(error)=>Err(*error)}
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    fn idle<T: Send + 'static>(pool: &Pool<T>) {
        let until = Instant::now() + Duration::from_secs(2);
        while pool.state.active.load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    struct Blocking {
        entered: mpsc::Sender<std::thread::ThreadId>,
        released: mpsc::Receiver<()>,
    }
    impl Drop for Blocking {
        fn drop(&mut self) {
            self.entered.send(std::thread::current().id()).unwrap();
            self.released.recv().unwrap();
        }
    }
    #[test]
    fn a_slow_close_runs_off_caller_and_retains_capacity_until_it_returns() {
        let p = Pool::new(1, 1).unwrap();
        let ticket = p.reserve().unwrap();
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        drop(Owned::new(
            Blocking {
                entered: tx,
                released: wait,
            },
            Some(ticket),
        ));
        assert_ne!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            std::thread::current().id()
        );
        assert!(matches!(p.reserve(), Err(Error::Busy)));
        release.send(()).unwrap();
        idle(&p);
        assert!(p.reserve().is_ok());
    }
    #[test]
    fn queued_closes_also_keep_their_reservations_and_do_not_fall_back_to_caller() {
        let p = Pool::new(2, 1).unwrap();
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        drop(Owned::new(
            Blocking {
                entered: tx,
                released: wait,
            },
            Some(p.reserve().unwrap()),
        ));
        rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (tx, rx) = mpsc::channel();
        let (next, wait) = mpsc::channel();
        drop(Owned::new(
            Blocking {
                entered: tx,
                released: wait,
            },
            Some(p.reserve().unwrap()),
        ));
        assert!(matches!(p.reserve(), Err(Error::Busy)));
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release.send(()).unwrap();
        assert_ne!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            std::thread::current().id()
        );
        next.send(()).unwrap();
        idle(&p);
    }
    #[test]
    fn abandoned_preopen_reservation_is_reusable_and_local_drop_stays_synchronous() {
        let p = Pool::<()>::new(1, 1).unwrap();
        drop(p.reserve().unwrap());
        assert!(p.reserve().is_ok());
        struct Where(mpsc::Sender<std::thread::ThreadId>);
        impl Drop for Where {
            fn drop(&mut self) {
                self.0.send(std::thread::current().id()).unwrap();
            }
        }
        let (tx, rx) = mpsc::channel();
        drop(Owned::new(Where(tx), None));
        assert_eq!(rx.recv().unwrap(), std::thread::current().id());
    }
    #[test]
    fn an_accepted_file_keeps_its_close_workers_alive_after_external_pool_release() {
        struct Where(mpsc::Sender<std::thread::ThreadId>);
        impl Drop for Where {
            fn drop(&mut self) {
                self.0.send(std::thread::current().id()).unwrap();
            }
        }
        let pool = Pool::new(1, 1).unwrap();
        let weak = Arc::downgrade(&pool);
        let (tx, rx) = mpsc::channel();
        let file = Owned::new(Where(tx), Some(pool.reserve().unwrap()));
        drop(pool);
        assert!(weak.upgrade().is_some());
        drop(file);
        assert_ne!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            std::thread::current().id()
        );
        let until = Instant::now() + Duration::from_secs(1);
        while weak.upgrade().is_some() {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }
    #[test]
    fn a_panicking_drop_does_not_remove_a_worker_or_release_capacity_early() {
        struct Fault;
        impl Drop for Fault {
            fn drop(&mut self) {
                panic!("own synthetic close fault");
            }
        }
        let p = Pool::new(1, 1).unwrap();
        drop(Owned::new(Fault, Some(p.reserve().unwrap())));
        idle(&p);
        drop(Owned::new(Fault, Some(p.reserve().unwrap())));
        idle(&p);
    }
}
