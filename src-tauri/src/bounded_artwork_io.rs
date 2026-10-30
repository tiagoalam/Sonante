//! Admission bounds real filesystem workers, not merely the time an IPC waits.
//! A blocked NAS read retains its lease until it actually exits.
use std::sync::atomic::{AtomicUsize, Ordering};
pub(crate) struct Reads {
    active: AtomicUsize,
    limit: usize,
}
pub(crate) static COVER_READS: Reads = Reads {
    active: AtomicUsize::new(0),
    limit: 2,
};
pub(crate) struct Permit<'a>(&'a Reads);
impl Reads {
    pub(crate) const fn new(limit: usize) -> Self {
        Self { active: AtomicUsize::new(0), limit }
    }

    pub(crate) fn try_acquire(&self) -> Option<Permit<'_>> {
        let mut count = self.active.load(Ordering::SeqCst);
        loop {
            if count >= self.limit { return None; }
            match self.active.compare_exchange_weak(
                count, count + 1, Ordering::SeqCst, Ordering::SeqCst,
            ) {
                Ok(_) => return Some(Permit(self)),
                Err(observed) => count = observed,
            }
        }
    }

    pub(crate) fn acquire(&self) -> Result<Permit<'_>, String> {
        self.try_acquire().ok_or_else(|| "local_artwork_busy".into())
    }
}
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_io_is_bounded_and_failure_or_completion_restores_capacity() {
        let reads = std::sync::Arc::new(Reads {
            active: AtomicUsize::new(0),
            limit: 2,
        });
        let (entered, start) = std::sync::mpsc::channel();
        let (resume, wait) = std::sync::mpsc::channel();
        let worker_reads = reads.clone();
        let worker = std::thread::spawn(move || {
            let _permit = worker_reads.acquire().unwrap();
            entered.send(()).unwrap();
            wait.recv().unwrap();
            Err::<(), _>("controlled failure")
        });
        start.recv().unwrap();
        let second = reads.acquire().unwrap();
        for _ in 0..100 {
            assert!(reads.acquire().is_err());
        }
        assert_eq!(reads.active.load(Ordering::SeqCst), 2);
        drop(second);
        let another = reads.acquire().unwrap();
        assert!(reads.acquire().is_err());
        drop(another);
        resume.send(()).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert_eq!(reads.active.load(Ordering::SeqCst), 0);
        assert!(reads.acquire().is_ok());
    }
}
