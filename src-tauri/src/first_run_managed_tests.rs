//! These cases use real private directories, Lease, identity allocation,
//! managed builder/publication, readonly reader and durable control I/O.
//! The DB bytes simulate a completed MPD write; this is not a live MPD/GUI test.
use crate::{config::AppConfig, first_run_managed::InitialPublication,
    local_media::mirror, profile_lease::Lease, recovery_fs::Dir};
use std::{fs, os::unix::fs::{MetadataExt, PermissionsExt}, path::PathBuf,
    sync::atomic::{AtomicU64, Ordering}};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture { root: PathBuf, profile: PathBuf, media: PathBuf }
impl Fixture {
    fn new() -> Self {
        let parent = std::env::var_os("SONANTE_FIRST_RUN_TEST_ROOT").map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = parent.join(format!("sonante-first-run-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let profile = root.join("profile"); let media = root.join("media");
        fs::create_dir(&profile).unwrap(); fs::create_dir(&media).unwrap();
        fs::set_permissions(&profile, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(media.join("track.wav"), b"focal synthetic content; no playback").unwrap();
        Self { root, profile, media }
    }
    fn configs(&self, prefilled: bool) -> (AppConfig, AppConfig) {
        let mut previous = AppConfig::default();
        let mut next = previous.clone(); next.first_run = false;
        next.local_folders = vec![self.media.to_str().unwrap().into()];
        if prefilled { previous.local_folders = next.local_folders.clone(); self.persist(&previous); }
        (previous, next)
    }
    fn persist(&self, config: &AppConfig) {
        crate::persistence::atomic_write_private_synced(&self.profile.join("config.json"),
            &serde_json::to_vec_pretty(config).unwrap(), "config.json").unwrap();
    }
    fn db(&self) { Dir::open(&self.profile).unwrap().write_new("mpd-v2.db", b"simulated MPD database").unwrap(); }
    fn marker(&self) -> PathBuf { self.profile.join(crate::managed_save_intent::NAME) }
    fn assert_managed(&self) {
        let live = self.profile.join("library");
        let reader = mirror::ReadonlyMirrorSnapshot::open(&live).unwrap();
        assert_eq!(reader.manifest.schema_version, 2);
        let (id, physical) = reader.manifest.sources.iter().next().unwrap();
        assert_eq!(physical, &self.media);
        assert!(fs::symlink_metadata(live.join(id)).unwrap().is_dir());
        assert_eq!(fs::read_link(live.join(id).join("track.wav")).unwrap(), self.media.join("track.wav"));
        reader.revalidate().unwrap();
        assert_eq!(fs::read(self.media.join("track.wav")).unwrap(), b"focal synthetic content; no playback");
    }
}

#[cfg_attr(test, test)]
pub(crate) fn empty_to_selected_folder_publishes_schema2_before_config() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    assert!(mirror::ReadonlyMirrorSnapshot::open(&f.profile.join("library")).is_err());
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-empty", &|| false).unwrap();
    assert!(f.marker().exists()); assert!(!f.profile.join("config.json").exists());
    tx.publish(&|| false).unwrap(); f.assert_managed();
    assert!(!f.profile.join("mpd-v2.db").exists());
    f.db(); f.persist(&next); tx.finish_committed().unwrap();
    assert!(!f.marker().exists()); assert!(!f.profile.join("library.next").exists()); f.assert_managed();
}

#[cfg_attr(test, test)]
pub(crate) fn prefilled_equal_folders_still_publish_schema2() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    assert_eq!(previous.local_folders, next.local_folders);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-prefilled", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); f.assert_managed();
    f.db(); f.persist(&next); tx.finish_committed().unwrap();
    assert!(!f.marker().exists());
}

#[cfg_attr(test, test)]
pub(crate) fn refresh_or_precommit_failure_restores_absence_and_retains_new_db() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-rollback", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); f.db();
    let before = fs::metadata(f.profile.join("mpd-v2.db")).unwrap();
    tx.rollback(|| Ok(())).unwrap();
    for name in ["library", "library.next", "config.json", "mpd-v2.db"] { assert!(!f.profile.join(name).exists()); }
    let kept = fs::metadata(f.profile.join(".first-run-rejected-first-run-rollback-mpd-v2.db")).unwrap();
    assert_eq!((kept.dev(), kept.ino()), (before.dev(), before.ino()));
    assert!(!f.marker().exists());
}

