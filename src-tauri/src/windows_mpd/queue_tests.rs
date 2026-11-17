use super::*;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let mut r = [0; 12];
        getrandom::fill(&mut r).unwrap();
        let p = std::env::temp_dir().join(format!("sonante-queue-{r:x?}"));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap()
    }
}
fn queue(root: &Path) -> SavedQueue {
    let tracks = (0..3)
        .map(|i| Track {
            id: format!("id{i}"),
            catalog_album_id: Some("album-id".into()),
            title: format!("Música {i} \"ao vivo\""),
            artist: "Artista".into(),
            album: "Álbum".into(),
            disc: 1,
            number: i + 1,
            file_name: format!("{i}.flac"),
            media_root: None,
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
            path: root.join(format!("{i}.flac")),
        })
        .collect::<Vec<_>>();
    SavedQueue::capture(root, &tracks, 1, 23.5, 120.0, 37, Some("album-id".into()))
}
#[cfg(windows)]
#[test]
fn unc_queue_validation_and_edits_remain_resource_free(){
    for root in [r"\\server\share\Music",r"\\?\UNC\server\share\Music"]{
        let q=queue(Path::new(root));q.validate().unwrap();
        let moved=q.edited(super::super::queue_edit::Edit::Move{from:0,to:2}).unwrap().unwrap();
        assert_eq!(moved.index,0);assert_eq!(moved.selection().root,PathBuf::from(root));
        moved.validate().unwrap();
    }
    for root in [r"\\.\pipe\music",r"\\?\GLOBALROOT\Device\HarddiskVolume1\music"]{assert!(queue(Path::new(root)).validate().is_err());}
}
#[test]
fn queue_cursor_position_and_volume_survive_without_opening_media() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut j = Journal::open(path.clone());
    assert!(j.saved.is_none());
    j.observe(queue(&f.0), true);
    assert!(j.warning.is_none());
    let restored = Journal::open(path);
    let q = restored.saved.unwrap();
    assert_eq!(q.index, 1);
    assert_eq!(q.elapsed, 23.5);
    assert_eq!(q.volume, 37);
    assert_eq!(q.selection().tracks[1].title, "Música 1 \"ao vivo\"");
    assert!(!q.selection().tracks[1].path.exists());
}
#[test]
fn invalid_or_oversized_cache_is_explicit_and_preserved() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    for bytes in [b"broken".to_vec(), vec![b' '; MAX_BYTES as usize + 1]] {
        fs::write(&path, &bytes).unwrap();
        let mut j = Journal::open(path.clone());
        assert_eq!(j.warning, Some("queue_restore_failed"));
        assert!(j.saved.is_none());
        j.flush();
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
#[test]
fn cursor_changes_checkpoint_and_explicit_clear_survives() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut j = Journal::open(path.clone());
    let mut q = queue(&f.0);
    j.observe(q.clone(), true);
    q.index = 2;
    q.elapsed = 1.0;
    j.observe(q, false);
    assert_eq!(Journal::open(path.clone()).saved.unwrap().index, 2);
    j.clear().unwrap();
    assert!(Journal::open(path).saved.is_none());
}
#[test]
fn final_flush_preserves_latest_position_after_throttled_poll() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut j = Journal::open(path.clone());
    let mut q = queue(&f.0);
    j.observe(q.clone(), true);
    q.elapsed = 42.0;
    j.observe(q, false);
    assert_eq!(Journal::open(path.clone()).saved.unwrap().elapsed, 23.5);
    j.flush();
    assert_eq!(Journal::open(path).saved.unwrap().elapsed, 42.0);
}
#[test]
fn failed_write_keeps_previous_durable_queue_and_can_retry() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut j = Journal::open(path.clone());
    j.observe(queue(&f.0), true);
    let previous = fs::read(&path).unwrap();
    let backup = f.0.join("previous.json");
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
    let mut q = queue(&f.0);
    q.index = 2;
    j.observe(q, true);
    assert_eq!(j.warning, Some("queue_save_failed"));
    assert_eq!(fs::read(&backup).unwrap(), previous);
    assert_eq!(j.clear(), Err("queue_save_failed"));
    assert!(j.saved.is_some());
    fs::remove_dir(&path).unwrap();
    fs::rename(backup, &path).unwrap();
    j.flush();
    assert!(j.warning.is_none());
    assert_eq!(Journal::open(path).saved.unwrap().index, 2);
}
#[test]
fn malformed_cursor_paths_and_versions_are_rejected() {
    let f = Fixture::new();
    let mut q = queue(&f.0);
    q.index = 3;
    assert!(q.validate().is_err());
    q.index = 1;
    q.elapsed = f64::NAN;
    assert!(q.validate().is_err());
    q.elapsed = 1.0;
    q.tracks[0].path = f.0.join("../escape.flac");
    assert!(q.validate().is_err());
    let p = f.0.join("queue.json");
    fs::write(&p, b"{\"version\":3,\"queue\":null}").unwrap();
    assert_eq!(Journal::open(p).warning, Some("queue_restore_failed"));
}

