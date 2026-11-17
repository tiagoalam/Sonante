use super::*;
struct Temp(PathBuf);
impl Temp{fn new()->Self{let mut id=[0;12];getrandom::fill(&mut id).unwrap();let p=std::env::temp_dir().join(format!("sonante-own-setup-{id:x?}"));fs::create_dir(&p).unwrap();Self(p)}fn path(&self)->PathBuf{self.0.join("folders.json")}}
impl Drop for Temp{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
#[test]fn fresh_pending_survives_restart_partial_account_setup_and_finishes_without_a_scan_gate(){
 let t=Temp::new();let library=Library::new(t.path());assert!(library.first_run().unwrap());assert!(!t.path().exists());library.prepare_first_run(false).unwrap();
 let pending=fs::read(t.path()).unwrap();assert_eq!(serde_json::from_slice::<serde_json::Value>(&pending).unwrap()["first_run"],true);library.stop();
 let library=Library::new(t.path());library.prepare_first_run(true).unwrap();assert!(library.first_run().unwrap());assert_eq!(fs::read(t.path()).unwrap(),pending);
 library.configure(None,Sources{local:false,navidrome:true,plex:false}).unwrap();assert!(library.first_run().unwrap());let before=library.status().unwrap();
 library.complete_first_run().unwrap();assert!(!library.first_run().unwrap());let after=library.status().unwrap();assert_eq!(before.epoch,after.epoch);assert_eq!(before.revision,after.revision);library.stop();
 let reopened=Library::new(t.path());assert!(!reopened.first_run().unwrap());let completed=fs::read(t.path()).unwrap();reopened.complete_first_run().unwrap();assert_eq!(fs::read(t.path()).unwrap(),completed);reopened.stop();
}
#[test]fn legacy_config_and_existing_profile_are_preserved_read_only_without_reopening_the_wizard(){
 for version in [2,3]{let t=Temp::new();let bytes=format!("{{\"version\":{version},\"roots\":[],\"sources\":{{\"local\":false,\"navidrome\":true}}}}");fs::write(t.path(),&bytes).unwrap();let library=Library::new(t.path());library.prepare_first_run(false).unwrap();assert!(!library.first_run().unwrap());library.complete_first_run().unwrap();assert_eq!(fs::read(t.path()).unwrap(),bytes.as_bytes());library.stop();}
 let t=Temp::new();let library=Library::new(t.path());library.prepare_first_run(true).unwrap();assert!(!library.first_run().unwrap());assert!(!t.path().exists());library.stop();
}
#[test]fn pending_failure_or_rollback_never_finishes_setup_or_resets_current_records(){
 let t=Temp::new();let library=Library::new(t.path());library.prepare_first_run(false).unwrap();let before=library.preferences().unwrap();let mut calls=0;
 let result=super::super::config_transaction::apply(before,None,Sources{local:true,navidrome:true,plex:false},|r,s|library.configure(r,s).map(|_|()),|_|{calls+=1;if calls==1{Err("NAVIDROME_STORAGE")}else{Ok(())}},||true);
 assert_eq!(result,Err("NAVIDROME_STORAGE"));assert!(library.first_run().unwrap());assert_eq!(serde_json::from_slice::<serde_json::Value>(&fs::read(t.path()).unwrap()).unwrap()["first_run"],true);
 fs::write(t.path(),b"corrupt current own fixture").unwrap();assert!(library.complete_first_run().is_err());assert_eq!(fs::read(t.path()).unwrap(),b"corrupt current own fixture");assert!(library.first_run().unwrap());library.stop();
}
#[test]fn first_pending_publication_is_create_only_and_completion_never_recreates_absent_records(){
 let t=Temp::new();let library=Library::new(t.path());fs::write(t.path(),b"external own fixture").unwrap();assert!(library.prepare_first_run(false).is_err());assert_eq!(fs::read(t.path()).unwrap(),b"external own fixture");library.stop();
 let t=Temp::new();let library=Library::new(t.path());library.prepare_first_run(false).unwrap();fs::remove_file(t.path()).unwrap();assert!(library.complete_first_run().is_err());assert!(!t.path().exists());assert!(library.first_run().unwrap());library.stop();
}
#[test]fn invalid_pending_type_or_link_never_turns_into_new_profile_or_overwrites_target(){
 let t=Temp::new();let bytes=br#"{"version":3,"first_run":"bad","roots":[]}"#;fs::write(t.path(),bytes).unwrap();let library=Library::new(t.path());assert!(library.first_run().is_err());assert!(library.complete_first_run().is_err());assert_eq!(fs::read(t.path()).unwrap(),bytes);library.stop();
 #[cfg(unix)]{let t=Temp::new();let library=Library::new(t.path());library.prepare_first_run(false).unwrap();let target=t.0.join("target");fs::write(&target,b"target fixture preserved").unwrap();fs::remove_file(t.path()).unwrap();std::os::unix::fs::symlink(&target,t.path()).unwrap();assert!(library.complete_first_run().is_err());assert_eq!(fs::read(&target).unwrap(),b"target fixture preserved");library.stop();}
}
#[test]fn setup_source_writes_never_repair_changed_corrupt_or_deleted_current_settings(){
 for bad in [b"corrupt own fixture".to_vec(),br#"{"version":3,"first_run":true,"roots":[],"sources":{"local":false,"navidrome":true}}"#.to_vec()]{
  let t=Temp::new();let library=Library::new(t.path());library.prepare_first_run(false).unwrap();fs::write(t.path(),&bad).unwrap();assert!(library.configure(None,Sources::default()).is_err());assert_eq!(fs::read(t.path()).unwrap(),bad);assert!(library.first_run().unwrap());library.stop();
 }
 let t=Temp::new();let library=Library::new(t.path());library.prepare_first_run(false).unwrap();fs::remove_file(t.path()).unwrap();assert!(library.configure(None,Sources::default()).is_err());assert!(!t.path().exists());library.stop();
}
#[cfg(unix)]#[test]fn setup_does_not_publish_into_a_linked_parent_or_restore_from_a_linked_current_record(){
 let t=Temp::new();let own=t.0.join("own-target");fs::create_dir(&own).unwrap();let linked_parent=t.0.join("linked");std::os::unix::fs::symlink(&own,&linked_parent).unwrap();let library=Library::new(linked_parent.join("folders.json"));assert!(library.prepare_first_run(false).is_err());assert!(!own.join("folders.json").exists());library.stop();
 let target=t.0.join("target.json");let bytes=br#"{"version":3,"first_run":true,"roots":[]}"#;fs::write(&target,bytes).unwrap();std::os::unix::fs::symlink(&target,t.path()).unwrap();let library=Library::new(t.path());assert!(library.first_run().is_err());assert!(library.configure(None,Sources::default()).is_err());assert_eq!(fs::read(&target).unwrap(),bytes);library.stop();
}
#[cfg(unix)]#[test]fn legacy_migration_never_creates_a_current_record_through_a_linked_parent(){
 let t=Temp::new();let target=t.0.join("target-folder");fs::create_dir(&target).unwrap();let parent=t.0.join("linked-parent");std::os::unix::fs::symlink(&target,&parent).unwrap();let legacy=t.0.join("legacy.json");let bytes=br#"{"version":2,"roots":[],"sources":{"local":false,"navidrome":true}}"#;fs::write(&legacy,bytes).unwrap();let library=Library::with_legacy(parent.join("folders.json"),legacy.clone());assert!(!library.first_run().unwrap());assert!(library.configure(None,Sources{local:false,navidrome:true,plex:false}).is_err());assert!(!target.join("folders.json").exists());assert_eq!(fs::read(&legacy).unwrap(),bytes);library.stop();
}
#[test]fn both_windows_feature_preferences_participate_in_real_store_host_rollback(){
 let t=Temp::new();let l=Library::new(t.path());l.prepare_first_run(false).unwrap();let before=Sources{local:false,navidrome:true,plex:false};l.configure_features(None,before,Some(true),Some(true)).unwrap();l.complete_first_run().unwrap();let mut attempts=0;
 let result=super::super::config_transaction::apply_all_options((Vec::new(),before,(true,true)),None,Sources{plex:true,..before},(false,false),false,|r,s,(beta,art)|l.configure_features(r,s,Some(beta),Some(art)).map(|_|()),|_|panic!("unchanged Nav"),|_|{attempts+=1;if attempts==1{Err("PLEX_STORAGE")}else{Ok(())}},||true);
 assert_eq!(result,Err("PLEX_STORAGE"));assert_eq!(attempts,2);assert!(l.experimental_features().unwrap());assert!(l.online_artwork_enabled().unwrap());assert_eq!(l.sources().unwrap(),before);
 let value:serde_json::Value=serde_json::from_slice(&fs::read(t.path()).unwrap()).unwrap();assert_eq!(value["experimental_features"],true);assert_eq!(value["online_artwork_enabled"],true);l.stop();let reopened=Library::new(t.path());assert!(reopened.experimental_features().unwrap());assert!(reopened.online_artwork_enabled().unwrap());reopened.stop();
}