#[cfg_attr(test, test)]
pub(crate) fn failed_stop_preserves_live_publication_and_db() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-stop", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); f.db();
    assert!(tx.rollback(|| Err("stop not confirmed".into())).is_err());
    assert!(f.marker().exists()); assert!(f.profile.join("mpd-v2.db").exists()); f.assert_managed();
}

#[cfg_attr(test, test)]
pub(crate) fn empty_existing_library_is_restored_with_original_inode() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    fs::create_dir(f.profile.join("library")).unwrap();
    let before = fs::metadata(f.profile.join("library")).unwrap();
    let config = fs::read(f.profile.join("config.json")).unwrap();
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-exchange", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); tx.rollback(|| Ok(())).unwrap();
    let after = fs::metadata(f.profile.join("library")).unwrap();
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert_eq!(fs::read(f.profile.join("config.json")).unwrap(), config);
    assert_eq!(fs::read_dir(f.profile.join("library")).unwrap().count(), 0);
}

#[cfg_attr(test, test)]
pub(crate) fn cancellation_before_prepare_has_no_effect_and_before_publish_can_abort() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    assert!(InitialPublication::prepare(&owner, &previous, &next, "first-run-cancel", &|| true).is_err());
    assert!(!f.marker().exists()); assert!(!f.profile.join("library.next").exists());
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-cancel", &|| false).unwrap();
    assert!(tx.publish(&|| true).is_err());
    assert!(!f.profile.join("library").exists()); assert!(f.marker().exists());
    tx.rollback(|| Ok(())).unwrap(); assert!(!f.marker().exists());
}

#[cfg_attr(test, test)]
pub(crate) fn replaced_physical_origin_refuses_publication_and_keeps_proofs() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-origin", &|| false).unwrap();
    fs::rename(&f.media, f.root.join("original-media-retained")).unwrap(); fs::create_dir(&f.media).unwrap();
    assert!(tx.publish(&|| false).is_err());
    assert!(f.marker().exists()); assert!(f.profile.join("library.next").exists());
    assert!(!f.profile.join("library").exists());
}

#[cfg_attr(test, test)]
pub(crate) fn missing_db_or_uncommitted_config_cannot_finish() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-incomplete", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); assert!(tx.finish_committed().is_err());
    f.persist(&next); assert!(tx.finish_committed().is_err());
    assert!(tx.rollback(|| Ok(())).is_err()); // committed config must not be rolled back
    assert!(f.marker().exists()); f.assert_managed();
}

#[cfg_attr(test, test)]
pub(crate) fn retained_library_or_db_is_not_a_fresh_profile() {
    for existing in ["library", "mpd-v2.db"] {
        let f = Fixture::new(); let (previous, next) = f.configs(false);
        if existing == "library" { fs::create_dir(f.profile.join(existing)).unwrap(); fs::write(f.profile.join("library/retained"), b"preserve").unwrap(); }
        else { f.db(); }
        let owner = Lease::acquire(&f.profile).unwrap();
        assert!(InitialPublication::prepare(&owner, &previous, &next, "first-run-existing", &|| false).is_err());
        assert!(!f.marker().exists()); assert!(f.profile.join(existing).exists());
    }
}

#[cfg_attr(test, test)]
pub(crate) fn replaced_journal_cannot_authorize_rollback_or_commit() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-journal", &|| false).unwrap();
    tx.publish(&|| false).unwrap();
    fs::rename(f.marker(), f.profile.join("original-journal-retained")).unwrap();
    fs::write(f.marker(), b"replacement").unwrap();
    assert!(tx.rollback(|| Ok(())).is_err()); assert!(tx.finish_committed().is_err());
    assert_eq!(fs::read(f.marker()).unwrap(), b"replacement"); f.assert_managed();
}

#[cfg_attr(test, test)]
pub(crate) fn no_local_folders_publishes_an_empty_managed_generation() {
    let f = Fixture::new(); let (previous, mut next) = f.configs(false);
    next.local_folders.clear();
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-plex-only", &|| false).unwrap();
    tx.publish(&|| false).unwrap();
    assert_eq!(tx.validate_before_commit().unwrap_err(), "FIRST_RUN_DATABASE_UNCONFIRMED");
    f.db(); tx.validate_before_commit().unwrap();
    let reader = mirror::ReadonlyMirrorSnapshot::open(&f.profile.join("library")).unwrap();
    assert_eq!(reader.manifest.schema_version, 2); assert!(reader.manifest.sources.is_empty());
    f.persist(&next); tx.finish_committed().unwrap(); assert!(!f.marker().exists());
}

