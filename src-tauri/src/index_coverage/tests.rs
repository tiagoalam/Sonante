use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    base: PathBuf,
    music: PathBuf,
    live: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "sonante-coverage-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let music = base.join("Music");
        fs::create_dir_all(&music).unwrap();
        Self {
            live: base.join("library"),
            base,
            music,
        }
    }
    fn file(&self, name: &str, bytes: &[u8]) {
        let p = self.music.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn build(&self, id: &str) -> Inventory {
        let prepared = local_media::mirror::build_filtered_staging(
            &self.base.join(format!("stage-{id}")),
            &self.live,
            &[self.music.clone()],
            id,
        )
        .unwrap();
        let inv = prepared.coverage.clone();
        if self.live.exists() {
            fs::rename(&self.live, self.base.join(format!("previous-{id}"))).unwrap();
        }
        fs::rename(prepared.staging_path, &self.live).unwrap();
        inv
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}
fn catalog(files: &[&str]) -> Catalog {
    Catalog::from_records(files.iter().map(|u| format!("file: {u}")).collect()).unwrap()
}
fn ids() -> BTreeSet<String> {
    BTreeSet::from(["Music".into()])
}

#[test]
fn coverage_categories_keep_cue_absence_distinct_from_audio_and_policy() {
    let f = Fixture::new();
    f.file("image.wav", b"fixture");
    f.file("album.cue", b"FILE \"image.wav\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n");
    f.file("other.txt", b"excluded");
    let mut inv = f.build("categories");
    let physical = f.music.join("image.wav");
    inv.observe("Music", Path::new("unusable\n.wav"), &physical, Kind::Audio,
        &fs::metadata(&physical).unwrap()).unwrap();
    let r = compare(&inv, &catalog(&[]), &ids(), &catalog(&["Music/image.wav"])).unwrap();
    let c = &r.sources[0];
    assert_eq!((c.audio_files, c.cataloged_audio), (1, 1));
    assert_eq!((c.audio_unconfirmed, c.cue_groups_unobserved, c.unaddressable), (0, 1, 1));
    assert_eq!((c.omitted, c.policy_excluded), (2, 1));
    let absent = compare(&inv, &catalog(&[]), &ids(), &catalog(&[])).unwrap();
    let c = &absent.sources[0];
    assert_eq!((c.audio_unconfirmed, c.cue_groups_unobserved, c.unaddressable), (1, 1, 1));
    assert_eq!(c.omitted, 3);
    assert_eq!(compare(&inv, &catalog(&["Music/image.wav"]), &ids(), &catalog(&[])).unwrap_err(), LOST);
}
#[test]
fn real_scan_preserves_unknown_new_omissions_and_true_empty() {
    let f = Fixture::new();
    f.file("broken.flac", b"not a decoder proof");
    f.file("outside.xyz", b"ignored");
    let inv = f.build("first");
    let r = compare(&inv, &catalog(&[]), &ids(), &catalog(&[])).unwrap();
    assert_eq!(r.outcome, Outcome::Partial);
    assert_eq!(
        (
            r.sources[0].audio_files,
            r.sources[0].omitted,
            r.sources[0].policy_excluded
        ),
        (1, 1, 1)
    );
    fs::remove_file(f.music.join("broken.flac")).unwrap();
    fs::remove_file(f.music.join("outside.xyz")).unwrap();
    let empty = f.build("empty");
    assert_eq!(
        compare(&empty, &catalog(&[]), &ids(), &catalog(&[]))
            .unwrap()
            .outcome,
        Outcome::Confirmed
    );
}
#[test]
fn real_cue_policy_counts_files_groups_and_virtual_tracks_separately() {
    let f = Fixture::new();
    f.file("image.wav", b"fixture");
    f.file("album.cue",b"FILE \"image.wav\" WAVE\n TRACK 01 AUDIO\n INDEX 01 00:00:00\n TRACK 02 AUDIO\n INDEX 01 00:01:00\n");
    f.file("excluded.cue", b"FILE \"../outside.wav\" WAVE\n");
    let inv = f.build("cue");
    let after = catalog(&[
        "Music/image.wav",
        "Music/album.cue/track0001",
        "Music/album.cue/track0002",
    ]);
    let r = compare(&inv, &catalog(&[]), &ids(), &after).unwrap();
    assert_eq!(r.outcome, Outcome::Confirmed);
    let c = &r.sources[0];
    assert_eq!(
        (
            c.audio_files,
            c.cue_groups,
            c.cataloged_audio,
            c.virtual_tracks,
            c.policy_excluded
        ),
        (1, 1, 1, 2, 1)
    );
    assert_eq!(
        compare(
            &inv,
            &catalog(&[]),
            &ids(),
            &catalog(&["Music/image.wav", "Music/album.cue/track0001"])
        )
        .unwrap()
        .outcome,
        Outcome::Confirmed
    ); // Group presence only.
    assert_eq!(
        compare(
            &inv,
            &after,
            &ids(),
            &catalog(&["Music/image.wav", "Music/album.cue/track0001"])
        )
        .unwrap_err(),
        LOST
    );
}
#[test]
fn rewrite_keeps_uri_protected_but_confirmed_physical_removal_does_not_resurrect() {
    let f = Fixture::new();
    f.file("track.flac", b"old");
    let _ = f.build("old");
    f.file("track.flac", b"new invalid payload");
    let changed = f.build("changed");
    let before = catalog(&["Music/track.flac"]);
    assert_eq!(
        compare(&changed, &before, &ids(), &catalog(&[])).unwrap_err(),
        LOST
    );
    fs::remove_file(f.music.join("track.flac")).unwrap();
    let removed = f.build("removed");
    assert_eq!(
        compare(&removed, &before, &ids(), &catalog(&[]))
            .unwrap()
            .outcome,
        Outcome::Confirmed
    );
    let prior = catalog(&["Retired/track.flac"]);
    let known = BTreeSet::from(["Retired".into()]);
    assert_eq!(
        compare(&removed, &prior, &known, &catalog(&[]))
            .unwrap()
            .outcome,
        Outcome::Confirmed
    );
    assert_eq!(
        compare(&removed, &prior, &ids(), &catalog(&[])).unwrap_err(),
        INVALID
    );
}
#[test]
fn file_root_and_generation_changes_are_fenced_without_media_reads() {
    let f = Fixture::new();
    f.file("a.wav", b"a");
    let inv = f.build("window");
    let check = Check::from_catalog(inv.clone(), catalog(&[]), ids()).unwrap();
    f.file("a.wav", b"changed");
    assert_eq!(inv.verify().unwrap_err(), CHANGED);
    let inv = f.build("window2");
    let mut manifest = local_media::mirror::read_manifest(&f.live).unwrap();
    manifest.generation_id = "another".into();
    fs::write(
        f.live.join(local_media::mirror::MANIFEST_NAME),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert_eq!(
        check.finish_at("/must-not-connect", &f.live).unwrap_err(),
        CHANGED
    );
    fs::rename(&f.music, f.base.join("gone")).unwrap();
    assert_eq!(inv.verify().unwrap_err(), CHANGED);
}
pub(crate) fn assert_partial_executor_contract() {
    for partial in [true, false] {
        let f = Fixture::new();
        f.file("old.wav", b"old");
        f.file("new.flac", b"unknown");
        let inv = f.build("executor");
        let after = if partial {
            "file: Music/old.wav\nOK\n"
        } else {
            "OK\n"
        };
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/old.wav\nOK\n"),
            ("update \"Music\"", "updating_db: 107\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", after),
        ]);
        let check = crate::prepare_index_check(&socket, inv, ids(), || false, None).unwrap();
        let report = std::cell::RefCell::new(None);
        let committed = std::cell::Cell::new(false);
        let restored = std::cell::Cell::new(false);
        let result = crate::complete_published_mirror_refresh(
            || {
                crate::request_refresh_and_inspect(
                    &socket,
                    &local_media::mirror::ManagedRefreshPlan::Targeted {
                        directories: vec!["Music".into()],
                        changed_files: 1,
                    },
                    None,
                    None,
                    || {
                        let (r, _) = check.finish_at(&socket, &f.live)?;
                        *report.borrow_mut() = Some(r);
                        Ok(())
                    },
                )
            },
            || Ok(()),
            || Ok(()),
            || Ok(()),
            |_| {
                restored.set(true);
                Ok(crate::RestorePreviousOutcome::Restored)
            },
            || Ok(()),
            || {
                committed.set(true);
                Ok(())
            },
            || Ok(()),
        );
        server.join().unwrap();
        assert_eq!(committed.get(), partial);
        assert_eq!(restored.get(), !partial);
        let state = std::sync::Arc::new(crate::config_save_operation::Coordinator::new());
        let (_, operation) = state.reserve().unwrap();
        if let Some(r) = report.into_inner() {
            assert_eq!(r.outcome, Outcome::Partial);
            operation.coverage(r);
        }
        operation.finish(result, false);
        let outcome = state.snapshot().unwrap().unwrap();
        assert_eq!(
            outcome.phase,
            if partial {
                crate::config_save_operation::Phase::Completed
            } else {
                crate::config_save_operation::Phase::Failed
            }
        );
        assert_eq!(outcome.coverage.is_some(), partial);
    }
}
#[test]
fn duplicate_extra_and_invalid_catalog_do_not_confirm() {
    for response in [
        "file: Music/a.wav\nfile: Music/a.wav\nOK\n",
        "file: Other/a.wav\nOK\n",
        "file: Music/../bad.wav\nOK\n",
        "file: Music/extra.wav\nOK\n",
    ] {
        let f = Fixture::new();
        f.file("a.wav", b"a");
        let inv = f.build("unsafe");
        let check = Check::from_catalog(inv, catalog(&[]), ids()).unwrap();
        let (socket, server) =
            crate::tests::fake_restorative_rescan_mpd(vec![("listall", response)]);
        assert!(check.finish_at(&socket, &f.live).is_err());
        server.join().unwrap();
    }
}
#[test]
fn metadata_only_has_no_socket_or_inspection_and_session_rejects_stale_results() {
    assert_eq!(
        crate::library_refresh::refresh_inspected(
            "/must-not-connect",
            &local_media::mirror::ManagedRefreshPlan::MetadataOnly,
            true,
            || true,
            false,
            || panic!("no audit")
        )
        .unwrap(),
        crate::library_refresh::Outcome::Completed
    );
    let state = State::default();
    let old = state.begin().unwrap();
    let latest = state.begin().unwrap();
    let f = Fixture::new();
    let inv = f.build("session");
    let report = compare(&inv, &catalog(&[]), &ids(), &catalog(&[])).unwrap();
    assert!(state
        .publish(old, &["private".into()], report.clone())
        .is_none());
    assert!(state.publish(latest, &["private".into()], report).is_some());
    assert!(state.snapshot(&["different".into()]).report.is_none());
    assert!(state.snapshot(&["private".into()]).report.is_some());
}
#[test]
fn pure_comparator_large_inventory_has_no_io_and_reports_cost() {
    let f = Fixture::new();
    f.file("template.wav", b"x");
    let mut inv = f.build("scale");
    let template = inv.selected.values().next().unwrap().clone();
    inv.selected.clear();
    let mut files = BTreeSet::new();
    for i in 0..30000 {
        let uri = format!("Music/Album{:05}/track{i:06}.wav", i / 10);
        inv.selected.insert(uri.clone(), template.clone());
        files.insert(uri);
    }
    let catalog = Catalog {
        records: files.iter().map(|u| format!("file: {u}")).collect(),
        files,
    };
    fs::remove_file(f.music.join("template.wav")).unwrap();
    let started = std::time::Instant::now();
    let r = compare(&inv, &catalog, &ids(), &catalog).unwrap();
    assert_eq!(r.outcome, Outcome::Confirmed);
    let uri_bytes: usize = inv.selected.keys().map(String::len).sum();
    eprintln!("coverage_cost entries=30000 elapsed_us={} uri_bytes={} selected_value_floor_bytes={} no_filesystem_in_compare=true",started.elapsed().as_micros(),uri_bytes,30000*std::mem::size_of::<Selected>());
}
#[test]
fn recovery_requires_previous_catalog_and_shutdown_never_queries_or_cleans() {
    for shutdown in [false, true] {
        let f = Fixture::new();
        f.file("old.wav", b"old");
        let inv = f.build("recovery");
        let check = Check::from_catalog(inv, catalog(&["Music/old.wav"]), ids()).unwrap();
        let cleaned = std::cell::Cell::new(false);
        let socket_and_server = if shutdown {
            None
        } else {
            Some(crate::tests::fake_restorative_rescan_mpd(vec![(
                "listall", "OK\n",
            )]))
        };
        let socket = socket_and_server
            .as_ref()
            .map(|(socket, _)| socket.as_str())
            .unwrap_or("/must-not-connect");
        let error = crate::complete_published_mirror_refresh(
            || Err(LOST.into()),
            || Ok(()),
            || Ok(()),
            || Ok(()),
            |_| {
                crate::covered_index_recovery(
                    if shutdown {
                        crate::RestorePreviousOutcome::SkippedForShutdown
                    } else {
                        crate::RestorePreviousOutcome::Restored
                    },
                    Some(&check),
                    socket,
                )
            },
            || {
                cleaned.set(true);
                Ok(())
            },
            || panic!("no commit"),
            || panic!("no previous cleanup"),
        )
        .unwrap_err();
        assert!(!cleaned.get());
        assert!(error.contains(if shutdown {
            crate::library_refresh::CANCELLED
        } else {
            "LOCAL_INDEX_RECOVERY_UNCONFIRMED"
        }));
        if let Some((_, server)) = socket_and_server {
            server.join().unwrap();
        }
    }
}

