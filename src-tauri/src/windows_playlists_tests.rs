use super::*;
use std::{fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut bytes = [0u8; 12];
        getrandom::fill(&mut bytes).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-playlists-{bytes:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn store(&self) -> PlaylistStore {
        PlaylistStore::new(self.0.join("playlists.json"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn item(n: usize) -> Value {
    json!({"media_locator":{"kind":"local","uri":format!("windows/{}/1/{}","a".repeat(64),format!("{n:064x}"))},"metadata":{"title":format!("Track {n}"),"artist":"Artist","album":"Album","duration":123.0}})
}
fn create(store: &PlaylistStore) -> Playlist {
    serde_json::from_value(
        dispatch(
            store,
            "create_playlist_with_items",
            &json!({"name":"Set","items":[item(1),item(1),item(2)]}),
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn a_retired_availability_read_does_not_mark_tracks_missing_or_change_the_store(){
    let fixture=Fixture::new();let store=fixture.store();let playlist=create(&store);let before=fs::read(fixture.0.join("playlists.json")).unwrap();
    let library=crate::windows_mpd::library::Library::new(fixture.0.join("folders.json"));
    assert!(matches!(super::resolve_read(&playlist,&library,||false),Err("index_timeout")));
    assert_eq!(fs::read(fixture.0.join("playlists.json")).unwrap(),before);assert_eq!(store.get(&playlist.id).unwrap().items,playlist.items);library.stop();
}
#[test]
fn original_store_schema_reloads_native_crud_duplicates_and_reorder() {
    let f = Fixture::new();
    let store = f.store();
    let created = create(&store);
    assert_ne!(created.items[0].id, created.items[1].id);
    let ids: Vec<_> = created.items.iter().rev().map(|i| i.id.clone()).collect();
    let updated = dispatch(
        &store,
        "reorder_playlist_items",
        &json!({"playlistId":created.id,"orderedItemIds":ids}),
    )
    .unwrap();
    assert_eq!(updated["items"][0]["id"], created.items[2].id);
    assert_eq!(
        dispatch(&store, "create_playlist", &json!({"name":" set "})),
        Err("playlist_name_duplicate")
    );
    dispatch(
        &store,
        "remove_playlist_item",
        &json!({"playlistId":created.id,"itemId":created.items[0].id}),
    )
    .unwrap();
    let reloaded = f.store().get(&created.id).unwrap();
    assert_eq!(reloaded.items.len(), 2);
    assert!(reloaded.items.iter().any(|i| i.id == created.items[1].id));
    dispatch(
        &store,
        "rename_playlist",
        &json!({"id":created.id,"name":"Changed"}),
    )
    .unwrap();
    assert_eq!(f.store().get(&created.id).unwrap().name, "Changed");
    let single = dispatch(
        &store,
        "add_playlist_item",
        &json!({"playlistId":created.id,"item":item(3)}),
    )
    .unwrap();
    assert_eq!(single["items"].as_array().unwrap().len(), 3);
    dispatch(&store, "delete_playlist", &json!({"id":created.id})).unwrap();
    assert!(f.store().list().unwrap().is_empty());
}
#[test]
fn nonlocal_or_forged_references_fail_atomically_before_writes() {
    let f = Fixture::new();
    let store = f.store();
    let mut remote = item(1);
    remote["media_locator"] = json!({"kind":"plex","server_id":"s","part_key":"/part/1"});
    assert_eq!(
        dispatch(
            &store,
            "create_playlist_with_items",
            &json!({"name":"No","items":[item(1),remote]})
        ),
        Err("PLEX_INVALID")
    );
    assert!(!f.0.join("playlists.json").exists());
    let mut bad = item(1);
    bad["media_locator"]["uri"] = json!("C:/outside.flac");
    assert_eq!(
        dispatch(
            &store,
            "create_playlist_with_items",
            &json!({"name":"No","items":[bad]})
        ),
        Err("invalid_argument")
    );
    assert!(!f.0.join("playlists.json").exists());
    let playlist = create(&store);
    let bytes = fs::read(f.0.join("playlists.json")).unwrap();
    assert!(dispatch(
        &store,
        "reorder_playlist_items",
        &json!({"playlistId":playlist.id,"orderedItemIds":["wrong"]})
    )
    .is_err());
    assert_eq!(fs::read(f.0.join("playlists.json")).unwrap(), bytes);
}
#[test]
fn malformed_store_and_failed_write_preserve_prior_bytes() {
    let f = Fixture::new();
    let store = f.store();
    let path = f.0.join("playlists.json");
    fs::write(&path, b"not JSON").unwrap();
    assert!(dispatch(&store, "create_playlist", &json!({"name":"No"})).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"not JSON");
    fs::remove_file(&path).unwrap();
    let created = create(&store);
    let before = fs::read(&path).unwrap();
    let backup = f.0.join("backup.json");
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(dispatch(
        &store,
        "add_playlist_item",
        &json!({"playlistId":created.id,"item":item(3)})
    )
    .is_err());
    assert_eq!(fs::read(&backup).unwrap(), before);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    assert_eq!(f.store().get(&created.id).unwrap().items.len(), 3);
}
#[test]
fn worker_serializes_concurrent_writes_and_shutdown_rejects_new_work() {
    let f = Fixture::new();
    let worker = Controller::new(f.0.join("playlists.json")).unwrap();
    let jobs: Vec<_> = (0..8)
        .map(|i| {
            let c = worker.clone();
            std::thread::spawn(move || {
                c.call("create_playlist", json!({"name":format!("Set {i}")}))
                    .unwrap()
            })
        })
        .collect();
    for j in jobs {
        j.join().unwrap();
    }
    assert_eq!(
        worker
            .call("list_playlists", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        8
    );
    worker.shutdown().unwrap();
    assert_eq!(
        worker.call("list_playlists", json!({})),
        Err("shutting_down")
    );
    assert_eq!(f.store().list().unwrap().len(), 8);
}
#[test]
fn playback_preparation_uses_occurrence_ids_preserves_duplicates_and_skips_without_deleting() {
    let f = Fixture::new();
    let p = create(&f.store());
    let ready = [true, false, true];
    let plan = prepare(&p, &ready, Some(&p.items[2].id), false, |_| {
        panic!("no shuffle")
    })
    .unwrap();
    assert_eq!(plan.indices, [0, 2]);
    assert_eq!(plan.start, 1);
    assert_eq!(plan.skipped, 1);
    assert_eq!(
        prepare(&p, &ready, Some(&p.items[1].id), false, |_| Ok(0)).unwrap_err(),
        "playlist_selected_item_unavailable"
    );
    assert_eq!(
        prepare(&p, &[false; 3], None, false, |_| Ok(0)).unwrap_err(),
        "playlist_no_playable_items"
    );
    assert_eq!(
        prepare(&p, &[true; 3], Some("absent"), false, |_| Ok(0)).unwrap_err(),
        "windows_playlist_missing"
    );
    let both = prepare(&p, &[true; 3], None, false, |_| Ok(0)).unwrap();
    assert_eq!(both.indices, [0, 1, 2]);
    assert_eq!(f.store().get(&p.id).unwrap().items, p.items);
}
#[test]
fn shuffle_is_ephemeral_and_invalid_combinations_are_rejected() {
    let f = Fixture::new();
    let p = create(&f.store());
    let before = fs::read(f.0.join("playlists.json")).unwrap();
    let plan = prepare(&p, &[true; 3], None, true, |_| Ok(0)).unwrap();
    assert_eq!(plan.indices, [1, 2, 0]);
    assert_eq!(plan.start, 0);
    assert_eq!(fs::read(f.0.join("playlists.json")).unwrap(), before);
    assert_eq!(
        prepare(&p, &[true; 3], Some(&p.items[0].id), true, |_| Ok(0)).unwrap_err(),
        "invalid_argument"
    );
    assert!(prepare(&p, &[true; 3], None, true, |n| Ok(n)).is_err());
    assert_eq!(random_index(0), Err("invalid_argument"));
    assert_eq!(random_index(1), Ok(0));
}
#[test]
fn availability_distinguishes_missing_tracks_from_unavailable_root_and_never_deletes() {
    use crate::windows_mpd::library::Library;
    let f = Fixture::new();
    let music = f.0.join("music");
    fs::create_dir(&music).unwrap();
    let file = music.join("one.flac");
    fs::write(
        &file,
        b"synthetic invalid metadata, catalog filename fallback",
    )
    .unwrap();
    let library = Library::new(f.0.join("folder.json"));
    library.select(music.clone()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while library.status().unwrap().phase == "scanning" {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let album = library.albums("", 0, 10).unwrap().items[0].clone();
    let track = library.tracks(&album.id).unwrap()[0].clone();
    let uri = format!("windows/{}/{}/{}", album.id, track.disc, track.id);
    let mut row = item(1);
    row["media_locator"]["uri"] = json!(uri);
    let p: Playlist = serde_json::from_value(
        dispatch(
            &f.store(),
            "create_playlist_with_items",
            &json!({"name":"Real catalog reference","items":[row]}),
        )
        .unwrap(),
    )
    .unwrap();
    let bytes = fs::read(f.0.join("playlists.json")).unwrap();
    assert!(library.status().unwrap().warnings>0,"fixture intentionally keeps tag warnings");
    let mut unknown=p.clone();unknown.items[0].media_locator=MediaLocator::Local{uri:format!("windows/{}/1/{}","f".repeat(64),"e".repeat(64))};
    assert_eq!(resolve(&unknown,&library).unwrap()[0].status,"unavailable","tag warnings cannot prove an unknown catalog URI missing");
    assert_eq!(resolve(&p, &library).unwrap()[0].status, "available");
    fs::remove_file(&file).unwrap();
    assert_eq!(resolve(&p, &library).unwrap()[0].status, "missing");
    fs::rename(&music, f.0.join("away")).unwrap();
    assert_eq!(resolve(&p, &library).unwrap()[0].status, "unavailable");
    assert_eq!(fs::read(f.0.join("playlists.json")).unwrap(), bytes);
    library.stop();
}
#[test]
fn unknown_local_playlist_uris_are_missing_only_with_a_clean_live_catalog(){
    let f=Fixture::new();let music=f.0.join("empty-music");fs::create_dir(&music).unwrap();let library=crate::windows_mpd::library::Library::new(f.0.join("folders.json"));library.select(music.clone()).unwrap();
    let end=std::time::Instant::now()+Duration::from_secs(3);while library.status().unwrap().phase=="scanning"{assert!(std::time::Instant::now()<end);std::thread::sleep(Duration::from_millis(5));}
    assert_eq!(library.status().unwrap().warnings,0);let store=f.store();let p=create(&store);let before=fs::read(f.0.join("playlists.json")).unwrap();
    assert!(resolve(&p,&library).unwrap().iter().all(|row|row.status=="missing"));
    fs::rename(&music,f.0.join("removed-root")).unwrap();assert!(resolve(&p,&library).unwrap().iter().all(|row|row.status=="unavailable"));
    assert_eq!(fs::read(f.0.join("playlists.json")).unwrap(),before);library.stop();
}
#[test]
fn startup_without_selected_library_marks_items_unavailable_not_missing() {
    let f = Fixture::new();
    let p = create(&f.store());
    let library = crate::windows_mpd::library::Library::new(f.0.join("folder.json"));
    assert!(resolve(&p, &library)
        .unwrap()
        .iter()
        .all(|r| r.status == "unavailable"));
    library.stop();
}

#[cfg(unix)]
#[test]
fn a_retargeted_ancestor_inside_the_root_is_not_a_playback_admitted_playlist_file(){
    use crate::windows_mpd::library::Library;
    let f=Fixture::new();let music=f.0.join("music");let album=music.join("album");fs::create_dir_all(&album).unwrap();
    fs::write(album.join("one.flac"),b"own invalid tag fixture").unwrap();
    let library=Library::new(f.0.join("folder.json"));library.select(music.clone()).unwrap();
    let end=std::time::Instant::now()+Duration::from_secs(3);while library.status().unwrap().phase=="scanning"{assert!(std::time::Instant::now()<end);std::thread::sleep(Duration::from_millis(5));}
    let id=library.albums("",0,10).unwrap().items[0].id.clone();let track=library.tracks(&id).unwrap().remove(0);let mut row=item(1);
    row["media_locator"]["uri"]=json!(format!("windows/{id}/{}/{}",track.disc,track.id));
    let store=f.store();let p:Playlist=serde_json::from_value(dispatch(&store,"create_playlist_with_items",&json!({"name":"Bound identity","items":[row]})).unwrap()).unwrap();let before=fs::read(f.0.join("playlists.json")).unwrap();
    assert_eq!(resolve(&p,&library).unwrap()[0].status,"available");
    let moved=music.join("moved");fs::rename(&album,&moved).unwrap();std::os::unix::fs::symlink(&moved,&album).unwrap();
    assert_eq!(resolve(&p,&library).unwrap()[0].status,"unavailable","a reparse ancestor cannot inherit playback admission from a canonical prefix");
    fs::remove_file(moved.join("one.flac")).unwrap();
    assert_eq!(resolve(&p,&library).unwrap()[0].status,"unavailable","NotFound through a reparse ancestor is not proof that a catalog track was removed");
    assert_eq!(fs::read(f.0.join("playlists.json")).unwrap(),before);library.stop();
}
#[test]
fn only_actual_windows_path_or_file_absence_codes_can_be_missing_candidates(){
    assert!(windows_absence_code(Some(2)));assert!(windows_absence_code(Some(3)));
    for code in [None,Some(0),Some(5),Some(21),Some(32),Some(53),Some(64),Some(67),Some(121),Some(123),Some(995),Some(1201),Some(1231),Some(1232)]{assert!(!windows_absence_code(code));}
    assert!(!definite_absence(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
    assert!(!definite_absence(&std::io::Error::from(std::io::ErrorKind::TimedOut)));
}

#[test]
fn separate_instances_refuse_a_busy_store_and_can_retry_without_losing_data() {
    let f = Fixture::new();
    let path = f.0.join("playlists.json");
    let first = Controller::new(path.clone()).unwrap();
    let second = Controller::new(path.clone()).unwrap();
    let guard = crate::persistence::operation_lock(&path.with_extension("lock")).unwrap();
    assert_eq!(
        second.call("create_playlist", json!({"name":"Deferred"})),
        Err("windows_playlist_busy")
    );
    assert!(!path.exists());
    drop(guard);
    first
        .call("create_playlist", json!({"name":"One"}))
        .unwrap();
    second
        .call("create_playlist", json!({"name":"Two"}))
        .unwrap();
    assert_eq!(
        first
            .call("list_playlists", json!({}))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    first.shutdown().unwrap();
    second.shutdown().unwrap();
    assert_eq!(f.store().list().unwrap().len(), 2);
}

#[test]
fn windows_limits_refuse_large_requests_and_files_without_replacing_data() {
    let f = Fixture::new();
    let store = f.store();
    let created = create(&store);
    let path = f.0.join("playlists.json");
    let old = fs::read(&path).unwrap();
    assert_eq!(
        dispatch(
            &store,
            "add_playlist_items",
            &json!({"playlistId":created.id,"items":vec![item(1);1001]})
        ),
        Err("windows_playlist_limit")
    );
    assert_eq!(fs::read(&path).unwrap(), old);
    let mut huge = item(1);
    huge["metadata"]["title"] = json!("x".repeat(16385));
    assert_eq!(
        dispatch(
            &store,
            "add_playlist_item",
            &json!({"playlistId":created.id,"item":huge})
        ),
        Err("windows_playlist_limit")
    );
    assert_eq!(fs::read(&path).unwrap(), old);
    let big = vec![b' '; 16 * 1024 * 1024 + 1];
    fs::write(&path, &big).unwrap();
    assert!(dispatch(&store, "list_playlists", &json!({})).is_err());
    assert_eq!(fs::metadata(&path).unwrap().len(), big.len() as u64);
}
#[test]fn valid_plex_references_persist_duplicates_without_server_filesystem_hints_and_bad_parts_are_atomic(){
 let f=Fixture::new();let store=f.store();let mut remote=item(1);remote["media_locator"]=json!({"kind":"plex","server_id":"server-one","part_key":"/library/parts/1/file.flac","rating_key":"song-one","file_path":"Z:/server-only.flac"});
 let created=dispatch(&store,"create_playlist_with_items",&json!({"name":"Plex","items":[item(1),remote.clone(),remote.clone()]})).unwrap();let id=created["id"].as_str().unwrap();let rows=f.store().get(id).unwrap().items;assert_eq!(rows.len(),3);assert_ne!(rows[1].id,rows[2].id);
 let encoded=std::fs::read_to_string(f.0.join("playlists.json")).unwrap();assert!(!encoded.contains("server-only"));assert!(!encoded.contains("Z:"));
 for part in ["/library/parts/../secret","/library/parts/%2e%2e/secret","/library/parts/1/file?X-Plex-Token=secret"]{remote["media_locator"]["part_key"]=json!(part);assert!(dispatch(&store,"add_playlist_items",&json!({"playlistId":id,"items":[item(2),remote.clone()]})).is_err());assert_eq!(std::fs::read_to_string(f.0.join("playlists.json")).unwrap(),encoded);}
}
