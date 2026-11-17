use super::*;
use std::sync::mpsc;
struct Fixture {
    base: PathBuf,
    media: PathBuf,
    library: Library,
}
impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).unwrap();
        let base = std::env::temp_dir().join(format!("sonante-library-check-{random:x?}"));
        let media = base.join("Acervo 日本");
        fs::create_dir_all(&media).unwrap();
        let library = Library::new(base.join("state/folder.json"));
        library.select(media.clone()).unwrap();
        ready(&library);
        Self {
            base,
            media,
            library,
        }
    }
    fn expected(&self) -> Vec<String> {
        vec![ui_root(
            self.library.configured_root().unwrap().as_ref().unwrap(),
        )]
    }
    fn check(&self) -> Result<bool> {
        self.library
            .check_changes(Some("windows-local"), Some(&self.expected()))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.library.stop();
        fs::remove_dir_all(&self.base).unwrap();
    }
}
fn ready(library: &Library) {
    let end = Instant::now() + Duration::from_secs(3);
    while library.status().unwrap().phase == "scanning" {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(library.status().unwrap().phase, "ready");
}
fn wave(path: &Path, title: &str) {
    let mut body = b"WAVEfmt ".to_vec();
    body.extend(16u32.to_le_bytes());
    body.extend([1, 0, 2, 0]);
    body.extend(44100u32.to_le_bytes());
    body.extend(176400u32.to_le_bytes());
    body.extend([4, 0, 16, 0]);
    body.extend(b"data");
    body.extend(176400u32.to_le_bytes());
    body.extend(vec![0; 176400]);
    let mut info = b"INFO".to_vec();
    for (key, value) in [(b"INAM", title), (b"IPRD", "Álbum"), (b"IART", "Artista")] {
        let mut value = value.as_bytes().to_vec();
        value.push(0);
        info.extend(key);
        info.extend((value.len() as u32).to_le_bytes());
        info.extend(&value);
        if value.len() % 2 != 0 {
            info.push(0);
        }
    }
    body.extend(b"LIST");
    body.extend((info.len() as u32).to_le_bytes());
    body.extend(info);
    let mut file = b"RIFF".to_vec();
    file.extend((body.len() as u32).to_le_bytes());
    file.extend(body);
    fs::write(path, file).unwrap();
}
#[test]
fn completed_empty_and_unchanged_scans_return_false_and_preserve_catalog_models() {
    let f = Fixture::new();
    assert_eq!(f.check(), Ok(false));
    wave(&f.media.join("01 Canção.wav"), "Primeira");
    assert_eq!(f.check(), Ok(true));
    let before = f.library.compatibility_catalog().unwrap()["albums"].clone();
    let id = before[0]["id"].as_str().unwrap();
    let tracks = f.library.compatibility_tracks(id).unwrap();
    let root = f.library.configured_root().unwrap();
    let bytes = fs::read(&f.library.config).unwrap();
    assert_eq!(f.check(), Ok(false));
    assert_eq!(f.library.status().unwrap().phase, "ready");
    assert_eq!(f.library.compatibility_catalog().unwrap()["albums"], before);
    assert_eq!(f.library.compatibility_tracks(id).unwrap(), tracks);
    assert_eq!(f.library.configured_root().unwrap(), root);
    assert_eq!(fs::read(&f.library.config).unwrap(), bytes);
}
#[test]
fn addition_removal_and_same_path_tag_rewrite_are_confirmed_after_scan() {
    let f = Fixture::new();
    let path = f.media.join("Seconda.wav");
    wave(&path, "One");
    assert_eq!(f.check(), Ok(true));
    wave(&path, "Título alterado com tamanho diferente");
    assert_eq!(f.check(), Ok(true));
    assert_eq!(
        f.library.compatibility_search("alterado").unwrap()["tracks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    fs::remove_file(path).unwrap();
    assert_eq!(f.check(), Ok(true));
    assert_eq!(f.library.status().unwrap().tracks, 0);
    assert_eq!(f.check(), Ok(false));
}
#[test]
fn known_cover_add_edit_remove_count_but_unrelated_files_do_not() {
    let f = Fixture::new();
    wave(&f.media.join("one.wav"), "One");
    assert_eq!(f.check(), Ok(true));
    fs::write(f.media.join("notes.txt"), b"Unrelated fixture").unwrap();
    assert_eq!(f.check(), Ok(false));
    let cover = f.media.join("COVER.PNG");
    fs::write(&cover, b"\x89PNG\r\n\x1a\nfixture").unwrap();
    assert_eq!(f.check(), Ok(true));
    fs::write(&cover, b"\x89PNG\r\n\x1a\nfixture changed length").unwrap();
    assert_eq!(f.check(), Ok(true));
    fs::remove_file(cover).unwrap();
    assert_eq!(f.check(), Ok(true));
    assert_eq!(f.check(), Ok(false));
}
#[test]
fn stale_source_and_expected_folder_refuse_before_epoch_or_settings_mutation() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    let settings = fs::read(&f.library.config).unwrap();
    for (source, folders) in [
        (Some("other"), Some(f.expected())),
        (None, Some(vec![])),
        (None, Some(vec!["C:/stale".into()])),
        (
            Some("windows-local"),
            Some(vec!["one".into(), "two".into()]),
        ),
    ] {
        assert_eq!(
            f.library.check_changes(source, folders.as_deref()),
            Err(SCOPE_CHANGED)
        );
        assert_eq!(f.library.status().unwrap().epoch, epoch);
        assert_eq!(fs::read(&f.library.config).unwrap(), settings);
    }
    assert_eq!(f.check(), Ok(false));
}
#[test]
fn one_confirmation_and_a_scanning_catalog_refuse_duplicate_work() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    f.library.checks.active.store(true, Ordering::Release);
    assert_eq!(f.check(), Err("engine_busy"));
    assert_eq!(f.library.rescan_confirmed(), Err("engine_busy"));
    f.library.checks.active.store(false, Ordering::Release);
    {
        f.library.data.lock().unwrap().status.phase = "scanning".into();
    }
    assert_eq!(f.check(), Err("engine_busy"));
    assert_eq!(f.library.rescan_confirmed(), Err("engine_busy"));
    assert_eq!(f.library.status().unwrap().epoch, epoch);
}
#[test]
fn invalid_partial_failed_warned_and_missing_signature_never_confirm() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    for (phase, warnings, error, signature) in [
        ("partial", 0, Some("library_limit"), Some([0; 32])),
        ("failed", 0, Some("worker_failed"), None),
        ("ready", 1, None, Some([0; 32])),
        ("ready", 0, None, None),
        ("idle", 0, None, None),
    ] {
        {
            let mut state = f.library.data.lock().unwrap();
            state.status.phase = phase.into();
            state.status.warnings = warnings;
            state.status.error = error;
            state.signature = signature;
        }
        assert_eq!(
            f.library.wait_for_scan(epoch, Duration::from_millis(1)),
            Err(UNCONFIRMED)
        );
        assert_eq!(f.check(), Err(UNCONFIRMED));
    }
}
#[test]
fn waiting_releases_catalog_mutex_and_complete_publication_wakes_confirmation() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    {
        f.library.data.lock().unwrap().status.phase = "scanning".into();
    }
    let library = f.library.clone();
    let (tx, rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        tx.send(library.wait_for_scan(epoch, Duration::from_secs(1)))
            .unwrap();
    });
    {
        let mut state = f.library.data.lock().unwrap();
        state.status.phase = "ready".into();
        state.signature = Some([7; 32]);
        f.library.checks.notify();
    }
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Ok([7; 32])
    );
    waiter.join().unwrap();
}
#[test]
fn timeout_is_not_a_completed_scan_or_an_automatic_restart() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    {
        f.library.data.lock().unwrap().status.phase = "scanning".into();
    }
    assert_eq!(
        f.library.wait_for_scan(epoch, Duration::from_millis(2)),
        Err("windows_library_check_timeout")
    );
    assert_eq!(f.library.status().unwrap().phase, "scanning");
    assert_eq!(f.library.status().unwrap().epoch, epoch);
    assert_eq!(f.check(), Err("engine_busy"));
}
#[test]
fn changed_epoch_and_shutdown_invalidate_a_wait_without_success() {
    let f = Fixture::new();
    let epoch = f.library.status().unwrap().epoch;
    {
        f.library.data.lock().unwrap().status.phase = "scanning".into();
    }
    let library = f.library.clone();
    let (tx, rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        tx.send(library.wait_for_scan(epoch, Duration::from_secs(1)))
            .unwrap();
    });
    {
        let mut state = f.library.data.lock().unwrap();
        state.status.epoch += 1;
        f.library.epoch.fetch_add(1, Ordering::AcqRel);
        f.library.checks.notify();
    }
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Err(SCOPE_CHANGED)
    );
    waiter.join().unwrap();
    let epoch = f.library.status().unwrap().epoch;
    let library = f.library.clone();
    let waiter = std::thread::spawn(move || library.wait_for_scan(epoch, Duration::from_secs(1)));
    f.library.stop();
    assert_eq!(waiter.join().unwrap(), Err("shutting_down"));
    assert_eq!(f.check(), Err("shutting_down"));
}
#[test]
fn real_root_disappearance_keeps_old_catalog_and_does_not_ack_rescan() {
    let f = Fixture::new();
    fs::remove_dir(&f.media).unwrap();
    let epoch = f.library.status().unwrap().epoch;
    assert_eq!(f.library.rescan_confirmed(), Err("library_unavailable"));
    assert_eq!(f.library.status().unwrap().epoch, epoch);
}
#[test]
fn signature_is_order_independent_and_bounded_to_relevant_existing_metadata() {
    let f = Fixture::new();
    let a = f.media.join("one.wav");
    let b = f.media.join("folder.jpg");
    wave(&a, "One");
    fs::write(&b, b"fixture").unwrap();
    let (am, bm) = (
        fs::symlink_metadata(&a).unwrap(),
        fs::symlink_metadata(&b).unwrap(),
    );
    let mut first = Signature::new();
    first.observe(&a, &am);
    first.observe(&b, &bm);
    let mut second = Signature::new();
    second.observe(&b, &bm);
    second.observe(&a, &am);
    assert_eq!(first.finish(), second.finish());
    let mut capped = Signature {
        hashes: vec![[0; 32]; MAX_SIGNATURES],
        valid: true,
    };
    capped.observe(&a, &am);
    assert_eq!(capped.hashes.len(), MAX_SIGNATURES);
    assert_eq!(capped.finish(), None);
}
#[test]
fn explicit_rescan_waits_for_complete_ready_and_remains_read_only() {
    let f = Fixture::new();
    let path = f.media.join("album.wav");
    wave(&path, "Readonly");
    let before = fs::read(&path).unwrap();
    f.library.rescan_confirmed().unwrap();
    assert_eq!(f.library.status().unwrap().phase, "ready");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(f.check(), Ok(false));
}