#[test]
fn playback_options_roundtrip_and_legacy_cache_remains_readable() {
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut q = queue(&f.0);
    q.options = super::super::options::PlaybackOptions::from_status(
        &["repeat: 1", "single: 1", "random: 1"].map(String::from),
    );
    save(&path, Some(&q)).unwrap();
    let restored = load(&path).unwrap().unwrap();
    assert_eq!(restored.options, q.options);
    let mut legacy = serde_json::json!({"version":2,"queue":q});
    legacy["queue"].as_object_mut().unwrap().remove("options");
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let restored = load(&path).unwrap().unwrap();
    assert_eq!(restored.options, None);
    assert_eq!(restored.index, q.index);
    assert_eq!(restored.elapsed, q.elapsed);
    legacy["queue"]["options"] =
        serde_json::json!({"repeat":true,"single":"unsupported","random":false});
    let bytes = serde_json::to_vec(&legacy).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert_eq!(
        Journal::open(path.clone()).warning,
        Some("queue_restore_failed")
    );
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn logical_queue_edits_keep_occurrence_cursor_options_and_resume_without_media() {
    use super::super::queue_edit::{Edit, Placement};
    let f = Fixture::new();
    let mut q = queue(&f.0);
    q.options = super::super::options::PlaybackOptions::from_status(
        &["repeat: 1", "single: 0", "random: 1"].map(String::from),
    );
    // Duplicate physical paths remain separate occurrences, not deduplicated IDs.
    let selected = q.selection();
    let added = q.inserted(selected, Placement::Next).unwrap();
    assert_eq!(added.tracks().len(), 6);
    assert_eq!(added.index, 1);
    assert_eq!(added.elapsed, 23.5);
    assert_eq!(added.options, q.options);
    let moved = q.edited(Edit::Move { from: 1, to: 0 }).unwrap().unwrap();
    assert_eq!(moved.index, 0);
    assert_eq!(moved.tracks()[0].id, "id1");
    assert_eq!(moved.elapsed, 23.5);
    assert_eq!(moved.options, q.options);
    let removed = q.edited(Edit::Remove { index: 0 }).unwrap().unwrap();
    assert_eq!(removed.index, 0);
    assert_eq!(removed.tracks()[0].id, "id1");
    assert_eq!(removed.elapsed, 23.5);
    let removed_current = q.edited(Edit::Remove { index: 1 }).unwrap().unwrap();
    assert_eq!(removed_current.index, 1);
    assert_eq!(removed_current.tracks()[1].id, "id2");
    assert_eq!(removed_current.elapsed, 0.0);
    assert_eq!(removed_current.duration, 0.0);
    let mut selected = q.selection();
    selected.root = f.0.join("other");
    // Per-track roots are the multi-folder boundary. A forged root for an
    // unchanged path still fails; a different valid source is now supported.
    for track in &mut selected.tracks { track.media_root = Some(selected.root.clone()); }
    assert!(q.inserted(selected, Placement::End).is_err());
    assert_eq!(q.tracks().len(), 3);
}
#[test]
fn failed_logical_edit_does_not_publish_or_overwrite_previous_saved_queue() {
    use super::super::queue_edit::Edit;
    let f = Fixture::new();
    let path = f.0.join("queue.json");
    let mut j = Journal::open(path.clone());
    j.replace_logical(Some(queue(&f.0))).unwrap();
    let bytes = fs::read(&path).unwrap();
    let next = j
        .saved
        .as_ref()
        .unwrap()
        .edited(Edit::Move { from: 1, to: 0 })
        .unwrap();
    let backup = f.0.join("backup.json");
    fs::rename(&path, &backup).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(j.replace_logical(next.clone()), Err("queue_save_failed"));
    assert_eq!(j.saved.as_ref().unwrap().index, 1);
    assert_eq!(fs::read(&backup).unwrap(), bytes);
    fs::remove_dir(&path).unwrap();
    fs::rename(&backup, &path).unwrap();
    j.replace_logical(next).unwrap();
    assert_eq!(Journal::open(path).saved.unwrap().index, 0);
}
#[test]
fn removing_last_logical_track_clears_without_opening_or_playing_it() {
    use super::super::queue_edit::Edit;
    let f = Fixture::new();
    let q = queue(&f.0);
    let tracks = q.tracks();
    let one = SavedQueue::capture(&f.0, &tracks[..1], 0, 23.0, 120.0, 37, None);
    assert!(one.edited(Edit::Remove { index: 0 }).unwrap().is_none());
}

#[test]
fn mixed_roots_persist_occurrences_cursor_options_and_pause_without_opening_files(){
 let f=Fixture::new();let a=f.0.join("a");let b=f.0.join("b");let q=queue(&a);let other=queue(&b);
 let inserted=q.inserted(other.selection(),super::super::queue_edit::Placement::Next).unwrap();assert_eq!(inserted.index,q.index);assert_eq!(inserted.elapsed,q.elapsed);assert_eq!(inserted.tracks().len(),6);
 let mut j=Journal::open(f.0.join("mixed.json"));j.replace_logical(Some(inserted)).unwrap();
 let restored=Journal::open(f.0.join("mixed.json")).saved.unwrap();assert_eq!(restored.tracks()[2].media_root.as_ref(),Some(&b));assert_eq!(restored.tracks()[0].media_root.as_ref(),Some(&a));assert!(restored.tracks().iter().all(|t|!t.path.exists()));
 let moved=restored.edited(super::super::queue_edit::Edit::Move{from:2,to:0}).unwrap().unwrap();assert_eq!(moved.selection().root,b);assert_eq!(moved.tracks()[0].media_root.as_ref(),Some(&moved.selection().root));
 let encoded=serde_json::to_string(&moved).unwrap();assert!(!encoded.contains("http://"));assert!(!encoded.contains("/media/"));
}
#[test]
fn queue_legacy_read_only_migration_and_invalid_new_cache_do_not_erase_backup(){
 let f=Fixture::new();let old=f.0.join("old.json");let next=f.0.join("new.json");let mut value=serde_json::json!({"version":1,"queue":queue(&f.0)});
 for row in value["queue"]["tracks"].as_array_mut().unwrap(){row.as_object_mut().unwrap().remove("root");}
 let bytes=serde_json::to_vec(&value).unwrap();fs::write(&old,&bytes).unwrap();let j=Journal::open_migrating(next.clone(),Some(old.clone()));assert!(j.warning.is_none());assert_eq!(j.saved.unwrap().index,1);assert!(!next.exists());assert_eq!(fs::read(&old).unwrap(),bytes);
 fs::write(&next,b"broken").unwrap();let j=Journal::open_migrating(next.clone(),Some(old.clone()));assert_eq!(j.warning,Some("queue_restore_failed"));assert_eq!(fs::read(next).unwrap(),b"broken");assert_eq!(fs::read(old).unwrap(),bytes);
}

#[test]
fn corrupt_checkpoint_is_preserved_and_radio_is_refused_before_candidate_preparation(){
 let f=Fixture::new();let path=f.0.join("queue.json");let bytes=b"broken cache fixture";fs::write(&path,bytes).unwrap();let mut journal=Journal::open(path.clone());
 assert_eq!(journal.warning,Some("queue_restore_failed"));assert!(journal.saved.is_none());
 for force in [false,true] {journal.observe_checkpoint(None,force);assert_eq!(fs::read(&path).unwrap(),bytes);assert_eq!(journal.warning,Some("queue_restore_failed"));assert!(!journal.dirty);}
 let selected=crate::windows_radio::Station{id:format!("manual:{}","a".repeat(64)),name:"Fixture".into(),country_code:String::new(),tags:vec![],codec:String::new(),bitrate:0,favicon:None,stream_url:"https://fixture.invalid/live".into(),manual:true}.selection().unwrap();
 assert_eq!(super::super::radio::admit(&selected,journal.warning),Err("PLAYBACK_PERSISTENCE_FAILED"));assert_eq!(fs::read(&path).unwrap(),bytes);
}
#[test]
fn confirmed_transient_uses_null_envelope_once_and_failed_write_keeps_previous_data(){
 let f=Fixture::new();let path=f.0.join("queue.json");let backup=f.0.join("prior.json");let mut journal=Journal::open(path.clone());journal.observe(queue(&f.0),true);let old=fs::read(&path).unwrap();
 fs::rename(&path,&backup).unwrap();fs::create_dir(&path).unwrap();fs::write(path.join("fixture"),b"keep").unwrap();
 journal.observe_checkpoint(None,true);assert_eq!(journal.warning,Some("queue_save_failed"));assert!(journal.saved.is_none()&&journal.dirty);assert_eq!(fs::read(&backup).unwrap(),old);assert_eq!(fs::read(path.join("fixture")).unwrap(),b"keep");
 fs::remove_dir_all(&path).unwrap();let last=journal.last_write;journal.observe_checkpoint(None,false);assert_eq!(journal.last_write,last);assert!(!path.exists());assert!(journal.dirty);
 journal.last_write=Instant::now()-Duration::from_secs(6);journal.observe_checkpoint(None,false);assert_eq!(journal.warning,None);assert!(!journal.dirty);assert!(load(&path).unwrap().is_none());assert_eq!(fs::read(&backup).unwrap(),old);
 fs::remove_file(&path).unwrap();fs::create_dir(&path).unwrap();journal.observe_checkpoint(None,true);assert!(path.is_dir());assert_eq!(journal.warning,None);
}
#[test]
fn local_checkpoint_policy_is_exact_and_radio_never_restores_as_a_local_path(){
 let f=Fixture::new();let path=f.0.join("queue.json");let mut before=Journal::open(path.clone());before.observe(queue(&f.0),true);let expected=fs::read(&path).unwrap();
 let mut projected=Journal::open(path.clone());projected.observe_checkpoint(Some(queue(&f.0)),true);assert_eq!(fs::read(&path).unwrap(),expected);
 let mut poisoned=queue(&f.0);poisoned.tracks[0].metadata.radio=Some(crate::windows_radio::Playing{station_id:format!("manual:{}","a".repeat(64)),thumb:None,stream_url:"https://fixture.invalid/?key=PRIVATE".into(),station_name:"Fixture".into()});
 assert_eq!(poisoned.validate(),Err("queue_restore_failed"));
 let bytes=serde_json::to_vec(&poisoned).unwrap();assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE"));fs::write(&path,bytes).unwrap();assert_eq!(Journal::open(path).warning,Some("queue_restore_failed"));
}

#[test]
fn live_v3_imports_legacy_read_only_then_saves_without_altering_rollback_files() {
 let f=Fixture::new();let current=f.0.join("v3.json");let v2=f.0.join("v2.json");let v1=f.0.join("v1.json");
 let before=serde_json::to_vec(&serde_json::json!({"version":2,"queue":queue(&f.0)})).unwrap();
 fs::write(&v2,&before).unwrap();fs::write(&v1,b"keep older backup").unwrap();
 let mut j=Journal::open_chain(current.clone(),vec![v2.clone(),v1.clone()]);
 assert!(j.warning.is_none());assert_eq!(j.saved.as_ref().unwrap().index,1);assert!(!current.exists());
 let selected=j.saved.clone();j.replace_logical(selected).unwrap();
 let encoded:serde_json::Value=serde_json::from_slice(&fs::read(&current).unwrap()).unwrap();assert_eq!(encoded["version"],4);assert_eq!(encoded["content"]["kind"],"queue");
 assert_eq!(fs::read(&v2).unwrap(),before);assert_eq!(fs::read(&v1).unwrap(),b"keep older backup");
 assert_eq!(Journal::open_chain(current,vec![v2,v1]).saved.unwrap().elapsed,23.5);
}
#[test]
fn live_v3_current_or_legacy_empty_is_authoritative_and_never_resurrects_v1() {
 let f=Fixture::new();let current=f.0.join("v3.json");let v2=f.0.join("v2.json");let v1=f.0.join("v1.json");
 fs::write(&v2,br#"{"version":2,"queue":null}"#).unwrap();let old=serde_json::to_vec(&serde_json::json!({"version":1,"queue":queue(&f.0)})).unwrap();fs::write(&v1,&old).unwrap();
 let mut j=Journal::open_chain(current.clone(),vec![v2.clone(),v1.clone()]);assert!(j.saved.is_none());assert!(j.warning.is_none());assert!(!current.exists());
 j.clear().unwrap();fs::write(&v2,old.clone()).unwrap();
 assert!(Journal::open_chain(current,vec![v2,v1.clone()]).saved.is_none());assert_eq!(fs::read(v1).unwrap(),old);
}
#[test]
fn live_v3_corrupt_current_or_v2_never_falls_through_or_rewrites_any_file() {
 let f=Fixture::new();let current=f.0.join("v3.json");let v2=f.0.join("v2.json");let v1=f.0.join("v1.json");
 let old=serde_json::to_vec(&serde_json::json!({"version":1,"queue":queue(&f.0)})).unwrap();fs::write(&v1,&old).unwrap();fs::write(&v2,&old).unwrap();
 for bad in [b"broken".to_vec(),br#"{"version":5,"content":{"kind":"empty"}}"#.to_vec(),old.clone()] {
  fs::write(&current,&bad).unwrap();let mut j=Journal::open_chain(current.clone(),vec![v2.clone(),v1.clone()]);assert_eq!(j.warning,Some("queue_restore_failed"));j.observe_checkpoint(None,true);j.flush();assert_eq!(fs::read(&current).unwrap(),bad);assert_eq!(fs::read(&v2).unwrap(),old);
 }
 fs::remove_file(&current).unwrap();fs::write(&v2,b"bad legacy").unwrap();let mut j=Journal::open_chain(current.clone(),vec![v2.clone(),v1.clone()]);assert_eq!(j.warning,Some("queue_restore_failed"));j.observe_checkpoint(None,true);assert!(!current.exists());assert_eq!(fs::read(v2).unwrap(),b"bad legacy");assert_eq!(fs::read(v1).unwrap(),old);
}
#[test]
fn live_mixed_queue_roundtrip_preserves_occurrences_without_live_context_or_media_access() {
 use super::super::queue_v3::Source;
 let f=Fixture::new();let path=f.0.join("v3.json");let mut tracks=queue(&f.0).tracks();
 for index in [0,2] { let t=&mut tracks[index];
  t.path=PathBuf::new();t.media_root=None;t.remote_source=Some(Source::Navidrome{account_id:"a".repeat(64),song_id:"same-song".into()});t.remote_connection_id=Some("PRIVATE-live-session".into());
 }
 let mut mixed=SavedQueue::capture(&f.0,&tracks,2,33.0,90.0,71,None);mixed.options=super::super::options::PlaybackOptions::from_status(&["repeat: 1","single: oneshot","random: 0"].map(String::from));
 let mut j=Journal::open_chain(path.clone(),vec![]);j.replace_logical(Some(mixed.clone())).unwrap();
 let bytes=fs::read(&path).unwrap();let text=String::from_utf8_lossy(&bytes);assert!(!text.contains("PRIVATE"));assert!(!text.contains("remoteConnectionId"));assert!(!text.contains("http"));
 let q=Journal::open_chain(path,vec![]).saved.unwrap();assert_eq!(q.index,2);assert_eq!(q.elapsed,33.0);assert_eq!(q.volume,71);assert_eq!(q.options,mixed.options);
 let restored=q.tracks();assert_eq!(restored.len(),3);assert_eq!(restored[0].remote_source,restored[2].remote_source);assert!(restored[0].path.as_os_str().is_empty());assert!(restored[0].media_root.is_none());assert!(restored[0].remote_connection_id.is_none());assert_eq!(restored[1].path,tracks[1].path);assert_eq!(restored[1].media_root,tracks[1].media_root);
 let moved=q.edited(super::super::queue_edit::Edit::Move{from:2,to:0}).unwrap().unwrap();assert_eq!(moved.index,0);assert_eq!(moved.elapsed,33.0);
 let removed=q.edited(super::super::queue_edit::Edit::Remove{index:2}).unwrap().unwrap();assert_eq!(removed.index,1);assert_eq!(removed.elapsed,0.0);
}
#[test]
fn live_remote_only_queue_has_no_synthetic_local_paths_and_rejects_ambiguous_transports() {
 use super::super::queue_v3::Source;
 let f=Fixture::new();let mut t=queue(&f.0).tracks().remove(0);t.path=PathBuf::new();t.media_root=None;t.remote_source=Some(Source::Plex{server_id:"server".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:"1".into()});
 let q=SavedQueue::capture(Path::new(""),&[t.clone()],0,0.0,0.0,50,None);q.validate().unwrap();let p=f.0.join("remote.json");save(&p,Some(&q)).unwrap();let restored=Journal::open_chain(p,vec![]).saved.unwrap();assert!(restored.root.as_os_str().is_empty());assert!(restored.selection().root.as_os_str().is_empty());assert!(restored.tracks()[0].media_root.is_none());
 t.path=f.0.join("ambiguous.flac");assert!(SavedQueue::capture(&f.0,&[t],0,0.0,0.0,50,None).validate().is_err());
}
#[cfg(unix)]
#[test]
fn live_v3_rejects_symlinks_dangling_links_and_nonregular_entries_without_reading_target() {
 use std::os::unix::fs::symlink;
 let f=Fixture::new();let target=f.0.join("target.json");save(&target,Some(&queue(&f.0))).unwrap();let bytes=fs::read(&target).unwrap();let path=f.0.join("link.json");
 symlink(&target,&path).unwrap();assert_eq!(Journal::open_chain(path.clone(),vec![target.clone()]).warning,Some("queue_restore_failed"));assert_eq!(fs::read(&target).unwrap(),bytes);fs::remove_file(&path).unwrap();
 symlink(f.0.join("missing"),&path).unwrap();assert_eq!(Journal::open_chain(path.clone(),vec![target.clone()]).warning,Some("queue_restore_failed"));fs::remove_file(&path).unwrap();
 fs::create_dir(&path).unwrap();assert_eq!(Journal::open_chain(path,vec![target]).warning,Some("queue_restore_failed"));
}

fn artwork_queue(root:&Path)->SavedQueue{
 use super::super::queue_v3::Source;
 let mut tracks=queue(root).tracks();
 tracks[1].path=PathBuf::new();tracks[1].media_root=None;tracks[1].catalog_album_id=None;tracks[1].remote_source=Some(Source::Navidrome{account_id:"a".repeat(64),song_id:"nav-one".into()});
 let p=&mut tracks[2];p.path=PathBuf::new();p.media_root=None;p.catalog_album_id=None;p.remote_source=Some(Source::Plex{server_id:"server-one".into(),part_key:"/library/parts/1/file.flac".into(),rating_key:"song-one".into()});p.plex_image=Some(crate::plex::PlexImageRef{server_id:"server-one".into(),path:"/library/metadata/album-one/thumb/12".into()});p.remote_connection_id=Some("PRIVATE_CONNECTION".into());
 tracks.push(tracks[2].clone());let mut saved=SavedQueue::capture(root,&tracks,3,22.0,90.0,71,None);saved.options=super::super::options::PlaybackOptions::from_status(&["repeat: 1","single: oneshot","random: 0"].map(String::from));saved
}
#[test]
fn v4_artwork_mixed_queue_restores_and_follows_duplicate_occurrences_through_edits(){
 use super::super::queue_edit::{Edit,Placement};
 let f=Fixture::new();let file=f.0.join("v4.json");let before=artwork_queue(&f.0);let expected=before.tracks()[2].plex_image.clone();let mut j=Journal::open_chain(file.clone(),vec![]);j.replace_logical(Some(before.clone())).unwrap();
 let bytes=fs::read(&file).unwrap();let text=String::from_utf8_lossy(&bytes);assert!(text.contains("album-one/thumb"));for denied in ["PRIVATE_CONNECTION","http://","remoteConnectionId","remoteLease","X-Plex-Token"]{assert!(!text.contains(denied));}
 let q=Journal::open_chain(file.clone(),vec![]).saved.unwrap();assert_eq!((q.index,q.elapsed,q.volume),(3,22.0,71));assert_eq!(q.options,before.options);let rows=q.tracks();assert!(rows[0].plex_image.is_none()&&rows[1].plex_image.is_none());assert_eq!(rows[2].plex_image,expected);assert_eq!(rows[3].plex_image,expected);assert!(rows.iter().all(|t|t.remote_lease.is_none()&&t.remote_connection_id.is_none()));
 let moved=q.edited(Edit::Move{from:3,to:0}).unwrap().unwrap();let removed=moved.edited(Edit::Remove{index:1}).unwrap().unwrap();let added=removed.inserted(before.selection(),Placement::Next).unwrap();j.replace_logical(Some(added)).unwrap();let restored=Journal::open_chain(file,vec![]).saved.unwrap();assert_eq!((restored.index,restored.elapsed),(0,22.0));
 for row in restored.tracks(){if matches!(row.remote_source,Some(super::super::queue_v3::Source::Plex{..})){assert_eq!(row.plex_image,expected);}else{assert!(row.plex_image.is_none());}}
}
#[test]
fn v4_absence_only_migrates_v3_read_only_and_empty_or_corrupt_current_remains_authoritative(){
 let f=Fixture::new();let old=f.0.join("v3.json");let current=f.0.join("v4.json");let bytes=super::super::queue_v3::encode(Some(&artwork_queue(&f.0).logical().unwrap())).unwrap();fs::write(&old,&bytes).unwrap();
 let mut j=Journal::open_chain(current.clone(),vec![old.clone()]);assert!(j.warning.is_none());assert_eq!(j.saved.as_ref().unwrap().index,3);assert!(!current.exists());assert_eq!(fs::read(&old).unwrap(),bytes);assert!(j.saved.as_ref().unwrap().tracks().iter().all(|t|t.plex_image.is_none()));
 j.replace_logical(Some(artwork_queue(&f.0))).unwrap();assert_eq!(fs::read(&old).unwrap(),bytes);assert_eq!(serde_json::from_slice::<serde_json::Value>(&fs::read(&current).unwrap()).unwrap()["version"],4);
 j.clear().unwrap();assert!(Journal::open_chain(current.clone(),vec![old.clone()]).saved.is_none());let empty=fs::read(&current).unwrap();assert_eq!(super::super::queue_v4::decode(&empty).unwrap().map(|_|()),None);
 fs::write(&current,b"corrupt v4 fixture").unwrap();let mut bad=Journal::open_chain(current.clone(),vec![old.clone()]);assert_eq!(bad.warning,Some("queue_restore_failed"));assert!(bad.saved.is_none());bad.observe_checkpoint(None,true);assert_eq!(fs::read(&current).unwrap(),b"corrupt v4 fixture");assert_eq!(fs::read(&old).unwrap(),bytes);
 fs::remove_file(&current).unwrap();fs::write(&old,b"corrupt v3 fixture").unwrap();assert_eq!(Journal::open_chain(current.clone(),vec![old]).warning,Some("queue_restore_failed"));assert!(!current.exists());
}
#[test]
fn v4_invalid_artwork_is_discarded_without_changing_media_but_ambiguous_schema_is_rejected(){
 let f=Fixture::new();let path=f.0.join("v4.json");save(&path,Some(&artwork_queue(&f.0))).unwrap();let raw:serde_json::Value=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
 for image in [serde_json::json!({"server_id":"foreign","path":"/library/metadata/1/thumb"}),serde_json::json!({"server_id":"server-one","path":"https://foreign.invalid/image"}),serde_json::json!({"server_id":"server-one","path":"/library/metadata/1/thumb?X-Plex-Token=SECRET"}),serde_json::json!({"server_id":"server-one","path":"/library/metadata/%2e%2e/thumb"})]{let mut value=raw.clone();value["content"]["artwork"][2]=image;let result=decode(&serde_json::to_vec(&value).unwrap(),false).unwrap().unwrap();assert_eq!(result.index,3);assert_eq!(result.tracks()[2].remote_source,artwork_queue(&f.0).tracks()[2].remote_source);assert!(result.tracks()[2].plex_image.is_none());assert!(result.tracks()[3].plex_image.is_some());}
 for kind in 0..5 {let mut value=raw.clone();match kind{0=>value["content"]["artwork"][2]["token"]=serde_json::json!("SECRET"),1=>value["content"]["artwork"].as_array_mut().unwrap().pop().map(|_|()).unwrap(),2=>value["version"]=serde_json::json!(5),3=>value["content"]["queue"]["options"]["fake"]=serde_json::json!(true),_=>value["credential"]=serde_json::json!("SECRET")};assert!(decode(&serde_json::to_vec(&value).unwrap(),false).is_err());}
 let duplicate=String::from_utf8(fs::read(path).unwrap()).unwrap().replacen("\"version\":4","\"version\":4,\"version\":4",1);assert!(decode(duplicate.as_bytes(),false).is_err());
}
#[test]
fn v4_oversized_save_keeps_the_previous_checkpoint_and_artwork_bytes(){
 let f=Fixture::new();let path=f.0.join("v4.json");let before=artwork_queue(&f.0);let mut j=Journal::open_chain(path.clone(),vec![]);j.replace_logical(Some(before.clone())).unwrap();let bytes=fs::read(&path).unwrap();
 let mut row=before.tracks()[2].clone();row.title="T".repeat(4096);row.artist="A".repeat(4096);let huge=SavedQueue::capture(Path::new(""),&vec![row;1000],0,0.0,0.0,50,None);assert!(j.replace_logical(Some(huge)).is_err());assert_eq!(fs::read(&path).unwrap(),bytes);assert_eq!(j.saved.unwrap().index,3);assert_eq!(Journal::open_chain(path,vec![]).saved.unwrap().tracks()[2].plex_image,before.tracks()[2].plex_image);
}