#[cfg_attr(test, test)]
pub(crate) fn config_replaced_during_indexing_refuses_commit_and_rollback() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "first-run-config-drift", &|| false).unwrap();
    tx.publish(&|| false).unwrap();
    let mut unexpected = previous.clone(); unexpected.online_artwork_enabled = !unexpected.online_artwork_enabled;
    f.persist(&unexpected);
    assert!(tx.validate_before_commit().is_err());
    assert!(tx.rollback(|| Ok(())).is_err());
    assert_eq!(AppConfig::load_from_path(&f.profile.join("config.json")).unwrap(), unexpected);
    assert!(f.marker().exists()); f.assert_managed();
}

/// Opt-in real engine regression; never opens an audio device or sends play.
/// Supply a private fixture parent, a short private socket directory and the
/// exact runtime under test. Retain profiles/logs for inspection after failures.
fn real_mpd_database_case(deny_database_write: bool) {
    use std::{io::{BufRead, BufReader, Write}, os::unix::net::UnixStream,
        process::{Child, Command, Stdio}, time::{Duration, Instant}};
    struct OwnedMpd(Child);
    impl Drop for OwnedMpd {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                self.0.kill().unwrap(); self.0.wait().unwrap();
            }
        }
    }
    struct RestoreMode(PathBuf);
    impl Drop for RestoreMode {
        fn drop(&mut self) { fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)).unwrap(); }
    }
    fn command(socket: &std::path::Path, command: &str) -> Vec<String> {
        assert!(matches!(command, "rescan" | "status" | "stats" | "kill"));
        let mut connection = UnixStream::connect(socket).unwrap();
        connection.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        connection.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut reader = BufReader::new(connection.try_clone().unwrap());
        let mut line = String::new(); reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("OK MPD "));
        writeln!(connection, "{command}").unwrap();
        let mut lines = Vec::new();
        loop {
            line.clear(); let n = reader.read_line(&mut line).unwrap();
            if n == 0 { assert_eq!(command, "kill"); break; }
            assert!(!line.starts_with("ACK"), "{line}");
            if line == "OK\n" { break; }
            lines.push(line.trim_end().into());
        }
        lines
    }
    let runtime = PathBuf::from(std::env::var_os("SONANTE_FIRST_RUN_MPD_TEST_RUNTIME").expect("explicit runtime required"));
    let sockets = PathBuf::from(std::env::var_os("SONANTE_FIRST_RUN_MPD_SOCKET_ROOT").expect("explicit socket root required"));
    assert!(runtime.is_absolute() && runtime.is_file());
    let socket_metadata = fs::symlink_metadata(&sockets).unwrap();
    assert!(socket_metadata.is_dir() && socket_metadata.uid() == unsafe { libc::getuid() }
        && socket_metadata.mode() & 0o777 == 0o700);
    assert!(std::env::var_os("SONANTE_FIRST_RUN_TEST_ROOT").is_some());
    {
        let f = Fixture::new(); let (previous, mut next) = f.configs(false);
        if deny_database_write {
            next.local_folders = vec![std::env::var("SONANTE_FIRST_RUN_MPD_OVERLAP_MEDIA").expect("explicit larger synthetic fixture required")];
        }
        let mut wav = b"RIFF".to_vec(); wav.extend(1636u32.to_le_bytes());
        wav.extend(b"WAVEfmt "); wav.extend(16u32.to_le_bytes()); wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); wav.extend(8000u32.to_le_bytes()); wav.extend(16000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes()); wav.extend(16u16.to_le_bytes()); wav.extend(b"data");
        wav.extend(1600u32.to_le_bytes()); wav.resize(1644, 0);
        fs::write(f.media.join("track.wav"), wav).unwrap();
        let owner = Lease::acquire(&f.profile).unwrap();
        let mut tx = InitialPublication::prepare(&owner, &previous, &next,
            "first-run-real-regression", &|| false).unwrap();
        tx.publish(&|| false).unwrap();
        let socket = sockets.join(format!("regression-{}-{}.socket", std::process::id(), u8::from(deny_database_write)));
        assert!(!socket.exists() && socket.as_os_str().len() < 104);
        let escaped = |path: &std::path::Path| path.to_str().unwrap().replace('\\', "\\\\").replace('"', "\\\"");
        let config = format!("music_directory \"{}\"\ndatabase {{\n plugin \"simple\"\n path \"{}\"\n hide_playlist_targets \"no\"\n}}\nbind_to_address \"{}\"\nauto_update \"no\"\nfollow_inside_symlinks \"yes\"\nfollow_outside_symlinks \"yes\"\nzeroconf_enabled \"no\"\naudio_output {{\n type \"null\"\n name \"metadata only\"\n mixer_type \"none\"\n}}\n", escaped(&f.profile.join("library")), escaped(&f.profile.join("mpd-v2.db")), escaped(&socket));
        let conf = f.root.join("mpd.conf"); fs::write(&conf, config).unwrap();
        let log_path = f.root.join("mpd.stderr"); let log = fs::File::create(&log_path).unwrap();
        let restore = RestoreMode(f.profile.clone());
        let mut mpd = OwnedMpd(Command::new(&runtime).args(["--no-daemon", "--stderr"]).arg(&conf)
            .env_clear().env("PATH", "/usr/bin:/bin").env("HOME", &f.root).env("LANG", "C.UTF-8")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(log)).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&socket).is_err() {
            assert!(mpd.0.try_wait().unwrap().is_none(), "MPD startup failed");
            assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10));
        }
        if deny_database_write {
            assert!(!f.profile.join("mpd-v2.db").exists(), "startup must still be indexing the larger fixture");
            fs::set_permissions(&f.profile, fs::Permissions::from_mode(0o500)).unwrap();
        }
        assert!(command(&socket, "rescan").iter().any(|s| s.starts_with("updating_db:")));
        while command(&socket, "status").iter().any(|s| s.starts_with("updating_db:")) {
            assert!(Instant::now() < deadline); std::thread::sleep(Duration::from_millis(10));
        }
        let songs = if deny_database_write { 512 } else { 1 };
        assert!(command(&socket, "stats").iter().any(|s| s == &format!("songs: {songs}")));
        drop(restore);
        if deny_database_write {
            // Real MPD accepts/completes the scan and exposes its in-memory
            // catalogue, despite its real filesystem save failing.
            assert!(!f.profile.join("mpd-v2.db").exists());
            assert!(fs::read_to_string(&log_path).unwrap().contains("Failed to save database"));
            assert_eq!(tx.validate_before_commit().unwrap_err(), "FIRST_RUN_DATABASE_UNCONFIRMED");
            assert!(!f.profile.join("config.json").exists());
            tx.rollback(|| { command(&socket, "kill"); assert!(mpd.0.wait().unwrap().success()); Ok(()) }).unwrap();
            assert!(!f.marker().exists() && !f.profile.join("library").exists());
        } else {
            tx.validate_before_commit().unwrap(); f.persist(&next); tx.finish_committed().unwrap();
            command(&socket, "kill"); assert!(mpd.0.wait().unwrap().success());
            assert!(f.profile.join("mpd-v2.db").is_file() && !f.marker().exists());
        }
        fs::write(f.root.join("REAL-MPD-RESULT.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "database_write_deliberately_denied": deny_database_write,
            "real_catalogue_songs": songs, "no_playback_or_GUI": true, "own_MPD_exit": 0,
            "config_committed": !deny_database_write, "marker_present": f.marker().exists(),
        })).unwrap()).unwrap();
    }
}

