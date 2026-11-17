use super::*;
use std::fs;
use super::super::Library;
use std::time::{Duration, Instant};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self { let mut n=[0;12]; getrandom::fill(&mut n).unwrap(); let p=std::env::temp_dir().join(format!("sonante-added-{n:x?}")); fs::create_dir(&p).unwrap(); Self(p.canonicalize().unwrap()) }
    fn config(&self) -> PathBuf { self.0.join("folders.json") }
    fn audio(&self, root:&str, album:&str, name:&str) -> PathBuf {
        let p=self.0.join(root).join(album); fs::create_dir_all(&p).unwrap();
        fs::write(p.join(name), crate::local_file_info::tests::fixture(44100,16,2,44100)).unwrap();
        self.0.join(root)
    }
}
impl Drop for Fixture { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
fn ready(l:&Library) { let until=Instant::now()+Duration::from_secs(5); loop { let s=l.status().unwrap(); if s.phase=="ready" { assert_eq!(s.warnings,0); break; } assert!(Instant::now()<until,"{:?}",s.error); std::thread::sleep(Duration::from_millis(5)); } }
fn rows(track:&str, root:&str) -> Vec<(String,String)> { vec![(id(track),id(root))] }

#[test]
fn legacy_unknown_baseline_then_future_additions_are_dated_and_stable() {
    let f=Fixture::new(); let config=f.config(); let root=f.0.join("old");
    let root_hash=id(&root.to_string_lossy()); let legacy=BTreeSet::from([root_hash.clone()]);
    let first=vec![(id("old-track"),root_hash.clone())];
    let d=observe(&config,&legacy,&[root.clone()],&first,true,100,||true).unwrap(); assert_eq!(d[&id("old-track")],None);
    let mut next=first; next.push((id("new-track"),root_hash));
    let d=observe(&config,&legacy,&[root.clone()],&next,true,200,||true).unwrap(); assert_eq!(d[&id("new-track")],Some(200));
    let before=fs::read(path(&config)).unwrap();
    let d=observe(&config,&legacy,&[root],&next,true,999,||true).unwrap(); assert_eq!(d[&id("new-track")],Some(200)); assert_eq!(before,fs::read(path(&config)).unwrap());
}
#[test]
fn partial_and_cancelled_scans_do_not_record_dates_or_initialize_baseline() {
    let f=Fixture::new(); let config=f.config(); let root=f.0.join("root"); let row=rows("track",root.to_str().unwrap());
    assert!(observe(&config,&BTreeSet::new(),&[root.clone()],&row,false,100,||true).unwrap().is_empty()); assert!(!path(&config).exists());
    assert!(observe(&config,&BTreeSet::new(),&[root.clone()],&row,true,100,||false).is_err()); assert!(!path(&config).exists());
    let calls=std::cell::Cell::new(0);
    assert!(observe(&config,&BTreeSet::new(),&[root.clone()],&row,true,100,||{calls.set(calls.get()+1);calls.get()==1}).is_err()); assert!(!path(&config).exists());
    observe(&config,&BTreeSet::new(),&[root.clone()],&row,true,200,||true).unwrap();
    let before=fs::read(path(&config)).unwrap(); let mut next=row;next.extend(rows("new",root.to_str().unwrap()));
    let d=observe(&config,&BTreeSet::new(),&[root],&next,false,300,||true).unwrap(); assert_eq!(d[&id("track")],Some(200));assert!(!d.contains_key(&id("new")));assert_eq!(before,fs::read(path(&config)).unwrap());
}
#[test]
fn corruption_empty_duplicate_keys_and_invalid_schema_are_never_reset() {
    let f=Fixture::new();let config=f.config();
    for bytes in [b"".to_vec(),b"{".to_vec(),b"{\"version\":1,\"version\":1,\"roots\":[],\"tracks\":{}}".to_vec(),b"{\"version\":9,\"roots\":[],\"tracks\":{}}".to_vec(),b"{\"version\":1,\"roots\":[],\"tracks\":{\"bad\":100}}".to_vec()] {
        fs::write(path(&config),&bytes).unwrap();assert!(observe(&config,&BTreeSet::new(),&[],&[],true,200,||true).is_err());assert_eq!(fs::read(path(&config)).unwrap(),bytes);
    }
}
#[test]
fn current_empty_record_is_authoritative_and_new_roots_have_real_admission_dates() {
    let f=Fixture::new();let config=f.config(); let root=f.0.join("new");
    fs::write(path(&config),b"{\"version\":1,\"roots\":[],\"tracks\":{}}").unwrap();
    let d=observe(&config,&BTreeSet::new(),&[root.clone()],&rows("one",root.to_str().unwrap()),true,123,||true).unwrap();assert_eq!(d[&id("one")],Some(123));
    // Removal/re-add and metadata-only changes preserve the original event.
    observe(&config,&BTreeSet::new(),&[],&[],true,200,||true).unwrap();
    let d=observe(&config,&BTreeSet::new(),&[root.clone()],&rows("one",root.to_str().unwrap()),true,300,||true).unwrap();assert_eq!(d[&id("one")],Some(123));
}
#[test]
fn utc_wire_format_roundtrips_original_linux_parser_including_calendar_boundaries() {
    for (t, expected) in [(0,"1970-01-01T00:00:00Z"),(951782400,"2000-02-29T00:00:00Z"),(1709251199,"2024-02-29T23:59:59Z"),(4107542400,"2100-03-01T00:00:00Z"),(MAX_TIME,"9999-12-31T23:59:59Z")] {
        let value=utc(t).unwrap();assert_eq!(value,expected);assert_eq!(crate::windows_smart_playlists::local_added(Some(&value)),Some(t));
    }
    assert!(utc(MAX_TIME+1).is_none());
}
#[test]
fn restored_library_keeps_unknown_then_new_album_flows_to_original_smart_catalog_and_reopen() {
    let f=Fixture::new();let root=f.audio("old","Legacy","one.flac");
    super::super::save(&f.config(),&super::super::Saved{version:3,usage_mode:super::super::UsageMode::Full,online_artwork_enabled:false,experimental_features:false,first_run:None,root:None,roots:vec![root.clone()],sources:None,catalog_rebuild_attempted:false,catalog_generation:None}).unwrap();
    let l=Library::new(f.config());ready(&l);
    let old=l.compatibility_catalog().unwrap()["albums"][0]["id"].as_str().unwrap().to_owned();
    assert!(l.complete_album_snapshot().unwrap().1[0].added_at.is_none());
    f.audio("old","Future","new.flac");l.refresh().unwrap();ready(&l);
    let (_,catalog)=crate::windows_smart_playlists::catalog(&l).unwrap();let date=catalog.iter().find(|a|a.title=="Future").unwrap().added_at.unwrap();assert!(catalog.iter().find(|a|a.id==old).unwrap().added_at.is_none());
    assert_eq!(l.compatibility_catalog().unwrap()["albums"].as_array().unwrap().iter().find(|a|a["title"]=="Future").unwrap()["added_at"],utc(date).unwrap());
    let smart=crate::windows_smart_playlists::preview(&l,&crate::windows_favorites::Store::new(f.0.join("favorites.json")),serde_json::from_value(serde_json::json!({"source":{"kind":"local"},"genre":null,"artist":null,"year_from":null,"year_to":null,"added_days":1,"favorites_only":false,"sort":"added_newest","limit":10})).unwrap()).unwrap();
    assert_eq!(smart.matched_albums,1);assert_eq!(smart.albums[0].title,"Future");
    let root2=f.audio("newroot","Other","two.flac"); l.select_roots(vec![root,root2]).unwrap();ready(&l);assert!(crate::windows_smart_playlists::catalog(&l).unwrap().1.iter().find(|a|a.title=="Other").unwrap().added_at.is_some());
    l.stop(); let reopened=Library::new(f.config());ready(&reopened);
    assert_eq!(crate::windows_smart_playlists::catalog(&reopened).unwrap().1.iter().find(|a|a.title=="Future").unwrap().added_at,Some(date));reopened.stop();
}
#[test]
fn corrupt_date_record_fails_smart_read_but_does_not_break_catalog_selection() {
    let f=Fixture::new();let root=f.audio("root","Album","one.flac");fs::write(path(&f.config()),b"corrupt").unwrap();
    let l=Library::new(f.config());l.select(root).unwrap();ready(&l);
    let id=l.albums("",0,10).unwrap().items[0].id.clone();assert!(l.select_queue(&id,None).is_ok());assert!(crate::windows_smart_playlists::catalog(&l).is_err());assert_eq!(fs::read(path(&f.config())).unwrap(),b"corrupt");l.stop();
}
#[cfg(unix)]
#[test]
fn linked_journal_is_not_followed_or_replaced() {
    let f=Fixture::new();let outside=f.0.join("outside");fs::write(&outside,b"untouched").unwrap();std::os::unix::fs::symlink(&outside,path(&f.config())).unwrap();
    assert!(observe(&f.config(),&BTreeSet::new(),&[],&[],true,100,||true).is_err());assert_eq!(fs::read(&outside).unwrap(),b"untouched");assert!(fs::symlink_metadata(path(&f.config())).unwrap().is_symlink());
}

#[test]
fn warning_scan_never_establishes_upgrade_baseline_or_fabricates_dates() {
    let f=Fixture::new(); let root=f.audio("root","Old","good.flac");
    fs::write(root.join("bad.flac"),b"invalid metadata fixture").unwrap();
    super::super::save(&f.config(),&super::super::Saved{version:3,usage_mode:super::super::UsageMode::Full,online_artwork_enabled:false,experimental_features:false,first_run:None,root:None,roots:vec![root.clone()],sources:None,catalog_rebuild_attempted:false,catalog_generation:None}).unwrap();
    let l=Library::new(f.config()); let deadline=Instant::now()+Duration::from_secs(5);
    while l.status().unwrap().phase=="scanning" {assert!(Instant::now()<deadline);std::thread::sleep(Duration::from_millis(5));}
    assert!(l.status().unwrap().warnings>0);assert!(!path(&f.config()).exists());
    fs::remove_file(root.join("bad.flac")).unwrap();l.refresh().unwrap();ready(&l);assert!(path(&f.config()).exists());
    assert!(l.complete_album_snapshot().unwrap().1.iter().all(|a|a.added_at.is_none()));l.stop();
}
