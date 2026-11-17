 use super::*;
 struct Temp(PathBuf);
 impl Temp{fn new()->Self{let mut id=[0;12];getrandom::fill(&mut id).unwrap();let root=std::env::temp_dir().join(format!("sonante-source-flags-{id:x?}"));fs::create_dir(&root).unwrap();Self(root)}}
 impl Drop for Temp{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
 fn ready(library:&Library){let until=std::time::Instant::now()+std::time::Duration::from_secs(3);loop{let status=library.status().unwrap();if status.phase!="scanning"{assert_eq!(status.phase,"ready");return;}assert!(std::time::Instant::now()<until);std::thread::sleep(std::time::Duration::from_millis(5));}}
 #[test]
 fn legacy_folder_settings_migrate_flags_atomically_without_rescanning_when_only_navidrome_changes(){
  let t=Temp::new();let music=t.0.join("fixture-folder");fs::create_dir(&music).unwrap();let music=music.canonicalize().unwrap();let settings=t.0.join("folders.json");let old=serde_json::to_vec(&serde_json::json!({"version":2,"roots":[music]})).unwrap();fs::write(&settings,&old).unwrap();
  let library=Library::new(settings.clone());ready(&library);assert_eq!(library.sources().unwrap(),Sources::default());assert_eq!(fs::read(&settings).unwrap(),old);let before=library.status().unwrap();
  library.configure(None,Sources{local:true,navidrome:true,plex:false}).unwrap();let after=library.status().unwrap();assert_eq!(before.epoch,after.epoch);assert_eq!(before.revision,after.revision);assert_eq!(library.configured_roots().unwrap(),[music.clone()]);let value:serde_json::Value=serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();assert_eq!(value["version"],3);assert_eq!(value["sources"],serde_json::json!({"local":true,"navidrome":true,"plex":false}));
  library.stop();let reopened=Library::new(settings);ready(&reopened);assert_eq!(reopened.sources().unwrap(),Sources{local:true,navidrome:true,plex:false});assert_eq!(reopened.preferences().unwrap().0,[music]);reopened.stop();
 }
 #[test]
 fn nav_only_restart_never_requires_a_removed_local_folder_and_reenable_failure_can_roll_back_real_disk_state(){
  let t=Temp::new();let music=t.0.join("removed-folder");fs::create_dir(&music).unwrap();let music=music.canonicalize().unwrap();let settings=t.0.join("folders.json");fs::write(&settings,serde_json::to_vec(&serde_json::json!({"version":2,"roots":[music],"sources":{"local":false,"navidrome":true}})).unwrap()).unwrap();fs::remove_dir(&music).unwrap();
  let library=Library::new(settings.clone());let status=library.status().unwrap();assert_eq!(status.phase,"ready");assert!(status.error.is_none());assert_eq!(status.tracks,0);assert_eq!(library.configured_roots().unwrap(),[music.clone()]);let old=library.preferences().unwrap();
  let result=super::super::config_transaction::apply(old,None,Sources{local:true,navidrome:true,plex:false},|roots,sources|library.configure(roots,sources).map(|_|()),|_|panic!("unchanged Nav host"),||true);assert!(result.is_err());assert_eq!(library.sources().unwrap(),Sources{local:false,navidrome:true,plex:false});let disk:serde_json::Value=serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();assert_eq!(disk["sources"]["local"],false);assert_eq!(disk["sources"]["navidrome"],true);assert_eq!(library.configured_roots().unwrap(),[music]);library.stop();
 }
 #[test]
 fn invalid_or_future_sources_and_failed_atomic_save_preserve_previous_preferences(){
  let t=Temp::new();let settings=t.0.join("folders.json");let library=Library::new(settings.clone());let good=Sources{local:false,navidrome:true,plex:false};library.configure(None,good).unwrap();let bytes=fs::read(&settings).unwrap();assert!(library.configure(None,Sources{local:false,navidrome:false,plex:false}).is_err());assert_eq!(fs::read(&settings).unwrap(),bytes);
  let backup=t.0.join("before.json");fs::rename(&settings,&backup).unwrap();fs::create_dir(&settings).unwrap();assert!(library.configure(None,Sources::default()).is_err());assert_eq!(library.sources().unwrap(),good);assert_eq!(fs::read(&backup).unwrap(),bytes);fs::remove_dir(&settings).unwrap();fs::rename(backup,&settings).unwrap();library.stop();
  let corrupt=br#"{"version":2,"roots":[],"sources":{"local":true,"navidrome":true,"future":true}}"#;fs::write(&settings,corrupt).unwrap();let bad=Library::new(settings.clone());assert!(bad.sources().is_err());assert!(bad.configure(None,good).is_err());assert_eq!(fs::read(settings).unwrap(),corrupt);bad.stop();
 }

 #[test]
 fn orphan_favorite_is_presented_missing_only_after_clean_scan_and_roots_still_available(){
  let t=Temp::new();let music=t.0.join("music");fs::create_dir(&music).unwrap();let library=Library::new(t.0.join("folders.json"));library.configure(Some(vec![music.clone()]),Sources::default()).unwrap();ready(&library);
  let absent=library.favorite_cover("removed-album").unwrap();assert!(!absent.exists);assert!(absent.thumb.is_none());
  library.data.lock().unwrap().status.warnings=1;assert!(library.favorite_cover("removed-album").is_err());library.data.lock().unwrap().status.warnings=0;
  fs::remove_dir(&music).unwrap();assert!(library.favorite_cover("removed-album").is_err());library.stop();
 }
 #[test]
 fn disabled_or_unconfigured_local_catalog_never_proves_a_favorite_missing(){
  let t=Temp::new();let library=Library::new(t.0.join("folders.json"));assert!(library.favorite_cover("removed-album").is_err());library.configure(None,Sources{local:false,navidrome:true,plex:false}).unwrap();assert!(library.favorite_cover("removed-album").is_err());library.stop();
 }
 #[test]
 fn request_bound_missing_root_proof_rejects_version_warning_and_shutdown_changes(){
  let t=Temp::new();let music=t.0.join("music");fs::create_dir(&music).unwrap();let library=Library::new(t.0.join("folders.json"));library.configure(Some(vec![music]),Sources::default()).unwrap();ready(&library);
  let status=library.status().unwrap();let proof=library.missing_roots_read(status.epoch,status.revision,&||true).unwrap();assert!(proof.clean_catalog());
  assert!(library.missing_roots_read(status.epoch,status.revision+1,&||true).is_err());assert!(library.missing_roots_read(status.epoch,status.revision,&||false).is_err());
  library.data.lock().unwrap().status.warnings=1;assert!(proof.validate(&library).is_err());
  let partial=library.missing_roots_read(status.epoch,status.revision,&||true).unwrap();assert!(!partial.clean_catalog());assert!(library.favorite_cover("removed-album").is_err());
  library.stop();assert!(partial.validate(&library).is_err());
 }
 #[test]
 fn favorite_missing_proof_timeout_retains_worker_and_rejects_late_success(){
  use std::sync::{mpsc,Arc,atomic::{AtomicBool,Ordering}};use std::time::{Duration,Instant};
  let t=Temp::new();let music=t.0.join("music");fs::create_dir(&music).unwrap();let library=Library::new(t.0.join("folders.json"));library.configure(Some(vec![music]),Sources::default()).unwrap();ready(&library);let before=fs::read(t.0.join("folders.json")).unwrap();
  let(release,wait)=mpsc::channel();let(started,ready_worker)=mpsc::channel();let dropped=Arc::new(AtomicBool::new(false));
  struct Retained(Arc<AtomicBool>);impl Drop for Retained{fn drop(&mut self){self.0.store(true,Ordering::Release);}}
  let retained=Retained(dropped.clone());let owned=library.clone();let at=Instant::now();
  let caller=std::thread::spawn(move||owned.favorite_cover_with("removed-album",Duration::from_millis(30),move|path,current|{
   let _keep=&retained;started.send(()).unwrap();wait.recv().unwrap();assert!(!current(),"late OS return must observe retirement");Ok(path.to_path_buf())
  }).is_err());
  ready_worker.recv_timeout(Duration::from_secs(1)).unwrap();assert!(caller.join().unwrap());assert!(at.elapsed()<Duration::from_secs(1));assert!(!dropped.load(Ordering::Acquire));release.send(()).unwrap();
  let until=Instant::now()+Duration::from_secs(1);while !dropped.load(Ordering::Acquire){assert!(Instant::now()<until);std::thread::yield_now();}
  assert_eq!(fs::read(t.0.join("folders.json")).unwrap(),before);assert_eq!(library.status().unwrap().phase,"ready");library.stop();
 }
 #[test]
 fn favorite_missing_proof_revalidates_warnings_signature_and_root_set_before_publication(){
  use std::time::Duration;
  for changed in 0..3{
   let t=Temp::new();let music=t.0.join("music");fs::create_dir(&music).unwrap();let library=Library::new(t.0.join("folders.json"));library.configure(Some(vec![music]),Sources::default()).unwrap();ready(&library);
   let owned=library.clone();let before=fs::read(t.0.join("folders.json")).unwrap();
   assert!(library.favorite_cover_with("removed-album",Duration::from_secs(1),move|path,current|{
    assert!(current());let mut state=owned.data.lock().unwrap();match changed{0=>state.status.warnings=1,1=>{let mut next=state.signature.unwrap();next[0]^=1;state.signature=Some(next);},_=>state.roots.push(path.join("not-admitted")),};Ok(path.to_path_buf())
   }).is_err());assert_eq!(fs::read(t.0.join("folders.json")).unwrap(),before);library.stop();
  }
 }
 #[test]
 fn retired_or_zero_budget_missing_proof_never_visits_another_root_or_publishes_absence(){
  use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};use std::time::Duration;
  let t=Temp::new();let a=t.0.join("a");let b=t.0.join("b");fs::create_dir(&a).unwrap();fs::create_dir(&b).unwrap();let library=Library::new(t.0.join("folders.json"));library.configure(Some(vec![a,b]),Sources::default()).unwrap();ready(&library);
  assert!(library.favorite_cover_with("removed-album",Duration::ZERO,|_,_|panic!("zero budget must not start a worker")).is_err());
  let calls=Arc::new(AtomicUsize::new(0));let count=calls.clone();let owned=library.clone();
  assert!(library.favorite_cover_with("removed-album",Duration::from_secs(1),move|path,current|{
   assert!(current());count.fetch_add(1,Ordering::SeqCst);let epoch=owned.epoch.load(Ordering::Acquire);owned.invalid_before.fetch_max(epoch.saturating_add(1),Ordering::AcqRel);Ok(path.to_path_buf())
  }).is_err());assert_eq!(calls.load(Ordering::SeqCst),1);library.stop();
 }
 #[test]fn v3_absence_reads_v2_before_v1_without_writing_and_current_corruption_never_falls_back(){
  let t=Temp::new();let v3=t.0.join("v3.json");let v2=t.0.join("v2.json");let v1=t.0.join("v1.json");
  let old=br#"{"version":2,"roots":[],"sources":{"local":true,"navidrome":true}}"#;fs::write(&v2,old).unwrap();fs::write(&v1,b"bad legacy must not be read").unwrap();
  let lib=Library::with_legacy_chain(v3.clone(),vec![v2.clone(),v1.clone()]);assert_eq!(lib.sources().unwrap(),Sources{local:true,navidrome:true,plex:false});assert!(!v3.exists());assert_eq!(fs::read(&v2).unwrap(),old);
  lib.configure(None,Sources{local:false,navidrome:true,plex:true}).unwrap();lib.stop();assert_eq!(fs::read(&v2).unwrap(),old);
  let reopen=Library::with_legacy_chain(v3.clone(),vec![v2.clone(),v1.clone()]);assert_eq!(reopen.sources().unwrap(),Sources{local:false,navidrome:true,plex:true});reopen.stop();
  fs::write(&v3,b"bad current").unwrap();let bad=Library::with_legacy_chain(v3.clone(),vec![v2,v1]);assert!(bad.sources().is_err());assert!(bad.configure(None,Sources::default()).is_err());assert_eq!(fs::read(v3).unwrap(),b"bad current");bad.stop();
 }
