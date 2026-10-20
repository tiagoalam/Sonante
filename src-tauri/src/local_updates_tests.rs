use super::*;
use crate::local_changes::{Batch, Context};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{symlink, MetadataExt},
    sync::atomic::AtomicU64,
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    music: PathBuf,
    library: PathBuf,
    folders: Vec<String>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "sonante-local-updates-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let music = root.join("Music");
        fs::create_dir_all(music.join("Album")).unwrap();
        wav(&music.join("Album/track.wav"), 1);
        let library = root.join("library");
        let staged = local_media::mirror::build_filtered_staging(
            &root.join("library.next"),
            &library,
            &[music.clone()],
            "seed",
        )
        .unwrap();
        fs::rename(staged.staging_path, &library).unwrap();
        let folders = vec![music.to_string_lossy().into_owned()];
        Self {
            root,
            music,
            library,
            folders,
        }
    }
    fn batch(&self, paths: impl IntoIterator<Item = PathBuf>) -> Batch {
        Batch {
            context: Context::capture(&self.library, &self.folders).unwrap(),
            paths: paths.into_iter().collect(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
fn wav(path: &Path, frames: u32) {
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + frames * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(44100u32.to_le_bytes());
    b.extend(88200u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((frames * 2).to_le_bytes());
    b.resize(44 + frames as usize * 2, 0);
    fs::write(path, b).unwrap();
}

#[test]
fn rewritten_and_atomically_replaced_existing_uri_update_once_without_publication() {
    for rename in [false, true] {
        let f = Fixture::new();
        let track = f.music.join("Album/track.wav");
        let batch = f.batch([track.clone()]);
        let before = fs::metadata(&f.library).unwrap();
        let preserved_mtime = fs::metadata(&track).unwrap().modified().unwrap();
        if rename {
            let temp = f.root.join("replacement.wav");
            wav(&temp, 441);
            fs::rename(temp, &track).unwrap();
        } else {
            wav(&track, 441);
        }
        // Even preserving the EXACT previous mtime must still force reread.
        std::fs::File::open(&track)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(preserved_mtime))
            .unwrap();
        assert_eq!(
            fs::metadata(&track).unwrap().modified().unwrap(),
            preserved_mtime
        );
        assert!(batch.context.is_current(&f.library, &f.folders));
        let plan = batch.plan(&f.library).unwrap().unwrap();
        assert!(!plan.needs_mirror);
        assert_eq!(
            plan.refresh,
            local_media::mirror::ManagedRefreshPlan::Targeted {
                directories: vec!["Music/Album".into()],
                changed_files: 1
            }
        );
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Album/track.wav\nOK\n"),
            ("rescan \"Music/Album\"", "updating_db: 7\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Album/track.wav\nOK\n"),
        ]);
        assert_eq!(run_local_content_coverage(&socket, &plan.refresh, plan.coverage.clone(), &f.library, || false).unwrap().outcome, index_coverage::Outcome::Confirmed);
        server.join().unwrap();
        let after = fs::metadata(&f.library).unwrap();
        assert_eq!(
            (before.dev(), before.ino(), before.mtime_nsec()),
            (after.dev(), after.ino(), after.mtime_nsec())
        );
        assert!(!f.root.join("library.next").exists());
    }
}

#[test]
fn no_event_and_ignored_sidecars_are_noops_check_remains_topology_only() {
    let f = Fixture::new();
    assert!(f.batch([]).plan(&f.library).unwrap().is_none());
    let text = f.music.join("notes.txt");
    fs::write(&text, b"notes").unwrap();
    assert!(f.batch([text]).plan(&f.library).unwrap().is_none());
    assert!(!finish_unchanged_managed_generation(
        ManagedMirrorRefreshMode::Check,
        "/must-not-connect"
    )
    .unwrap());
}

#[test]
fn cue_new_removed_and_directory_changes_preserve_structural_policy() {
    let f = Fixture::new();
    let cue = f.music.join("Album/disc.cue");
    fs::write(
        &cue,
        b"FILE \"track.wav\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n",
    )
    .unwrap();
    let plan = f.batch([cue]).plan(&f.library).unwrap().unwrap();
    assert!(plan.needs_mirror);
    let prepared = local_media::mirror::prepare_managed_generation(
        &f.library,
        &f.root.join("library.next"),
        &[f.music.clone()],
        "cue-change",
    )
    .unwrap();
    let local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) = prepared else {
        panic!("CUE must be republished");
    };
    assert!(prepared
        .staging_path
        .join("Music/Album/disc.cue")
        .is_symlink());
    let refresh = local_media::mirror::plan_filesystem_refresh(&f.library, &prepared).unwrap();
    assert!(
        matches!(refresh, local_media::mirror::ManagedRefreshPlan::Targeted { ref directories, .. } if directories == &["Music/Album"])
    );
    local_media::mirror::cleanup_prepared_managed_generation(&prepared).unwrap();
    let new = f.music.join("Album/new.wav");
    wav(&new, 2);
    assert!(
        f.batch([new])
            .plan(&f.library)
            .unwrap()
            .unwrap()
            .needs_mirror
    );
    let removed = f.music.join("Album/track.wav");
    let batch = f.batch([removed.clone()]);
    fs::remove_file(removed).unwrap();
    assert!(batch.plan(&f.library).unwrap().unwrap().needs_mirror);
    let dir = f.music.join("Album");
    assert!(
        f.batch([dir])
            .plan(&f.library)
            .unwrap()
            .unwrap()
            .needs_mirror
    );
}

