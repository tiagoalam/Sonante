//! Focal cases for the initial-only preview admission. No live MPD or audio.
use crate::{
    config_save_operation::{Coordinator, Phase},
    local_import_preview::{self, PreviewError},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg_attr(test, test)]
pub(crate) fn initial_preview_observes_queued_predecessor_and_next_job() {
    let state = Arc::new(Coordinator::new());
    let (accepted, op) = state.reserve().unwrap();
    op.enable_initial_preview("unused", &["Nova".into()])
        .unwrap();
    op.phase(Phase::Indexing);
    let _job = op.preview_job(2);
    assert!(state.begin().is_err());
    assert!(state.reserve().is_err());
    for (job, album) in [(1, "First"), (2, "Second")] {
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let sample = local_import_preview::read_with(&accepted.id, &lease, |command, deadline| {
            assert!(deadline > Instant::now());
            if command == "status" {
                Ok(vec![format!("updating_db: {job}")])
            } else {
                assert!(command.starts_with("find "));
                Ok(vec![
                    format!("file: Nova/{album}/a.flac"),
                    format!("Album: {album}"),
                ])
            }
        })
        .unwrap();
        assert_eq!(sample.albums[0].album.as_deref(), Some(album));
        assert!(sample.provisional && !sample.playable && !sample.complete);
        drop(lease);
        if job == 1 {
            std::thread::sleep(Duration::from_millis(1050));
        }
    }
    op.finish(Ok(()), false);
    assert!(state.reserve_preview(&accepted.id).is_err());
}

#[cfg_attr(test, test)]
pub(crate) fn initial_preview_rejects_mid_page_job_change_and_idle() {
    for statuses in [[Some(1), Some(2)], [Some(2), None], [None, None]] {
        let state = Arc::new(Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_initial_preview("unused", &["Nova".into()])
            .unwrap();
        op.phase(Phase::Indexing);
        let _job = op.preview_job(2);
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let mut n = 0;
        let result = local_import_preview::read_with(&accepted.id, &lease, |command, _| {
            if command == "status" {
                let status = statuses[n];
                n += 1;
                Ok(status
                    .map(|j| vec![format!("updating_db: {j}")])
                    .unwrap_or_default())
            } else {
                Ok(vec!["file: Nova/A/a.flac".into(), "Album: A".into()])
            }
        });
        assert_eq!(result.unwrap_err(), PreviewError::Unavailable);
        assert_eq!(lease.offsets().unwrap(), [0]);
        op.finish(Err("controlled index failure".into()), true);
        assert!(lease.ensure_active().is_err());
        assert!(state.begin().is_err());
    }
}

#[cfg_attr(test, test)]
pub(crate) fn initial_preview_cycles_all_sources_with_eight_bounded_windows() {
    let state = Arc::new(Coordinator::new());
    let (accepted, op) = state.reserve().unwrap();
    let sources = (0..9).map(|n| format!("Source{n}")).collect::<Vec<_>>();
    op.enable_initial_preview("unused", &sources).unwrap();
    op.phase(Phase::Indexing);
    let _job = op.preview_job(2);
    let mut seen = std::collections::BTreeSet::new();
    for batch in 0..2 {
        let lease = state.reserve_preview(&accepted.id).unwrap();
        let mut windows = 0;
        local_import_preview::read_with(&accepted.id, &lease, |command, _| {
            if command == "status" {
                return Ok(vec!["updating_db: 1".into()]);
            }
            windows += 1;
            assert!(command.ends_with("window 0:16"));
            for source in &sources {
                if command.contains(source) {
                    seen.insert(source.clone());
                }
            }
            Ok(vec![])
        })
        .unwrap();
        assert_eq!(windows, 8);
        drop(lease);
        if batch == 0 {
            std::thread::sleep(Duration::from_millis(1050));
        }
    }
    assert_eq!(seen.len(), 9);
    op.finish(Ok(()), false);
}

#[cfg_attr(test, test)]
pub(crate) fn initial_groups_keep_identity_and_return_scoped_cover_references_without_images() {
    let state = Arc::new(Coordinator::new());
    let (accepted, op) = state.reserve().unwrap();
    op.enable_initial_preview("unused", &["A".into(), "B".into()]).unwrap();
    op.phase(Phase::Indexing);
    let _job = op.preview_job(2);
    let lease = state.reserve_preview(&accepted.id).unwrap();
    let sample = local_import_preview::read_with(&accepted.id, &lease, |command, _| {
        if command == "status" { return Ok(vec!["updating_db: 1".into()]); }
        let source = if command.contains("base 'A'") { "A" } else { "B" };
        Ok((0..64).flat_map(|n| [format!("file: {source}/Album{n}/track.flac"), "Album: Same".into(), "AlbumArtist: Artist".into()]).collect())
    }).unwrap();
    assert!(sample.initial && !sample.playable && !sample.complete);
    assert_eq!(sample.albums.len(), 128);
    let keys = sample.albums.iter().map(|r| r.provisional_group_key.clone()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys.len(), 128);
    assert!(sample.albums.iter().all(|r| r.image.is_none() && r.artwork_ref.as_ref().unwrap().starts_with(&format!("cover-v1:{}:", accepted.id))));
    let reference = sample.albums[0].artwork_ref.clone().unwrap();
    let cover = state.reserve_preview_artwork(&accepted.id).unwrap();
    // Cover lease coexists with the bounded page lease; normal mutation stays fenced.
    assert!(state.begin().is_err());
    assert!(matches!(state.reserve_preview_artwork(&accepted.id), Err(PreviewError::Busy)));
    assert_eq!(local_import_preview::read_artwork(&accepted.id, &reference, &cover), Ok(None));
    for invalid in ["cover-v1:old:41".to_string(), format!("cover-v1:{}:2e2e2f41", accepted.id), format!("cover-v1:{}:466f726569676e2f41", accepted.id), format!("cover-v1:{}:gg", accepted.id)] {
        assert_eq!(local_import_preview::read_artwork(&accepted.id, &invalid, &cover), Err(PreviewError::InvalidResponse));
    }
    op.phase(Phase::Validating);
    assert_eq!(local_import_preview::read_artwork(&accepted.id, &reference, &cover), Err(PreviewError::Unavailable));
    op.finish(Ok(()), false);
}
