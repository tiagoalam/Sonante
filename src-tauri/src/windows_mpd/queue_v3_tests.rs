use super::super::queue::SavedQueue;
use super::*;
fn root(label: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("C:\\Fixtures\\{label}"))
    } else {
        PathBuf::from(format!("/fixtures/{label}"))
    }
}
fn track(id: &str, base: &Path) -> Track {
    Track {
        id: id.into(),
        title: format!("Fixture {id}\nsecond line"),
        artist: "Fixture Artist".into(),
        album: "Fixture Album".into(),
        disc: 0,
        number: 2,
        file_name: format!("{id}.flac"),
        catalog_album_id: Some("album".into()),
        path: base.join(format!("{id}.flac")),
        media_root: Some(base.to_path_buf()),
        radio: None,upnp:None,
        remote_source: None,
        remote_connection_id: None,
        plex_image:None,remote_lease: None,
    }
}
fn legacy() -> SavedQueue {
    let one = root("one");
    let two = root("two");
    let tracks = vec![
        track("same", &one),
        track("different", &two),
        track("same", &one),
    ];
    let mut queue = SavedQueue::capture(&one, &tracks, 1, 12.5, 120., 61, Some("album".into()));
    queue.options = super::super::options::PlaybackOptions::from_status(
        &["repeat: 1", "single: oneshot", "random: 0"].map(String::from),
    );
    queue
}
fn legacy_bytes(queue: &SavedQueue, version: u8) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"version":version,"queue":queue})).unwrap()
}
#[cfg(windows)]
#[test]
fn unc_locators_survive_legacy_mixed_and_artwork_codecs_without_becoming_live_transport(){
    for root in [r"\\server\share\Music",r"\\?\UNC\server\share\Music"]{
        let root=PathBuf::from(root);let track=track("own-synthetic",&root);
        let old=SavedQueue::capture(&root,&[track],0,7.5,10.,51,None);
        let logical=decode_legacy(&legacy_bytes(&old,2)).unwrap().unwrap();
        comparable(&old,&logical);
        let bytes=encode(Some(&logical)).unwrap();comparable(&old,&decode(&bytes).unwrap().unwrap());
        let bytes=super::super::queue_v4::encode(Some(&logical),&[None]).unwrap();
        let(restored,images)=super::super::queue_v4::decode(&bytes).unwrap().unwrap();comparable(&old,&restored);assert!(images[0].is_none());
        assert!(!std::str::from_utf8(&bytes).unwrap().contains("http://"));
    }
    for path in [r"\\server\pipe\music",r"\\server\IPC$\music",r"\\.\pipe\music",r"\\?\GLOBALROOT\Device\HarddiskVolume1\music",r"\\server\share\Music\..\secret"]{assert!(!valid_path(Path::new(path)));}
}
fn comparable(old: &SavedQueue, next: &Logical) {
    assert_eq!(old.index, next.index);
    assert_eq!(old.elapsed, next.elapsed);
    assert_eq!(old.duration, next.duration);
    assert_eq!(old.volume, next.volume);
    assert_eq!(old.album_id, next.album_id);
    assert_eq!(old.options, next.options);
    let tracks = old.tracks();
    assert_eq!(tracks.len(), next.tracks.len());
    for (old, next) in tracks.iter().zip(&next.tracks) {
        assert!(Display::local(old) == next.metadata);
        let Source::Local { root, path } = &next.source else {
            panic!("local source changed")
        };
        assert_eq!(path, &old.path);
        assert_eq!(Some(root), old.media_root.as_ref());
    }
}
#[test]
fn actual_v1_v2_queue_projection_preserves_roots_duplicate_occurrences_cursor_tags_and_options() {
    let old = legacy();
    for version in [1, 2] {
        let bytes = legacy_bytes(&old, version);
        let next = decode_legacy(&bytes).unwrap().unwrap();
        comparable(&old, &next);
        let encoded = encode(Some(&next)).unwrap();
        comparable(&old, &decode(&encoded).unwrap().unwrap());
        assert!(next.tracks[0] == next.tracks[2]);
        assert!(next.tracks[0] != next.tracks[1]);
    }
}
#[test]
fn insert_and_edit_use_the_existing_queue_planner_and_match_actual_local_saved_queue() {
    let old = legacy();
    let next = decode_legacy(&legacy_bytes(&old, 2)).unwrap().unwrap();
    for action in [
        Edit::Move { from: 0, to: 2 },
        Edit::Move { from: 2, to: 0 },
        Edit::Next { index: 2 },
        Edit::Remove { index: 0 },
        Edit::Remove { index: 1 },
        Edit::Remove { index: 2 },
    ] {
        let expected = old.edited(action).unwrap().unwrap();
        let actual = next.edited(action).unwrap().unwrap();
        comparable(&expected, &actual);
    }
    for placement in [Placement::Next, Placement::End] {
        let base = root("three");
        let add = vec![track("new", &base), track("new", &base)];
        let selected = super::super::library::Selection {
            root: base.clone(),
            tracks: add.clone(),
            start: 0,
            album_id: Some("other".into()),
        };
        let expected = old.inserted(selected, placement).unwrap();
        let entries = add
            .iter()
            .map(|t| Entry::local(t, &base).unwrap())
            .collect();
        let actual = next.inserted(entries, placement).unwrap();
        comparable(&expected, &actual);
    }
}
#[test]
fn mixed_sources_keep_duplicates_without_connection_ids_tokens_paths_or_live_urls() {
    use crate::audio::MediaLocator;
    let nav = Source::from_locator(&MediaLocator::Navidrome {
        connection_id: "PRIVATE-live-session".into(),
        account_id: Some("a".repeat(64)),
        song_id: "song-a".into(),
    })
    .unwrap();
    let plex = Source::from_locator(&MediaLocator::Plex {
        server_id: "server-one".into(),
        part_key: "/library/parts/1/file.flac".into(),
        rating_key: Some("track-one".into()),
        file_path: Some("/PRIVATE/server-file.flac".into()),
    })
    .unwrap();
    let mut queue = decode_legacy(&legacy_bytes(&legacy(), 2)).unwrap().unwrap();
    queue.tracks[0].source = nav.clone();
    queue.tracks[1].source = plex;
    queue.tracks[2].source = nav;
    let bytes = encode(Some(&queue)).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    for forbidden in [
        "PRIVATE",
        "connection_id",
        "file_path",
        "127.0.0.1",
        "http://",
        "X-Plex-Token",
    ] {
        assert!(!text.contains(forbidden));
    }
    let restored = decode(&bytes).unwrap().unwrap();
    assert!(restored.tracks == queue.tracks);
    assert_eq!(restored.index, 1);
    let moved = restored
        .edited(Edit::Move { from: 2, to: 0 })
        .unwrap()
        .unwrap();
    assert_eq!(moved.index, 2);
    assert_eq!(moved.tracks.len(), 3);
    for locator in [
        MediaLocator::Navidrome {
            connection_id: "old".into(),
            account_id: None,
            song_id: "song".into(),
        },
        MediaLocator::Plex {
            server_id: "server".into(),
            part_key: "/library/parts/1/file.flac?X-Plex-Token=PRIVATE".into(),
            rating_key: Some("1".into()),
            file_path: None,
        },
        MediaLocator::Plex {
            server_id: "server".into(),
            part_key: "/library/parts/1/file.flac".into(),
            rating_key: None,
            file_path: None,
        },
        MediaLocator::Radio {
            station_id: "station".into(),
        },
        MediaLocator::Upnp {
            session_id: "session".into(),
        },
        MediaLocator::Local {
            uri: "http://PRIVATE.invalid/".into(),
        },
    ] {
        assert!(Source::from_locator(&locator).is_err());
    }
}
#[test]
fn current_corruption_and_io_errors_never_invoke_legacy_or_become_an_empty_queue() {
    for current in [
        Err(()),
        Ok(Some(b"broken".to_vec())),
        Ok(Some(
            br#"{"version":4,"content":{"kind":"empty"}}"#.to_vec(),
        )),
        Ok(Some(br#"{"version":3}"#.to_vec())),
        Ok(Some(
            br#"{"version":3,"content":{"kind":"empty","queue":null}}"#.to_vec(),
        )),
    ] {
        assert!(restore(current, || panic!(
            "invalid current data used legacy fallback"
        ))
        .is_err());
    }
    let empty = restore(Ok(Some(encode(None).unwrap())), || {
        panic!("explicit empty current data used legacy fallback")
    })
    .unwrap();
    assert!(empty.queue.is_none());
    assert!(!empty.needs_migration);
    let imported = restore(Ok(None), || Ok(Some(legacy_bytes(&legacy(), 2)))).unwrap();
    assert!(imported.needs_migration);
    assert!(imported.queue.is_some());
    assert!(restore(Ok(None), || Err(())).is_err());
    let absent = restore(Ok(None), || Ok(None)).unwrap();
    assert!(absent.queue.is_none());
    assert!(!absent.needs_migration);
}
#[test]
fn unknown_fields_duplicate_keys_missing_fields_and_nonfinite_or_out_of_bounds_data_fail_closed() {
    let queue = decode_legacy(&legacy_bytes(&legacy(), 2)).unwrap().unwrap();
    let bytes = encode(Some(&queue)).unwrap();
    let valid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for field in ["uri", "stream_url", "connection_id"] {
        let mut bad = valid.clone();
        bad["content"]["queue"]["tracks"][0]["source"][field] = serde_json::json!("PRIVATE");
        assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    for field in ["uri", "thumb"] {
        let mut bad = valid.clone();
        bad["content"]["queue"]["tracks"][0]["metadata"][field] =
            serde_json::json!("http://PRIVATE.invalid/");
        assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut bad = valid.clone();
    bad["content"]["queue"]["options"]["autoplay"] = serde_json::json!(true);
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = valid.clone();
    bad["content"]["queue"]
        .as_object_mut()
        .unwrap()
        .remove("tracks");
    assert!(decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    assert!(decode(br#"{"version":3,"version":3,"content":{"kind":"empty"}}"#).is_err());
    assert!(decode(br#"{"version":3,"content":{"kind":"empty","kind":"empty"}}"#).is_err());
    assert!(decode(&vec![b' '; MAX_BYTES + 1]).is_err());
    let mut bad = queue.clone();
    bad.index = usize::MAX;
    assert!(encode(Some(&bad)).is_err());
    bad = queue.clone();
    bad.elapsed = f64::NAN;
    assert!(encode(Some(&bad)).is_err());
    bad = queue.clone();
    bad.duration = f64::INFINITY;
    assert!(encode(Some(&bad)).is_err());
    bad = queue.clone();
    bad.volume = 101;
    assert!(encode(Some(&bad)).is_err());
    bad = queue.clone();
    bad.tracks = vec![bad.tracks[0].clone(); 1001];
    assert!(encode(Some(&bad)).is_err());
    bad = queue;
    bad.tracks[0].metadata.title = "x".repeat(4097);
    assert!(encode(Some(&bad)).is_err());
}
#[test]
fn legacy_absence_null_authority_root_fallback_and_corruption_are_distinct() {
    let old = legacy();
    let mut value: serde_json::Value = serde_json::from_slice(&legacy_bytes(&old, 2)).unwrap();
    value["queue"]["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("root");
    let projected = decode_legacy(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .unwrap();
    comparable(&old, &projected);
    assert!(decode_legacy(br#"{"version":2,"queue":null}"#)
        .unwrap()
        .is_none());
    assert!(decode_legacy(br#"{"version":2}"#).is_err());
    assert!(decode_legacy(br#"{"version":3,"queue":null}"#).is_err());
    value["queue"]["tracks"][0]["metadata"]["radio"] = serde_json::json!({"stationId":"station"});
    assert!(decode_legacy(&serde_json::to_vec(&value).unwrap()).is_err());
    assert!(decode_legacy(br#"{"version":2,"queue":null,"queue":null}"#).is_err());
    for path in [
        PathBuf::from("relative.flac"),
        root("one").join("..").join("escape.flac"),
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(&legacy_bytes(&old, 2)).unwrap();
        value["queue"]["tracks"][0]["path"] = serde_json::to_value(path).unwrap();
        assert!(decode_legacy(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

#[test]
fn encoding_rejects_wire_budget_overflow_while_writing() {
    let mut queue=decode_legacy(&legacy_bytes(&legacy(),2)).unwrap().unwrap();
    let mut row=queue.tracks[0].clone();row.metadata.title="x".repeat(4096);row.metadata.artist="y".repeat(4096);row.metadata.album="z".repeat(4096);
    queue.tracks=vec![row;400];assert!(queue.validate().is_ok());assert!(encode(Some(&queue)).is_err());
    let mut writer=Writer(Vec::new());use std::io::Write;writer.write_all(&vec![0;MAX_BYTES]).unwrap();assert!(writer.write_all(&[1]).is_err());assert_eq!(writer.0.len(),MAX_BYTES);
}