#[test]
fn dirty_paths_never_escape_sources_and_generation_changes_reject_old_context() {
    let f = Fixture::new();
    let batch = f.batch([f.music.join("../escape.wav")]);
    assert!(batch.plan(&f.library).is_err());
    let batch = f.batch([f.root.join("outside.wav")]);
    assert!(batch.plan(&f.library).is_err());
    let batch = f.batch([f.music.join("Album/track.wav")]);
    let mut manifest = local_media::mirror::read_manifest(&f.library).unwrap();
    manifest.generation_id = "other-generation".into();
    local_media::mirror::write_manifest(&f.library, &manifest).unwrap();
    assert!(!batch.context.is_current(&f.library, &f.folders));
    let link = f.music.join("escape.wav");
    symlink(f.root.join("outside.wav"), &link).unwrap();
    assert!(f.batch([link]).plan(&f.library).unwrap().is_none());
}

#[test]
fn content_merge_preserves_metadata_only_contract_and_coalesces_parent_paths() {
    use local_media::mirror::ManagedRefreshPlan as P;
    let content = P::Targeted {
        directories: vec!["Music/Album".into()],
        changed_files: 1,
    };
    assert_eq!(local_changes::merge(&P::MetadataOnly, &content), content);
    assert_eq!(local_changes::merge(&P::Full, &content), P::Full);
    let merged = local_changes::merge(
        &P::Targeted {
            directories: vec!["Music".into()],
            changed_files: 2,
        },
        &content,
    );
    assert_eq!(
        merged,
        P::Targeted {
            directories: vec!["Music".into()],
            changed_files: 3
        }
    );
}

#[test]
fn content_mpd_failure_and_shutdown_do_not_recover_or_publish() {
    let f = Fixture::new();
    let batch = f.batch([f.music.join("Album/track.wav")]);
    let plan = batch.plan(&f.library).unwrap().unwrap();
    let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
        ("status", "state: stop\nOK\n"),
        ("listall", "file: Music/Album/track.wav\nOK\n"),
        (
            "rescan \"Music/Album\"",
            "ACK [50@0] {update} fixture failure\n",
        ),
    ]);
    assert!(run_local_content_coverage(&socket, &plan.refresh, plan.coverage.clone(), &f.library, || false).is_err());
    server.join().unwrap();
    assert!(
        run_local_content_coverage("/must-not-connect", &plan.refresh, plan.coverage.clone(), &f.library, || true)
            .unwrap_err()
            .starts_with(library_refresh::CANCELLED)
    );
    assert!(!f.root.join("library.next").exists());
}

