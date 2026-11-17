//! A logical timeout never releases a still-running physical filesystem scan.
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
struct Capacity(AtomicUsize, usize);
pub(super) struct Admission(Arc<Capacity>);
impl Admission {
    pub(super) fn acquire() -> super::Result<Self> {
        static CAPACITY: OnceLock<Arc<Capacity>> = OnceLock::new();
        Self::from(
            CAPACITY
                .get_or_init(|| Arc::new(Capacity(AtomicUsize::new(0), 4)))
                .clone(),
        )
    }
    fn from(capacity: Arc<Capacity>) -> super::Result<Self> {
        capacity
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < capacity.1).then_some(n + 1)
            })
            .map_err(|_| "engine_busy")?;
        Ok(Self(capacity))
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.0 .0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub(super) struct Scope(crate::windows_io::watch::Watch);
impl Scope {
    pub(super) fn start(
        failed: impl FnOnce() + Send + 'static,
    ) -> Result<Self, crate::windows_io::Error> {
        Self::with_timeout(Duration::from_secs(10), failed)
    }
    fn with_timeout(
        timeout: Duration,
        failed: impl FnOnce() + Send + 'static,
    ) -> Result<Self, crate::windows_io::Error> {
        crate::windows_io::watch::Watch::start(timeout, failed).map(Self)
    }
    // Check BOTH sides of every filesystem operation. A late return cannot
    // reset an expired deadline even if the timeout callback has not run yet.
    pub(super) fn checkpoint(&self) -> bool {
        self.0.pulse()
    }
    pub(super) fn current(&self) -> bool {
        self.0.current()
    }
    pub(super) fn finish(&self) -> bool {
        self.0.park()
    }
}
pub(super) struct Reader<'a, R> {
    inner: R,
    current: &'a dyn Fn() -> bool,
}
impl<'a, R> Reader<'a, R> {
    pub(super) fn new(inner: R, current: &'a dyn Fn() -> bool) -> Self {
        Self { inner, current }
    }
}
impl<R: Read> Read for Reader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if !(self.current)() {
            return Err(io::Error::other("index_retired"));
        }
        let n = self.inner.read(bytes)?;
        if !(self.current)() {
            return Err(io::Error::other("index_retired"));
        }
        Ok(n)
    }
}
impl<R: Seek> Seek for Reader<'_, R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        if !(self.current)() {
            return Err(io::Error::other("index_retired"));
        }
        let n = self.inner.seek(to)?;
        if !(self.current)() {
            return Err(io::Error::other("index_retired"));
        }
        Ok(n)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    #[test]
    fn a_retired_physical_scan_keeps_its_slot_until_it_actually_returns() {
        let c = Arc::new(Capacity(AtomicUsize::new(0), 1));
        let permit = Admission::from(c.clone()).unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _permit = permit;
            rx.recv().unwrap();
        });
        assert!(matches!(Admission::from(c.clone()), Err("engine_busy")));
        tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(Admission::from(c).is_ok());
    }
    #[test]
    fn a_reader_checks_retirement_before_and_after_the_actual_io() {
        use std::cell::Cell;
        let live = Cell::new(true);
        let calls = Cell::new(0);
        struct Late<'a>(&'a Cell<bool>, &'a Cell<usize>);
        impl Read for Late<'_> {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.1.set(self.1.get() + 1);
                b[0] = 1;
                self.0.set(false);
                Ok(1)
            }
        }
        let current = || live.get();
        let mut reader = Reader::new(Late(&live, &calls), &current);
        let mut bytes = [0];
        assert!(reader.read(&mut bytes).is_err());
        assert!(reader.read(&mut bytes).is_err());
        assert_eq!(calls.get(), 1);
    }
    #[test]
    fn seek_is_not_allowed_after_retirement() {
        let current = || false;
        let mut reader = Reader::new(io::Cursor::new(vec![1, 2]), &current);
        assert!(reader.seek(SeekFrom::Start(1)).is_err());
        assert_eq!(reader.inner.position(), 0);
    }
    #[test]
    fn a_stalled_scan_is_retired_before_its_reader_returns_or_can_publish_dates() {
        use super::super::{added, Library};
        use std::sync::atomic::AtomicBool;
        let dir = std::env::temp_dir().join(format!(
            "sonante-own-scan-watch-{}",
            super::super::id(&format!("{:?}", std::time::SystemTime::now()))
        ));
        std::fs::create_dir(&dir).unwrap();
        let config = dir.join("folders.json");
        let library = Library::new(config.clone());
        library.epoch.store(2, Ordering::Release);
        {
            let mut s = library.data.lock().unwrap();
            s.status.epoch = 2;
            s.status.phase = "scanning".into();
        }
        let c = Arc::new(Capacity(AtomicUsize::new(0), 1));
        let permit = Admission::from(c.clone()).unwrap();
        let failed = library.clone();
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let entered = Arc::new(AtomicBool::new(false));
        let e = entered.clone();
        let worker_library = library.clone();
        let worker = std::thread::spawn(move || {
            let _permit = permit;
            let scope = Scope::with_timeout(Duration::from_millis(40), move || {
                failed.scan_failed(2, "index_timeout");
                tx.send(()).unwrap();
            })
            .unwrap();
            struct Stalled {
                entered: Arc<AtomicBool>,
                wait: mpsc::Receiver<()>,
            }
            impl Read for Stalled {
                fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                    self.entered.store(true, Ordering::Release);
                    self.wait.recv().unwrap();
                    b[0] = 1;
                    Ok(1)
                }
            }
            let active = || worker_library.current(2) && scope.current();
            let mut reader = Reader::new(Stalled { entered: e, wait }, &active);
            assert!(reader.read(&mut [0]).is_err());
            assert!(!scope.checkpoint());
            assert!(!worker_library.publish(2, &mut Vec::new(), 0, 0, true, false, None));
        });
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(entered.load(Ordering::Acquire));
        assert_eq!(library.status().unwrap().error, Some("index_timeout"));
        assert!(!library.current(2));
        assert!(matches!(Admission::from(c.clone()), Err("engine_busy")));
        assert!(
            added::observe(&config, &Default::default(), &[], &[], true, 10, || library
                .current(2))
            .is_err()
        );
        assert!(!added::path(&config).exists());
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(c.0.load(Ordering::Acquire), 0);
        library.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn an_old_scan_timeout_never_marks_a_newer_catalog_failed() {
        let dir = std::env::temp_dir().join(format!(
            "sonante-own-scan-generation-{}",
            super::super::id(&format!("{:?}", std::time::SystemTime::now()))
        ));
        std::fs::create_dir(&dir).unwrap();
        let library = super::super::Library::new(dir.join("folders.json"));
        library.epoch.store(3, Ordering::Release);
        {
            let mut s = library.data.lock().unwrap();
            s.status.epoch = 3;
            s.status.phase = "ready".into();
        }
        library.scan_failed(2, "index_timeout");
        assert!(library.current(3));
        assert_eq!(library.status().unwrap().phase, "ready");
        assert_eq!(library.status().unwrap().error, None);
        library.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn exhausted_scan_capacity_never_persists_or_replaces_the_folder_choice() {
        let dir = std::env::temp_dir().join(format!(
            "sonante-own-scan-admission-{}",
            super::super::id(&format!("{:?}", std::time::SystemTime::now()))
        ));
        std::fs::create_dir(&dir).unwrap();
        let root = dir.join("music");
        std::fs::create_dir(&root).unwrap();
        let config = dir.join("folders.json");
        let library = super::super::Library::new(config.clone());
        let permits: Vec<_> = (0..4).map(|_| Admission::acquire().unwrap()).collect();
        assert!(matches!(library.select(root), Err("engine_busy")));
        assert!(!config.exists());
        assert_eq!(library.status().unwrap().epoch, 0);
        assert_eq!(library.status().unwrap().phase, "idle");
        drop(permits);
        library.stop();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