#[cfg_attr(test, test)]
#[cfg_attr(test, ignore = "requires explicitly admitted real MPD and isolated fixture/socket roots")]
pub(crate) fn real_mpd_saved_database_can_commit_initial_config() { real_mpd_database_case(false); }

#[cfg_attr(test, test)]
#[cfg_attr(test, ignore = "requires explicitly admitted real MPD and isolated fixture/socket roots")]
pub(crate) fn real_mpd_failed_database_save_prevents_initial_config_commit() { real_mpd_database_case(true); }

#[cfg_attr(test, test)]
pub(crate) fn preferences_enter_before_database_and_resume_the_same_generation() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "initial-background-resume", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); f.persist(&next);
    tx.prepare_background().unwrap();
    let library = Dir::open(&f.profile.join("library")).unwrap().id();
    assert!(!f.profile.join("mpd-v2.db").exists());
    assert_eq!(crate::first_run_managed::PendingPreparation::config(&f.profile).unwrap(), Some(next.clone()));
    drop(tx); drop(owner); // Simulate shutdown/reopen; no old live owner survives.
    let owner = Lease::acquire(&f.profile).unwrap();
    let pending = crate::first_run_managed::PendingPreparation::open(&owner).unwrap();
    let inventory = pending.snapshot().unwrap().coverage_inventory().unwrap(); inventory.verify().unwrap();
    assert!(pending.finish(&owner).is_err()); // MPD idle without a durable DB is insufficient.
    assert_eq!(AppConfig::load_from_path(&f.profile.join("config.json")).unwrap(), next);
    assert!(f.marker().exists()); assert!(f.profile.join(crate::first_run_managed::PENDING).exists());
    f.db(); pending.finish(&owner).unwrap();
    assert_eq!(Dir::open(&f.profile.join("library")).unwrap().id(), library);
    assert!(!f.marker().exists()); assert!(!f.profile.join(crate::first_run_managed::PENDING).exists());
    assert!(!f.profile.join("library.next").exists()); f.assert_managed();
}