#[test]
fn only_durable_success_can_activate_prepared_subscriptions() {
    for (result, uncertain, expected) in [
        (Ok(()), false, 1),
        (Err("before rename".into()), false, 0),
        (Err("after rename unconfirmed".into()), true, 0),
        (Ok(()), true, 0),
        (Err("rollback/intent cleanup failed".into()), true, 0),
    ] {
        let calls = Cell::new(0);
        let original = result.clone();
        assert_eq!(
            after_confirmed_local_folders_save(result, uncertain, || calls.set(calls.get() + 1)),
            original
        );
        assert_eq!(calls.get(), expected);
    }
}

#[test]
fn typed_background_admission_distinguishes_busy_uncertain_and_shutdown() {
    use config_save_operation::BackgroundAdmission as A;
    let c = Arc::new(config_save_operation::Coordinator::new());
    let guard = c.begin().unwrap();
    assert_eq!(c.try_begin_background().unwrap_err(), A::Busy);
    drop(guard);
    let (_, op) = c.reserve().unwrap();
    assert_eq!(c.try_begin_background().unwrap_err(), A::Busy);
    op.finish(Ok(()), false);
    assert!(c.try_begin_background().is_ok());
    let (_, op) = c.reserve().unwrap();
    op.finish(Err("uncertain".into()), true);
    assert_eq!(c.try_begin_background().unwrap_err(), A::Blocked);
    let c = config_save_operation::Coordinator::new();
    c.request_shutdown(|| {});
    assert_eq!(c.try_begin_background().unwrap_err(), A::Blocked);
}

#[test]
fn own_tag_refresh_suppresses_only_its_exact_written_revision() {
    let f = Fixture::new();
    let track = f.music.join("Album/track.wav");
    let own = Arc::new(local_own_writes::OwnWrites::default());
    let batch = f.batch([track.clone()]);
    let lease = own
        .begin(&f.library, &["Music/Album/track.wav".into()])
        .unwrap();
    assert!(own.filter(batch.clone()).is_none());
    wav(&track, 2);
    let revision = tags::revision_for_metadata(&fs::metadata(&track).unwrap());
    lease.complete(true, &revision);
    assert!(own.filter(batch.clone()).unwrap().paths.is_empty());
    let replacement = f.root.join("replacement.wav");
    wav(&replacement, 2);
    fs::rename(replacement, &track).unwrap();
    assert_eq!(own.filter(batch.clone()).unwrap().paths, batch.paths);
    let lease = own
        .begin(&f.library, &["Music/Album/track.wav".into()])
        .unwrap();
    lease.complete(false, "");
    assert_eq!(own.filter(batch.clone()).unwrap().paths, batch.paths);
}

#[test]
fn historical_receipt_cannot_claim_an_external_write_that_arrives_before_refresh() {
    let f = Fixture::new();
    let track = f.music.join("Album/track.wav");
    let batch = f.batch([track.clone()]);
    let own = Arc::new(local_own_writes::OwnWrites::default());
    let lease = own
        .begin(&f.library, &["Music/Album/track.wav".into()])
        .unwrap();
    wav(&track, 2);
    let own_revision = tags::revision_for_metadata(&fs::metadata(&track).unwrap());
    wav(&track, 3);
    lease.complete(true, &own_revision);
    assert_eq!(own.filter(batch.clone()).unwrap().paths, batch.paths);
}

#[test]
fn bounded_paths_fail_visibly_instead_of_promoting_to_full_rescan() {
    let f = Fixture::new();
    let mut batch = f.batch([]);
    batch.paths = (0..=local_changes::MAX_DIRTY_PATHS)
        .map(|i| f.music.join(format!("{i}.wav")))
        .collect::<BTreeSet<_>>();
    assert!(batch.plan(&f.library).is_err());
}

#[test]
fn legacy_manifest_fast_content_gate_still_requires_coherent_identity_history() {
    let f = Fixture::new();
    let mut manifest = local_media::mirror::read_manifest(&f.library).unwrap();
    manifest.schema_version = 1;
    manifest.registry_id = None;
    local_media::mirror::write_manifest(&f.library, &manifest).unwrap();
    let batch = f.batch([f.music.join("Album/track.wav")]);
    assert!(batch.context.is_current(&f.library, &f.folders));
    fs::write(f.root.join("local-source-identities.json"), b"{}").unwrap();
    assert!(!batch.context.is_current(&f.library, &f.folders));
}
