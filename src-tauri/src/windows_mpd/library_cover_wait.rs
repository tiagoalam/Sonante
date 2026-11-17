//! A restored cover may arrive before the read-only scanner publishes its album.
//! Wait only within that scan epoch, without catalog locks, polling or MPD I/O.
use super::*;
use std::{
    sync::atomic::AtomicUsize,
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_READERS: usize = 8;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
pub(super) struct Admission<'a>(&'a AtomicUsize);
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
fn acquire(counter: &AtomicUsize) -> Result<Admission<'_>> {
    counter
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            (active < MAX_READERS).then_some(active + 1)
        })
        .map(|_| Admission(counter))
        .map_err(|_| "engine_busy")
}
pub(super) fn admit() -> Result<Admission<'static>> {
    acquire(&ACTIVE)
}
pub(super) fn capture(library: &Library, album: &str) -> Result<CoverSource> {
    capture_with_timeout(library, album, TIMEOUT)
}
fn capture_with_timeout(library: &Library, album: &str, timeout: Duration) -> Result<CoverSource> {
    let epoch = library.status()?.epoch;
    capture_epoch(library, album, timeout, epoch)
}
fn capture_epoch(
    library: &Library,
    album: &str,
    timeout: Duration,
    epoch: u64,
) -> Result<CoverSource> {
    let deadline = Instant::now() + timeout;
    loop {
        // Capture the wake revision before observing the catalog. Publication
        // cannot be lost between that observation and the condvar wait.
        let seen = library.checks.revision()?;
        {
            let state = library.data.lock().map_err(|_| "library_state")?;
            if state.status.epoch != epoch || !library.current(epoch) {
                return Err("library_changed");
            }
            if let Some(record) = state.albums.get(album) {
                return Ok(CoverSource {
                    album: album.into(),
                    epoch,
                    root: record.root.clone(),
                    cover: record.cover.clone(),
                    media: record.tracks.first().ok_or("library_changed")?.path.clone(),
                });
            }
            // A complete catalog missing this ID, failed scan, stop or new
            // root remains an error. Never turn it into an empty cover hit.
            if state.status.phase != "scanning" {
                return Err(state.status.error.unwrap_or("library_changed"));
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("cover_unavailable");
        }
        library.checks.wait(seen, remaining)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    fn pending() -> Library {
        // This path is absent, unique and confined to the fixture directory.
        let mut random = [0; 12];
        getrandom::fill(&mut random).unwrap();
        let library =
            Library::new(std::env::temp_dir().join(format!("sonante-cover-wait-{random:x?}.json")));
        library.data.lock().unwrap().status.phase = "scanning".into();
        library
    }
    fn record() -> Record {
        Record {
            root: PathBuf::from("fixture-root"),
            album: Album {
                id: "album".into(),
                title: "Fixture".into(),
                artist: "Fixture".into(),
                year: None,
                genre: None,
                track_count: 1,
                has_cover: false,
                added_at: None,
            },
            tracks: vec![Track {
                id: "track".into(),
                title: "Fixture".into(),
                artist: "Fixture".into(),
                album: "Fixture".into(),
                disc: 1,
                number: 1,
                file_name: "fixture.flac".into(),
                catalog_album_id: Some("album".into()),
                path: PathBuf::from("fixture-root/fixture.flac"),
                media_root: Some(PathBuf::from("fixture-root")),
                radio: None,upnp:None,
                remote_source: None,
                remote_connection_id: None,
                plex_image:None,remote_lease: None,
            }],
            folder: PathBuf::from("fixture-root"),
            cover: None,
            search: String::new(),
            durations: HashMap::new(),
        }
    }
    // No filesystem, MPD or audio reads: publication is controlled deterministically.
    fn waiter(
        library: &Library,
    ) -> (
        std::thread::JoinHandle<()>,
        mpsc::Receiver<Result<CoverSource>>,
    ) {
        let (tx, rx) = mpsc::channel();
        let library = library.clone();
        let epoch = library.status().unwrap().epoch;
        let thread = std::thread::spawn(move || {
            tx.send(capture_epoch(
                &library,
                "album",
                Duration::from_secs(2),
                epoch,
            ))
            .unwrap();
        });
        (thread, rx)
    }
    #[test]
    fn cold_cover_waits_for_its_album_then_returns_the_exact_published_context() {
        let library = pending();
        let (thread, rx) = waiter(&library);
        assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
        let mut batch = vec![record()];
        assert!(library.publish(0, &mut batch, 0, 0, false, false, None));
        let selected = rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        thread.join().unwrap();
        let former = CoverSource::capture(&library, "album").unwrap();
        assert_eq!(
            (
                &selected.album,
                selected.epoch,
                &selected.root,
                &selected.cover,
                &selected.media
            ),
            (
                &former.album,
                former.epoch,
                &former.root,
                &former.cover,
                &former.media
            )
        );
        assert_eq!(selected.validate(&library), Ok(()));
        assert_eq!(
            library.status().unwrap().phase,
            "scanning",
            "not waiting for unrelated albums"
        );
    }
    #[test]
    fn scan_completion_missing_album_and_real_failure_are_not_hidden() {
        for error in [None, Some("worker_failed")] {
            let library = pending();
            let (thread, rx) = waiter(&library);
            assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
            {
                let mut state = library.data.lock().unwrap();
                state.status.phase = if error.is_some() { "failed" } else { "ready" }.into();
                state.status.error = error;
            }
            library.checks.notify();
            assert!(
                matches!(rx.recv_timeout(Duration::from_secs(1)).unwrap(),Err(code) if code==error.unwrap_or("library_changed"))
            );
            thread.join().unwrap();
        }
    }
    #[test]
    fn new_epoch_or_stop_wakes_waiter_and_refuses_stale_album() {
        for stop in [false, true] {
            let library = pending();
            let (thread, rx) = waiter(&library);
            assert!(rx.recv_timeout(Duration::from_millis(20)).is_err());
            if stop {
                library.stop();
            } else {
                {
                    let mut state = library.data.lock().unwrap();
                    state.status.epoch = 1;
                    state.albums.insert("album".into(), record());
                    library.epoch.store(1, Ordering::Release);
                }
                library.checks.notify();
            }
            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                Err("library_changed")
            ));
            thread.join().unwrap();
        }
    }
    #[test]
    fn pending_timeout_is_a_failure_and_never_empty_success() {
        let library = pending();
        assert!(matches!(
            capture_with_timeout(&library, "album", Duration::from_millis(10)),
            Err("cover_unavailable")
        ));
        assert_eq!(library.status().unwrap().phase, "scanning");
    }
    #[test]
    fn physical_cover_capacity_is_bounded_and_released_by_raii() {
        let counter = AtomicUsize::new(0);
        let mut guards: Vec<_> = (0..MAX_READERS)
            .map(|_| acquire(&counter).unwrap())
            .collect();
        assert!(matches!(acquire(&counter), Err("engine_busy")));
        guards.pop();
        let last = acquire(&counter).unwrap();
        assert_eq!(counter.load(Ordering::Acquire), MAX_READERS);
        drop(guards);
        drop(last);
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }
    #[test]
    fn signal_publication_before_wait_does_not_get_lost() {
        let library = pending();
        let seen = library.checks.revision().unwrap();
        library.checks.notify();
        let start = Instant::now();
        library.checks.wait(seen, Duration::from_secs(1)).unwrap();
        assert!(start.elapsed() < Duration::from_millis(200));
    }
}