#[test]
fn inherited_unknown_invalidates_changed_configuration_but_not_same_scope_partial() {
    let f = Fixture::new();
    f.file("new.flac", b"unknown");
    let inv = f.build("inherited");
    let r = compare(&inv, &catalog(&[]), &ids(), &catalog(&[])).unwrap();
    let state = State::default();
    let t = state.begin().unwrap();
    state.publish(t, &["first".into()], r).unwrap();
    let same = state.begin().unwrap();
    assert!(state.inherit(same, &["first".into()]).is_none());
    assert_eq!(
        state.snapshot(&["first".into()]).report.unwrap().outcome,
        Outcome::Partial
    );
    let changed = state.begin().unwrap();
    let unknown = state.inherit(changed, &["new-config".into()]).unwrap();
    assert!(unknown.report.is_none());
    assert_eq!(unknown.revision, changed);
    assert!(state.snapshot(&["new-config".into()]).report.is_none());
}

#[test]
fn scoped_complete_source_allows_its_removals_and_preserves_other_prior_records() {
    let f=Fixture::new();let other=f.base.join("Other");fs::create_dir(&other).unwrap();
    let keep=other.join("keep.flac");fs::write(&keep,b"keep").unwrap();
    let sources=BTreeMap::from([("Music".into(),f.music.clone()),("Other".into(),other)]);
    let mut inventory=Inventory::new("source-check",&sources,false).unwrap();
    inventory.observe("Other",Path::new("keep.flac"),&keep,Kind::Audio,&fs::metadata(&keep).unwrap()).unwrap();
    let ids=sources.keys().cloned().collect();
    let before=catalog(&["Music/removed.flac","Other/keep.flac"]);
    let after=catalog(&["Other/keep.flac"]);
    assert_eq!(compare(&inventory,&before,&ids,&after).unwrap_err(),LOST);
    inventory.mark_source_complete("Music").unwrap();
    let report=compare(&inventory,&before,&ids,&after).unwrap();
    assert_eq!(report.scope,"paths");
    assert_eq!(report.outcome,Outcome::Confirmed);
    assert_eq!(compare(&inventory,&catalog(&["Music/removed.flac","Other/keep.flac","Other/must-stay.flac"]),&ids,&after).unwrap_err(),LOST);
    assert!(inventory.mark_source_complete("missing").is_err());
}
