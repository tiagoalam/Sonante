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
    tx.publish(&|| false).unwrap(); tx.validate_before_commit().unwrap();
    let reader = mirror::ReadonlyMirrorSnapshot::open(&f.profile.join("library")).unwrap();
    assert_eq!(reader.manifest.schema_version, 2); assert!(reader.manifest.sources.is_empty());
    f.db(); f.persist(&next); tx.finish_committed().unwrap(); assert!(!f.marker().exists());
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