#[test]
fn shutdown_does_not_wait_for_a_catalog_projection_mutex() {
    let f = Fixture::new();
    let held = f.library.data.lock().unwrap();
    let library = f.library.clone();
    let (tx, rx) = mpsc::channel();
    let stop = std::thread::spawn(move || {
        library.stop();
        tx.send(()).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(1));
    drop(held);
    stop.join().unwrap();
    assert!(result.is_ok(), "close waited behind unrelated catalog work");
}
#[test]
fn a_signal_between_snapshot_read_and_wait_is_not_lost() {
    let signals = Arc::new(Signals::default());
    let seen = signals.revision().unwrap();
    signals.notify();
    let worker = signals.clone();
    let (tx, rx) = mpsc::channel();
    let wait = std::thread::spawn(move || {
        worker.wait(seen, Duration::from_secs(3)).unwrap();
        tx.send(()).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(1));
    // Release a broken implementation before asserting, so no fixture thread
    // is abandoned if a regression loses the earlier notification.
    signals.notify();
    wait.join().unwrap();
    assert!(result.is_ok());
    assert_ne!(seen, signals.revision().unwrap());
}

#[test]
fn parsed_metadata_change_is_detected_even_when_length_and_mtime_are_restored() {
    let f = Fixture::new();
    let path = f.media.join("same.wav");
    wave(&path, "First");
    assert_eq!(f.check(), Ok(true));
    let before = fs::metadata(&path).unwrap();
    wave(&path, "Other");
    assert_eq!(fs::metadata(&path).unwrap().len(), before.len());
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    drop(file);
    assert_eq!(f.check(), Ok(true));
    assert_eq!(f.check(), Ok(false));
    assert_eq!(
        f.library.compatibility_search("other").unwrap()["tracks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn expected_unc_folder_matches_preferences_authority_without_filesystem_io() {
    let canonical = Path::new(r"\\?\UNC\server\Music\Música");
    assert_eq!(ui_root(canonical), "//server/Music/Música");
}
