use super::*;
use crate::windows_mpd::{TestJournal as Journal, TestSavedQueue as SavedQueue};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
struct Fixture {
    root: PathBuf,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let mut r = [0; 12];
        getrandom::fill(&mut r).unwrap();
        let root = std::env::temp_dir().join(format!("sonante-queue-finish-{r:x?}"));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("queue.json");
        Self { root, path }
    }
    fn journal(&self) -> Journal {
        let t = crate::windows_mpd::library::Track {
            id: "fixture".into(),
            catalog_album_id: Some("album".into()),
            title: "Fixture".into(),
            artist: "Fixture".into(),
            album: "Fixture".into(),
            disc: 1,
            number: 1,
            file_name: "not-opened.flac".into(),
            media_root: None,
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
            path: self.root.join("not-opened.flac"),
        };
        let q = SavedQueue::capture(&self.root, &[t], 0, 23.5, 120., 37, Some("album".into()));
        let mut j = Journal::open(self.path.clone());
        j.replace_logical(Some(q)).unwrap();
        j
    }
    fn block(&self) {
        std::fs::rename(&self.path, self.root.join("backup.json")).unwrap();
        std::fs::create_dir(&self.path).unwrap()
    }
    fn unblock(&self) {
        std::fs::remove_dir(&self.path).unwrap();
        std::fs::rename(self.root.join("backup.json"), &self.path).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap()
    }
}
struct Owned(Arc<AtomicUsize>);
impl Drop for Owned {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
#[test]
fn proven_empty_is_persisted_and_owned_route_closed_once_without_replay_or_media() {
    let f = Fixture::new();
    let mut j = f.journal();
    let drops = Arc::new(AtomicUsize::new(0));
    let mut current = Some(Owned(drops.clone()));
    let mut closes = 0;
    assert_eq!(
        confirmed_empty(&mut j, &mut current, |_| {
            closes += 1;
            assert!(Journal::open(f.path.clone()).saved.is_none());
            Ok(())
        }),
        Ok(())
    );
    assert_eq!(closes, 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(current.is_none());
    assert!(j.saved.is_none());
    assert!(j.warning.is_none());
    assert!(Journal::open(f.path.clone()).saved.is_none());
    assert!(!f.root.join("not-opened.flac").exists());
}
#[test]
fn old_finalization_resurrected_old_queue_new_live_observation_stays_empty_and_retry_only_persists()
{
    let f = Fixture::new();
    let mut old = f.journal();
    let mut j = Journal::open(f.path.clone());
    let bytes = std::fs::read(&f.path).unwrap();
    f.block();
    // Exact former worker order: owned MPD proof + abort/drop, replace_logical(None).
    assert_eq!(old.replace_logical(None), Err("queue_save_failed"));
    assert!(old.saved.is_some());
    assert!(old.warning.is_none());
    old.flush();
    assert!(old.saved.is_some());
    let drops = Arc::new(AtomicUsize::new(0));
    let mut current = Some(Owned(drops.clone()));
    let mut closes = 0;
    assert_eq!(
        confirmed_empty(&mut j, &mut current, |_| {
            closes += 1;
            Ok(())
        }),
        Err("PLAYBACK_PERSISTENCE_FAILED")
    );
    assert!(j.saved.is_none());
    assert_eq!(j.warning, Some("queue_save_failed"));
    assert!(current.is_none());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(closes, 1);
    assert_eq!(std::fs::read(f.root.join("backup.json")).unwrap(), bytes);
    j.flush();
    assert!(j.saved.is_none());
    assert_eq!(j.warning, Some("queue_save_failed"));
    assert_eq!(closes, 1);
    f.unblock();
    old.flush();
    assert_eq!(
        std::fs::read(&f.path).unwrap(),
        bytes,
        "old journal was not dirty and could not persist its confirmed empty effect"
    );
    j.flush();
    assert!(j.warning.is_none());
    assert!(Journal::open(f.path.clone()).saved.is_none());
    assert_eq!(closes, 1);
    assert_eq!(std::fs::read_dir(&f.root).unwrap().count(), 1);
}
#[test]
fn failed_cleanup_retains_owned_route_but_never_invents_old_confirmed_queue() {
    for write_fails in [false, true] {
        let f = Fixture::new();
        let mut j = f.journal();
        if write_fails {
            f.block()
        };
        let drops = Arc::new(AtomicUsize::new(0));
        let mut current = Some(Owned(drops.clone()));
        let mut closes = 0;
        assert_eq!(
            confirmed_empty(&mut j, &mut current, |_| {
                closes += 1;
                Err("shutdown_failed")
            }),
            Err("shutdown_failed")
        );
        assert!(current.is_some());
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(j.saved.is_none());
        assert_eq!(closes, 1);
        assert_eq!(j.warning.is_some(), write_fails);
        if write_fails {
            f.unblock();
            j.flush()
        };
        assert!(Journal::open(f.path.clone()).saved.is_none());
        assert_eq!(closes, 1);
        drop(current);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn missing_owned_route_cannot_clear_a_logical_queue() {
    let f = Fixture::new();
    let mut j = f.journal();
    let bytes = std::fs::read(&f.path).unwrap();
    let mut absent: Option<Owned> = None;
    assert_eq!(
        confirmed_empty(&mut j, &mut absent, |_| panic!("no cleanup authority")),
        Err("no_track")
    );
    assert!(j.saved.is_some());
    assert_eq!(std::fs::read(&f.path).unwrap(), bytes);
}
#[test]
fn logical_stop_failure_rolls_back_position_while_old_observe_published_unsaved_zero() {
    let f = Fixture::new();
    let mut old = f.journal();
    let mut j = Journal::open(f.path.clone());
    let mut stopped = j.saved.clone().unwrap();
    stopped.elapsed = 0.;
    let bytes = std::fs::read(&f.path).unwrap();
    f.block();
    old.observe(stopped.clone(), true);
    assert_eq!(old.saved.as_ref().unwrap().elapsed, 0.);
    assert_eq!(old.warning, Some("queue_save_failed"));
    assert_eq!(
        j.replace_logical(Some(stopped.clone())),
        Err("queue_save_failed")
    );
    assert_eq!(j.saved.as_ref().unwrap().elapsed, 23.5);
    assert_eq!(j.saved.as_ref().unwrap().volume, 37);
    assert_eq!(j.saved.as_ref().unwrap().tracks()[0].id, "fixture");
    assert_eq!(std::fs::read(f.root.join("backup.json")).unwrap(), bytes);
    f.unblock();
    j.replace_logical(Some(stopped)).unwrap();
    let reopened = Journal::open(f.path.clone());
    assert_eq!(reopened.saved.as_ref().unwrap().elapsed, 0.);
    assert_eq!(reopened.saved.as_ref().unwrap().volume, 37);
    assert!(!f.root.join("not-opened.flac").exists());
}
