// These fixtures call the actual mirror publisher, intent writer, durable-set
// protocol, filesystem guards, and executor. DB validator/legacy process proof
// are injected below; separate opt-in tests exercise an owned real null daemon.
fn recovery_fixture_catalog(fixture: &Fixture, next: bool) -> std::collections::HashSet<String> {
    let live = mirror::read_manifest(&fixture.config_dir.join(if next {
        "library.next"
    } else {
        "library"
    }))
    .unwrap();
    live.sources
        .keys()
        .map(|id| format!("file: {id}/track.flac"))
        .collect()
}
fn recovery_fake_validator<'a>(
    old: &'a std::collections::HashSet<String>,
    next: &'a std::collections::HashSet<String>,
) -> impl Fn(&[u8], &dyn Fn() -> bool) -> Result<std::collections::HashSet<String>, String> + 'a {
    move |bytes, cancel| {
        crate::recovery_fs::check(cancel)?;
        match bytes {
            b"fixture catalog" => Ok(old.clone()),
            b"next catalog" => Ok(next.clone()),
            _ => Err("synthetic invalid DB".into()),
        }
    }
}
#[test]
fn recovery_execution_capture_validates_db_and_retains_full_previous_catalog() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    assert!(
        crate::recovery_set::capture_before(&owner, Some(&next), &|| false, &validator).is_err()
    );
    assert!(!f.config_dir.join(crate::recovery_set::PENDING).exists());
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert_eq!(plan.previous_tracks, old.len());
    f.publish(&intent);
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    f.persist(&f.next).unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert_eq!(plan.choice, crate::recovery_set::Choice::RestorePrevious);
    let result =
        crate::recovery_set::execute_fixture(&owner, &plan.id, &|| false, &validator, &|_| Ok(()))
            .unwrap();
    assert!(result.completed);
    assert_eq!(
        fs::read(f.config_dir.join("mpd-v2.db")).unwrap(),
        b"fixture catalog"
    );
    assert_eq!(
        crate::config::AppConfig::load_from_path(&f.config_dir.join("config.json")).unwrap(),
        f.previous
    );
    check_startup(&f.config_dir).unwrap();
    assert!(f.config_dir.join(crate::recovery_set::LAST).is_dir());
    assert!(!f.config_dir.join("library.next").exists());
}
#[test]
fn recovery_execution_resumes_real_effect_boundaries_without_replaying_completed_effects() {
    for point in [
        "backup_complete",
        "applying",
        "exchanged",
        "staging_retained",
        "config.json",
        "mpd-v2.db",
        "validating",
        "committed",
        "intent_retained",
        "finalized",
    ] {
        let f = Fixture::new();
        let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
        let intent = f.intent();
        let old = recovery_fixture_catalog(&f, false);
        let next = recovery_fixture_catalog(&f, true);
        let validator = recovery_fake_validator(&old, &next);
        crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
        f.publish(&intent);
        fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
        f.persist(&f.next).unwrap();
        let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
        let seen = Cell::new(false);
        let failure = crate::recovery_set::execute_fixture(
            &owner,
            &plan.id,
            &|| false,
            &validator,
            &|phase| {
                if phase == point && !seen.replace(true) {
                    Err("simulated crash".into())
                } else {
                    Ok(())
                }
            },
        );
        // replace(true) must only happen at the selected boundary.
        if !seen.get() {
            panic!("boundary {point} was not reached: {:?}", failure.err())
        }
        assert!(failure.is_err(), "{point}");
        assert!(check_startup(&f.config_dir).is_err());
        let retry = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
        crate::recovery_set::execute_fixture(&owner, &retry.id, &|| false, &validator, &|_| Ok(()))
            .unwrap_or_else(|e| panic!("{point}: {e}"));
        check_startup(&f.config_dir).unwrap();
        assert_eq!(
            fs::read(f.config_dir.join("mpd-v2.db")).unwrap(),
            b"fixture catalog"
        );
    }
}
#[test]
fn recovery_execution_plan_rejects_file_replacement_partial_catalog_and_unknown_owner() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let _intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    let config = f.config_dir.join("config.json");
    let bytes = fs::read(&config).unwrap();
    fs::rename(&config, f.config_dir.join("old-config")).unwrap();
    crate::persistence::atomic_write_private_synced(&config, &bytes, "config.json").unwrap();
    assert!(crate::recovery_set::execute_fixture(
        &owner,
        &plan.id,
        &|| false,
        &validator,
        &|_| Ok(())
    )
    .is_err());
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert!(crate::recovery_set::execute_fixture(
        &owner,
        &plan.id,
        &|| false,
        &|_, _| Ok(Default::default()),
        &|_| Ok(())
    )
    .is_err());
    assert!(f.config_dir.join(NAME).exists());
    assert!(f.config_dir.join("library.next").exists());
}
#[test]
fn recovery_execution_finishes_confirmed_partial_indexing_without_selecting_previous() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let mut next = recovery_fixture_catalog(&f, true);
    next.retain(|r| old.contains(r));
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    let published = f.publish(&intent);
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    f.persist(&f.next).unwrap();
    crate::recovery_set::confirm_next(&owner, Some(&next), &|| false, &validator).unwrap();
    // A crash in cleanup may have removed individual entries of the old tree.
    let previous = mirror::read_manifest(&f.config_dir.join("library.next")).unwrap();
    let id = previous.sources.keys().next().unwrap();
    fs::remove_file(
        f.config_dir
            .join("library.next")
            .join(id)
            .join("track.flac"),
    )
    .unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert_eq!(plan.choice, crate::recovery_set::Choice::FinishConfirmed);
    crate::recovery_set::execute_fixture(&owner, &plan.id, &|| false, &validator, &|_| Ok(()))
        .unwrap();
    check_startup(&f.config_dir).unwrap();
    assert_eq!(
        fs::read(f.config_dir.join("mpd-v2.db")).unwrap(),
        b"next catalog"
    );
    drop(published);
}
#[test]
fn recovery_execution_save_emits_and_retires_its_complete_set() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    let publication = f.publish(&intent);
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    f.persist(&f.next).unwrap();
    crate::recovery_set::confirm_next(&owner, Some(&next), &|| false, &validator).unwrap();
    publication.cleanup_previous().unwrap();
    intent
        .complete_transaction(Ok(()), Some(Completed::Committed))
        .unwrap();
    assert!(
        check_startup(&f.config_dir).is_err(),
        "own pending set must still guard startup after legacy intent removal"
    );
    crate::recovery_set::finish_save(&owner, &|| false, &validator).unwrap();
    check_startup(&f.config_dir).unwrap();
}
#[test]
fn recovery_execution_rejects_incomplete_tree_and_cancellation_without_cleanup() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    f.publish(&intent);
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert!(
        crate::recovery_set::execute_fixture(&owner, &plan.id, &|| true, &validator, &|_| Ok(()))
            .is_err()
    );
    let previous = mirror::read_manifest(&f.config_dir.join("library.next")).unwrap();
    let id = previous.sources.keys().next().unwrap();
    fs::remove_file(
        f.config_dir
            .join("library.next")
            .join(id)
            .join("track.flac"),
    )
    .unwrap();
    assert!(crate::recovery_set::analyze(owner.root(), &|| false).is_err());
    assert!(f.config_dir.join(NAME).exists());
}
#[test]
fn recovery_execution_keeps_later_ledger_reservations_favorites_and_playlist_bytes() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    let extra = f.root.join("Extra");
    fs::create_dir(&extra).unwrap();
    let current = mirror::read_manifest(&f.config_dir.join("library")).unwrap();
    let current = current
        .sources
        .into_iter()
        .map(|(id, p)| (id, Some(p)))
        .collect();
    crate::source_identity::reserve(
        &f.config_dir.join("library"),
        &[f.root.join("Old"), extra],
        &current,
        None,
    )
    .unwrap();
    crate::persistence::atomic_write_private_synced(
        &f.config_dir.join("favorites.json"),
        b"[\"new favorite\"]",
        "favorites.json",
    )
    .unwrap();
    crate::persistence::atomic_write_private_synced(
        &f.config_dir.join("playlists.json"),
        b"{\"new\":true}",
        "playlists.json",
    )
    .unwrap();
    let ledger = fs::read(f.config_dir.join("local-source-identities.json")).unwrap();
    let anchor = fs::metadata(f.config_dir.join("local-source-identities.lock"))
        .unwrap()
        .ino();
    f.publish(&intent);
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    crate::recovery_set::execute_fixture(&owner, &plan.id, &|| false, &validator, &|_| Ok(()))
        .unwrap();
    assert_eq!(
        fs::read(f.config_dir.join("local-source-identities.json")).unwrap(),
        ledger
    );
    assert_eq!(
        fs::metadata(f.config_dir.join("local-source-identities.lock"))
            .unwrap()
            .ino(),
        anchor
    );
    assert_eq!(
        fs::read(f.config_dir.join("favorites.json")).unwrap(),
        b"[\"new favorite\"]"
    );
    assert_eq!(
        fs::read(f.config_dir.join("playlists.json")).unwrap(),
        b"{\"new\":true}"
    );
}
#[test]
fn recovery_execution_v2_restores_occurrences_and_logical_cursor_without_play_or_seek() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let (tracks, mut queue, mut playback) = backup_inputs(PlaybackState::Playing);
    let mut tracks = tracks;
    let id = mirror::read_manifest(&f.config_dir.join("library"))
        .unwrap()
        .sources
        .keys()
        .next()
        .unwrap()
        .clone();
    for track in &mut tracks {
        track.uri = format!("{id}/track.flac");
        track.media_locator = Some(MediaLocator::Local {
            uri: track.uri.clone(),
        });
    }
    for entry in &mut queue.entries {
        entry.uri = format!("{id}/track.flac");
    }
    playback.uris = tracks.iter().map(|t| t.uri.clone()).collect();
    let backup = QueueBackup::new(&tracks, &playback.uris, &queue, &playback).unwrap();
    let mut intent = create_backup(&f, &backup).unwrap();
    let cache = backup.recovery_cache().unwrap();
    crate::persistence::atomic_write_private_synced(
        &f.config_dir.join("queue_cache.json"),
        &cache,
        "queue_cache.json",
    )
    .unwrap();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    intent
        .persist_queue_cache(br#"{"version":2,"tracks":[]}"#)
        .unwrap();
    f.publish(&intent);
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    crate::recovery_set::execute_fixture(&owner, &plan.id, &|| false, &validator, &|_| Ok(()))
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(f.config_dir.join("queue_cache.json")).unwrap()).unwrap();
    assert_eq!(value["tracks"].as_array().unwrap().len(), 2);
    let resume: crate::audio::PlaybackResumeSnapshot =
        serde_json::from_value(value["resume"].clone()).unwrap();
    assert_eq!(resume.state, PlaybackState::Paused);
    assert_eq!(resume.queue_index, playback.current_index.unwrap());
    assert_eq!(resume.elapsed, playback.elapsed);
}
#[test]
fn recovery_execution_partial_capture_is_not_a_backup_or_an_executable_plan() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let _intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    assert!(crate::recovery_set::capture_before(
        &owner,
        Some(&old),
        &|| f.config_dir.join(crate::recovery_set::PENDING).exists(),
        &validator
    )
    .is_err());
    assert!(crate::recovery_set::analyze(owner.root(), &|| false).is_err());
    assert!(check_startup(&f.config_dir).is_err());
    assert!(!f
        .config_dir
        .join(crate::recovery_set::PENDING)
        .join("set.json")
        .exists());
    assert_eq!(
        fs::read(f.config_dir.join("mpd-v2.db")).unwrap(),
        b"fixture catalog"
    );
}
#[test]
fn recovery_execution_rejects_tampered_saved_controls_and_post_commit_changes() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    f.publish(&intent);
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    f.persist(&f.next).unwrap();
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert!(crate::recovery_set::execute_fixture(
        &owner,
        &plan.id,
        &|| false,
        &validator,
        &|stage| if stage == "committed" {
            Err("crash".into())
        } else {
            Ok(())
        }
    )
    .is_err());
    fs::write(f.config_dir.join("mpd-v2.db"), b"next catalog").unwrap();
    let resume = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    assert!(crate::recovery_set::execute_fixture(
        &owner,
        &resume.id,
        &|| false,
        &validator,
        &|_| Ok(())
    )
    .is_err());
    fs::write(
        f.config_dir
            .join(crate::recovery_set::PENDING)
            .join("files/config.json"),
        b"tampered",
    )
    .unwrap();
    assert!(crate::recovery_set::analyze(owner.root(), &|| false).is_err());
    assert!(f.config_dir.join(NAME).exists());
}
#[test]
fn recovery_execution_refused_capture_aborts_only_its_unpublished_draft() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    assert!(
        crate::recovery_set::capture_before(&owner, Some(&next), &|| false, &validator).is_err()
    );
    intent.abort_unpublished(&f.prepared).unwrap();
    check_startup(&f.config_dir).unwrap();
    assert_eq!(
        fs::read(f.config_dir.join("mpd-v2.db")).unwrap(),
        b"fixture catalog"
    );
    assert!(!f.config_dir.join("library.next").exists());
}
#[test]
fn recovery_execution_missing_seal_never_authorizes_a_complete_bundle() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let _intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    fs::remove_file(f.config_dir.join(crate::recovery_set::SEAL)).unwrap();
    assert!(crate::recovery_set::analyze(owner.root(), &|| false).is_err());
    assert!(check_startup(&f.config_dir).is_err());
}
#[test]
fn recovery_execution_cancellation_after_exchange_keeps_guard_and_allows_explicit_resume() {
    let f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    let intent = f.intent();
    let old = recovery_fixture_catalog(&f, false);
    let next = recovery_fixture_catalog(&f, true);
    let validator = recovery_fake_validator(&old, &next);
    crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
    f.publish(&intent);
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    let cancelled = Cell::new(false);
    assert!(crate::recovery_set::execute_fixture(
        &owner,
        &plan.id,
        &|| cancelled.get(),
        &validator,
        &|phase| {
            if phase == "exchanged" {
                cancelled.set(true)
            }
            Ok(())
        }
    )
    .is_err());
    assert!(check_startup(&f.config_dir).is_err());
    let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
    crate::recovery_set::execute_fixture(&owner, &plan.id, &|| false, &validator, &|_| Ok(()))
        .unwrap();
    check_startup(&f.config_dir).unwrap();
}
#[test]
fn recovery_execution_rotates_only_a_proven_finalized_last_backup() {
    let mut f = Fixture::new();
    let owner = crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();
    for attempt in 0..2 {
        let intent = f.intent();
        let old = recovery_fixture_catalog(&f, false);
        let next = recovery_fixture_catalog(&f, true);
        let validator = recovery_fake_validator(&old, &next);
        crate::recovery_set::capture_before(&owner, Some(&old), &|| false, &validator).unwrap();
        f.publish(&intent);
        let plan = crate::recovery_set::analyze(owner.root(), &|| false).unwrap();
        if attempt==1 {
            let cancel=|| f.config_dir.join(".sonante-recovery.discard.json").exists()&&!f.config_dir.join(crate::recovery_set::LAST).join("files/config.json").exists();
            assert!(crate::recovery_set::execute_fixture(&owner,&plan.id,&cancel,&validator,&|_|Ok(())).is_err());
            assert!(check_startup(&f.config_dir).is_err());
        }
        crate::recovery_set::execute_fixture(&owner, &plan.id, &|| false, &validator, &|phase| {
            eprintln!("rotation {attempt}: {phase}");
            Ok(())
        })
        .unwrap();
        check_startup(&f.config_dir).unwrap();
        assert!(f.config_dir.join(crate::recovery_set::LAST).is_dir());
        if attempt == 0 {
            let folders = f
                .next
                .local_folders
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            let ManagedGenerationPreparation::Prepared(prepared) =
                mirror::prepare_managed_generation(
                    &f.config_dir.join("library"),
                    &f.config_dir.join("library.next"),
                    &folders,
                    "retry",
                )
                .unwrap()
            else {
                panic!("fixture staging")
            };
            f.prepared = prepared;
        }
    }
    assert!(!f.config_dir.join(".sonante-recovery.discard.json").exists());
}
#[test]
fn recovery_execution_source_removal_restores_the_entire_previous_catalog(){
 let mut f=Fixture::new();let published=mirror::publish_prepared_managed_generation(&f.config_dir.join("library"),&f.prepared).unwrap();published.cleanup_previous().unwrap();
 let removed=f.previous.clone();f.previous=f.next.clone();f.next=removed;f.persist(&f.previous).unwrap();
 let folders=f.next.local_folders.iter().map(PathBuf::from).collect::<Vec<_>>();let ManagedGenerationPreparation::Prepared(prepared)=mirror::prepare_managed_generation(&f.config_dir.join("library"),&f.config_dir.join("library.next"),&folders,"remove").unwrap()else{panic!("fixture")};f.prepared=prepared;
 let owner=crate::profile_lease::Lease::acquire(&f.config_dir).unwrap();let intent=f.intent();let old=recovery_fixture_catalog(&f,false);let next=recovery_fixture_catalog(&f,true);assert_eq!(old.len(),2);assert_eq!(next.len(),1);let validator=recovery_fake_validator(&old,&next);
 crate::recovery_set::capture_before(&owner,Some(&old),&||false,&validator).unwrap();f.publish(&intent);fs::write(f.config_dir.join("mpd-v2.db"),b"next catalog").unwrap();f.persist(&f.next).unwrap();
 let plan=crate::recovery_set::analyze(owner.root(),&||false).unwrap();assert_eq!(plan.previous_tracks,2);crate::recovery_set::execute_fixture(&owner,&plan.id,&||false,&validator,&|_|Ok(())).unwrap();
 assert_eq!(recovery_fixture_catalog(&f,false),old);assert_eq!(fs::read(f.config_dir.join("mpd-v2.db")).unwrap(),b"fixture catalog");check_startup(&f.config_dir).unwrap();
}