#[cfg_attr(test, test)]
pub(crate) fn interrupted_preferences_and_changed_config_do_not_admit_resume() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "initial-uncommitted", &|| false).unwrap();
    tx.publish(&|| false).unwrap();
    assert!(tx.prepare_background().is_err());
    assert!(!f.profile.join(crate::first_run_managed::PENDING).exists());
    assert!(crate::first_run_managed::PendingPreparation::open(&owner).is_err());
    f.persist(&next); tx.prepare_background().unwrap();
    crate::first_run_managed::PendingPreparation::open(&owner).unwrap();
    let mut changed = next.clone(); changed.online_artwork_enabled = !changed.online_artwork_enabled;
    f.persist(&changed);
    assert!(crate::first_run_managed::PendingPreparation::open(&owner).is_err());
    assert!(f.marker().exists()); f.assert_managed();
}

#[cfg_attr(test, test)]
pub(crate) fn resumed_cleanup_never_removes_foreign_contents() {
    let f = Fixture::new(); let (previous, next) = f.configs(true);
    fs::create_dir(f.profile.join("library")).unwrap();
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "initial-cleanup", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); f.persist(&next); tx.prepare_background().unwrap(); f.db();
    let pending = crate::first_run_managed::PendingPreparation::open(&owner).unwrap();
    fs::write(f.profile.join("library.next/foreign"), b"preserve").unwrap();
    assert!(pending.finish(&owner).is_err());
    assert_eq!(fs::read(f.profile.join("library.next/foreign")).unwrap(), b"preserve");
    assert!(f.marker().exists());
}

#[cfg_attr(test, test)]
pub(crate) fn resumed_cleanup_can_finish_after_original_marker_was_unlinked() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "initial-cleanup-resume", &|| false).unwrap();
    tx.publish(&|| false).unwrap(); f.persist(&next); tx.prepare_background().unwrap(); f.db();
    fs::remove_file(f.marker()).unwrap(); Dir::open(&f.profile).unwrap().sync().unwrap();
    drop(tx); drop(owner);
    let owner = Lease::acquire(&f.profile).unwrap();
    crate::first_run_managed::PendingPreparation::open(&owner).unwrap().finish(&owner).unwrap();
    f.assert_managed(); assert!(!f.profile.join(crate::first_run_managed::PENDING).exists());
}

#[cfg_attr(test, test)]
pub(crate) fn pending_generation_tampering_and_database_write_failure_keep_preferences() {
    let f = Fixture::new(); let (previous, next) = f.configs(false);
    let owner = Lease::acquire(&f.profile).unwrap();
    let mut tx = InitialPublication::prepare(&owner, &previous, &next, "initial-db-failure", &||false).unwrap();
    tx.publish(&||false).unwrap(); f.persist(&next); tx.prepare_background().unwrap();
    let pending = crate::first_run_managed::PendingPreparation::open(&owner).unwrap();
    fs::create_dir(f.profile.join("mpd-v2.db")).unwrap();
    assert!(fs::write(f.profile.join("mpd-v2.db"), b"controlled DB write failure").is_err());
    assert!(pending.finish(&owner).is_err());
    assert_eq!(AppConfig::load_from_path(&f.profile.join("config.json")).unwrap(), next);
    let snapshot = pending.snapshot().unwrap();
    let id = snapshot.manifest.sources.keys().next().unwrap();
    let link = f.profile.join("library").join(id).join("extra.wav");
    std::os::unix::fs::symlink(f.media.join("track.wav"), &link).unwrap();
    assert!(crate::first_run_managed::PendingPreparation::open(&owner).is_err());
    assert!(link.exists()); assert!(f.marker().exists());
}
