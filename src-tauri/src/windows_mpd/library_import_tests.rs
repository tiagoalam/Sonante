use super::super::{metadata_record, Library, Sources};
use super::*;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    library: Library,
}
impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).unwrap();
        let base = std::env::temp_dir().join(format!("sonante-own-import-{random:x?}"));
        let root = base.join("music");
        fs::create_dir_all(&root).unwrap();
        wave(&root.join("one.wav"));
        let library = Library::new(base.join("state/folders.json"));
        Self {
            base,
            root,
            library,
        }
    }
    fn record(&self) -> Record {
        metadata_record(&self.root.join("one.wav"), &self.root, &|| true).0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.library.stop();
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn wave(path: &Path) {
    let mut b = b"WAVEfmt ".to_vec();
    b.extend(16u32.to_le_bytes());
    b.extend([1, 0, 2, 0]);
    b.extend(44100u32.to_le_bytes());
    b.extend(176400u32.to_le_bytes());
    b.extend([4, 0, 16, 0]);
    b.extend(b"data");
    b.extend(176400u32.to_le_bytes());
    b.extend(vec![0u8; 176400]);
    let mut info = b"INFO".to_vec();
    for (key, value) in [
        (b"INAM", "Own track"),
        (b"IPRD", "Own album"),
        (b"IART", "Own artist"),
    ] {
        let mut v = value.as_bytes().to_vec();
        v.push(0);
        info.extend(key);
        info.extend((v.len() as u32).to_le_bytes());
        info.extend(&v);
        if v.len() % 2 != 0 {
            info.push(0);
        }
    }
    b.extend(b"LIST");
    b.extend((info.len() as u32).to_le_bytes());
    b.extend(info);
    let mut file = b"RIFF".to_vec();
    file.extend((b.len() as u32).to_le_bytes());
    file.extend(b);
    fs::write(path, file).unwrap();
}
fn phase(c: &Coordinator) -> Phase {
    c.snapshot().unwrap().unwrap().phase
}
fn accepted(response: SaveResponse) -> Snapshot {
    match response {
        SaveResponse::Accepted { operation } => operation,
        _ => panic!("not accepted"),
    }
}
fn terminal(lib: &Library) -> Snapshot {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let s = lib.imports.snapshot().unwrap().unwrap();
        if s.phase.terminal() {
            return s;
        }
        assert!(Instant::now() < end, "import did not terminate");
        std::thread::yield_now();
    }
}
#[test]
fn admission_and_mode_only_save_never_invent_a_job() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(false, false).unwrap();
    assert!(c.snapshot().unwrap().is_none());
    assert!(matches!(c.admit(true, false), Err(BUSY)));
    assert!(matches!(g.commit(), Ok(SaveResponse::Completed)));
    assert!(c.snapshot().unwrap().is_none());
}
#[test]
fn preparing_never_exposes_candidate_and_fast_completion_is_truthful() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    let id = c.snapshot().unwrap().unwrap().id;
    g.bind(42, &[]).unwrap();
    assert!(c.preview(&id).is_err());
    c.validating(42);
    c.finish(42, None);
    assert_eq!(phase(&c), Phase::Preparing);
    assert!(matches!(g.commit(), Ok(SaveResponse::Completed)));
    assert_eq!(phase(&c), Phase::Completed);
}
#[test]
fn complete_config_ack_keeps_admission_until_its_exact_scanner_finishes() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    g.bind(41, &[]).unwrap();
    let op = accepted(g.commit().unwrap());
    assert_eq!(op.phase, Phase::Indexing);
    assert!(!op.confirmed_playback);
    assert!(matches!(c.admit(false, false), Err(BUSY)));
    c.finish(40, None);
    assert_eq!(phase(&c), Phase::Indexing);
    c.validating(41);
    assert_eq!(phase(&c), Phase::Validating);
    c.finish(41, None);
    assert_eq!(phase(&c), Phase::Completed);
    c.admit(false, false).unwrap().commit().unwrap();
}
#[test]
fn failed_before_commit_and_dropped_transaction_do_not_masquerade_as_completed() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    g.bind(1, &[]).unwrap();
    c.finish(1, Some("index_timeout"));
    let op = accepted(g.commit().unwrap());
    assert_eq!(op.phase, Phase::Failed);
    assert_eq!(op.error, Some("index_timeout"));
    let g = c.admit(true, false).unwrap();
    g.bind(2, &[]).unwrap();
    drop(g);
    assert_eq!(c.snapshot().unwrap().unwrap().error, Some("worker_failed"));
    c.finish(2, None);
    assert_eq!(phase(&c), Phase::Failed);
}
#[test]
fn rollback_revokes_preview_and_uncertain_never_resets_itself() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    g.bind(1, &[]).unwrap();
    g.fail("windows_config_uncertain");
    assert_eq!(phase(&c), Phase::Uncertain);
    assert!(matches!(
        c.admit(false, false),
        Err("windows_config_uncertain")
    ));
    c.finish(1, None);
    assert_eq!(phase(&c), Phase::Uncertain);
}
#[test]
fn physical_page_reservation_outlives_revoked_operation_and_throttle() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    g.bind(1, &[]).unwrap();
    let id = accepted(g.commit().unwrap()).id;
    let lease = c.preview(&id).unwrap();
    c.finish(1, None);
    assert!(lease.read().is_err());
    let g = c.admit(true, false).unwrap();
    g.bind(2, &[]).unwrap();
    let id2 = accepted(g.commit().unwrap()).id;
    assert_ne!(id, id2);
    assert!(matches!(c.preview(&id2), Err("LOCAL_IMPORT_PREVIEW_BUSY")));
    drop(lease);
    assert!(matches!(c.preview(&id2), Err("LOCAL_IMPORT_PREVIEW_BUSY")));
    c.inner.lock().unwrap().preview_not_before = None;
    let new = c.preview(&id2).unwrap();
    assert_eq!(new.read().unwrap().operation_id, id2);
}
#[test]
fn supersession_shutdown_and_wrong_job_cannot_revive_pages() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, true).unwrap();
    g.bind(1, &[]).unwrap();
    let id = accepted(g.commit().unwrap()).id;
    let lease = c.preview(&id).unwrap();
    c.supersede(2);
    assert!(lease.read().is_err());
    c.finish(1, None);
    assert_eq!(phase(&c), Phase::Failed);
    let g = c.admit(true, false).unwrap();
    g.bind(3, &[]).unwrap();
    g.commit().unwrap();
    c.shutdown();
    assert_eq!(
        c.snapshot().unwrap().unwrap().error,
        Some("LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN")
    );
    assert!(matches!(c.admit(false, false), Err("shutting_down")));
}
#[test]
fn actual_metadata_preview_is_bounded_scoped_nonplayable_and_contains_no_catalog_ids() {
    let f = Fixture::new();
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, true).unwrap();
    g.bind(11, &[f.root.clone()]).unwrap();
    let record = f.record();
    let album_id = record.album.id.clone();
    let track_id = record.tracks[0].id.clone();
    let mut wrong = f.record();
    wrong.root = f.base.join("unselected");
    c.feed(11, &[wrong]);
    for _ in 0..200 {
        c.feed(11, &[f.record()]);
    }
    let id = accepted(g.commit().unwrap()).id;
    let lease = c.preview(&id).unwrap();
    let p = lease.read().unwrap();
    let j = serde_json::to_value(&p).unwrap();
    assert_eq!(j["sampled_tracks"], 128);
    assert_eq!(j["albums"].as_array().unwrap().len(), 1);
    assert_eq!(j["albums"][0]["album"], "Own album");
    assert_eq!(j["albums"][0]["artist"], "Own artist");
    assert_eq!(
        j["albums"][0]["source_id"],
        source_scope::source_id(&f.root)
    );
    assert_eq!(j["albums"][0]["observed_tracks"], 128);
    assert_eq!(j["provisional"], true);
    assert_eq!(j["playable"], false);
    assert_eq!(j["complete"], false);
    assert_eq!(j["initial"], true);
    let raw = j.to_string();
    assert!(!raw.contains(&album_id));
    assert!(!raw.contains(&track_id));
    assert!(!raw.contains(&f.root.to_string_lossy().to_string()));
    assert!(j["albums"][0].get("artwork_ref").is_none());
    assert!(j["albums"][0].get("image").is_none());
    if let Ok(path) = std::env::var("SONANTE_OWN_IMPORT_JSON") {
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({"operation":c.snapshot().unwrap(),"preview":j}))
                .unwrap(),
        )
        .unwrap();
    }
    c.validating(11);
    assert!(lease.read().is_err());
}
#[test]
fn row_fields_obey_original_ui_limit_and_groups_are_operation_scoped() {
    let f = Fixture::new();
    let c = Arc::new(Coordinator::default());
    let mut keys = Vec::new();
    for epoch in 1..=2 {
        let g = c.admit(true, false).unwrap();
        g.bind(epoch, &[f.root.clone()]).unwrap();
        let mut r = f.record();
        r.album.title = "é".repeat(5000);
        r.album.artist = "🎵".repeat(5000);
        c.feed(epoch, &[r]);
        let id = accepted(g.commit().unwrap()).id;
        c.inner.lock().unwrap().preview_not_before = None;
        let lease = c.preview(&id).unwrap();
        let p = lease.read().unwrap();
        assert_eq!(p.albums[0].album.as_ref().unwrap().chars().count(), 256);
        assert_eq!(
            p.albums[0].artist.as_ref().unwrap().encode_utf16().count(),
            256
        );
        keys.push(p.albums[0].provisional_group_key.clone());
        drop(lease);
        c.finish(epoch, None);
    }
    assert_ne!(keys[0], keys[1]);
}
#[test]
fn actual_scanner_config_job_qualifies_then_reopens_as_a_distinct_initial_job() {
    let f = Fixture::new();
    let g = f.library.imports.admit(true, true).unwrap();
    f.library
        .configure_import_preferences(
            Some(vec![f.root.clone()]),
            Sources::default(),
            None,
            None,
            None,
            Some(&g),
        )
        .unwrap();
    g.commit().unwrap();
    let op = terminal(&f.library);
    assert_eq!(op.phase, Phase::Completed);
    assert_eq!(f.library.status().unwrap().tracks, 1);
    let bytes = fs::read(&f.library.config).unwrap();
    let reopened = Library::new(f.library.config.clone());
    assert_eq!(terminal(&reopened).phase, Phase::Completed);
    assert_eq!(reopened.status().unwrap().tracks, 1);
    assert_eq!(fs::read(&f.library.config).unwrap(), bytes);
    reopened.stop();
}
#[test]
fn actual_scanner_warned_parse_fails_operation_instead_of_fake_coverage_success() {
    let f = Fixture::new();
    fs::write(f.root.join("broken.flac"), b"not a flac").unwrap();
    let g = f.library.imports.admit(true, false).unwrap();
    f.library
        .configure_import_preferences(
            Some(vec![f.root.clone()]),
            Sources::default(),
            None,
            None,
            None,
            Some(&g),
        )
        .unwrap();
    g.commit().unwrap();
    let op = terminal(&f.library);
    assert_eq!(op.phase, Phase::Failed);
    assert_eq!(op.error, Some(UNCONFIRMED));
    assert!(f.library.status().unwrap().warnings > 0);
    assert!(serde_json::to_value(op).unwrap().get("coverage").is_none());
}
#[test]
fn real_store_failure_and_transaction_rollback_never_publish_a_successful_candidate() {
    let f = Fixture::new();
    let g = f.library.imports.admit(true, false).unwrap();
    let code = f
        .library
        .configure_import_preferences(
            Some(vec![f.base.join("missing")]),
            Sources::default(),
            None,
            None,
            None,
            Some(&g),
        )
        .err()
        .unwrap();
    g.fail(code);
    assert_eq!(phase(&f.library.imports), Phase::Failed);
    assert!(!f.library.config.exists());
    let g = f.library.imports.admit(true, false).unwrap();
    f.library
        .configure_import_preferences(
            Some(vec![f.root.clone()]),
            Sources::default(),
            None,
            None,
            None,
            Some(&g),
        )
        .unwrap();
    f.library
        .configure_import_preferences(Some(vec![]), Sources::default(), None, None, None, None)
        .unwrap();
    g.fail("PLEX_UNAVAILABLE");
    assert_eq!(
        f.library.imports.snapshot().unwrap().unwrap().error,
        Some("PLEX_UNAVAILABLE")
    );
    assert!(f.library.configured_roots().unwrap().is_empty());
}
#[test]
fn close_during_admitted_transaction_keeps_exact_interruption_instead_of_drop_error() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    g.bind(1, &[]).unwrap();
    c.shutdown();
    assert!(matches!(
        g.commit(),
        Err("LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN")
    ));
    assert_eq!(
        c.snapshot().unwrap().unwrap().error,
        Some("LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN")
    );
}
#[test]
fn real_rollback_uses_original_transaction_and_restores_roots_without_binding_rollback_scan() {
    let f = Fixture::new();
    let sources = Sources {
        local: true,
        navidrome: false,
        plex: true,
    };
    let owner = f.library.imports.admit(true, false).unwrap();
    let mut first = true;
    let mut plex_calls = 0;
    let result = super::super::super::config_transaction::apply_all_options(
        (Vec::new(), Sources::default(), ()),
        Some(vec![f.root.clone()]),
        sources,
        (),
        true,
        |roots, flags, ()| {
            let bind = if first {
                first = false;
                Some(&owner)
            } else {
                None
            };
            f.library
                .configure_import_preferences(roots, flags, None, None, None, bind)
                .map(|_| ())
        },
        |_| panic!("unchanged Nav"),
        |_| {
            plex_calls += 1;
            if plex_calls == 1 {
                Err("PLEX_UNAVAILABLE")
            } else {
                Ok(())
            }
        },
        || true,
    );
    assert_eq!(result, Err("PLEX_UNAVAILABLE"));
    owner.fail(result.err().unwrap());
    assert!(f.library.configured_roots().unwrap().is_empty());
    assert_eq!(
        f.library.imports.snapshot().unwrap().unwrap().error,
        Some("PLEX_UNAVAILABLE")
    );
}
#[test]
fn manual_refresh_and_check_cannot_steal_an_admitted_config_job() {
    let f = Fixture::new();
    let owner = f.library.imports.admit(true, false).unwrap();
    assert!(matches!(f.library.refresh(), Err(BUSY)));
    assert!(matches!(f.library.check_changes(None, None), Err(BUSY)));
    assert!(matches!(
        f.library.select_roots(vec![f.root.clone()]),
        Err(BUSY)
    ));
    drop(owner);
}
#[test]
fn worker_unwind_or_early_return_fails_only_the_exact_unfinished_import() {
    let c = Arc::new(Coordinator::default());
    let g = c.admit(true, false).unwrap();
    g.bind(1, &[]).unwrap();
    g.commit().unwrap();
    assert!(std::panic::catch_unwind(|| {
        let _worker = c.worker(1);
        panic!("own synthetic worker unwind");
    })
    .is_err());
    assert_eq!(c.snapshot().unwrap().unwrap().error, Some("worker_failed"));
    let g = c.admit(true, false).unwrap();
    g.bind(2, &[]).unwrap();
    drop(c.worker(1));
    assert_eq!(phase(&c), Phase::Preparing);
    c.finish(2, None);
    drop(c.worker(2));
    assert!(matches!(g.commit(), Ok(SaveResponse::Completed)));
}
#[test]
fn corrupt_added_data_is_preserved_and_cannot_produce_successful_import_completion() {
    let f = Fixture::new();
    fs::create_dir_all(f.library.config.parent().unwrap()).unwrap();
    let path = f.library.config.with_extension("added-v1.json");
    fs::write(&path, b"corrupt own fixture").unwrap();
    let g = f.library.imports.admit(true, false).unwrap();
    f.library
        .configure_import_preferences(
            Some(vec![f.root.clone()]),
            Sources::default(),
            None,
            None,
            None,
            Some(&g),
        )
        .unwrap();
    g.commit().unwrap();
    assert_eq!(terminal(&f.library).phase, Phase::Failed);
    assert_eq!(fs::read(path).unwrap(), b"corrupt own fixture");
    assert_eq!(f.library.status().unwrap().tracks, 1);
}
#[path="library_import_artwork_tests.rs"]
mod artwork;
