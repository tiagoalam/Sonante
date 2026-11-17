//! Existing-profile detection only. New pending setup is persisted in the same
//! native folders/sources record; startup never launches an engine/listener.
use std::{fs,path::Path};
const RECORDS:&[&str]=&[
 "library-v3/folders.json","library-v2/folders.json","library-v1/folder.json",
 "queue-v4/queue.json","queue-v3/queue.json","queue-v2/queue.json","queue-v1/queue.json",
 "user-data/windows-navidrome-v1.json","user-data/windows-plex-account-v1.json",
 "user-data/windows-audio-v1.json","user-data/playlists.json","user-data/radio-favorites.json",
 "user-data/windows-favorites-v2.json","user-data/windows-favorites-v1.json",
 "user-data/windows-listening-history-v2.json","user-data/windows-listening-history-v1.json",
 "user-data/windows-smart-playlists-v2.json","user-data/windows-smart-playlists-v1.json",
 "user-data/windows-artwork-progress-v1.json","user-data/equalizer-shared-v1.json",
];
pub(crate) fn existing_profile(root:&Path)->Result<bool,&'static str>{
 for name in RECORDS {
  match fs::symlink_metadata(root.join(name)){
   Ok(_)=>return Ok(true), // Empty/corrupt/linked stays authoritative; never repair.
   Err(e)if e.kind()==std::io::ErrorKind::NotFound=>{},
   Err(_)=>return Err("windows_setup_unavailable"),
  }
 }
 Ok(false)
}
#[cfg(test)]mod tests {
 use super::*;
 struct Temp(std::path::PathBuf);
 impl Temp{fn new()->Self{let mut id=[0;12];getrandom::fill(&mut id).unwrap();let p=std::env::temp_dir().join(format!("sonante-own-first-run-{id:x?}"));fs::create_dir(&p).unwrap();Self(p)}}
 impl Drop for Temp{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
 #[test]fn empty_directories_locks_and_runtime_cache_do_not_suppress_first_use(){
  let t=Temp::new();fs::create_dir(t.0.join("user-data")).unwrap();fs::create_dir(t.0.join("mpd-pilot")).unwrap();fs::write(t.0.join("windows-profile.lock"),b"own fixture").unwrap();assert!(!existing_profile(&t.0).unwrap());
 }
 #[test]fn every_previous_record_counts_without_reading_or_mutating_its_content(){
  for n in RECORDS{let t=Temp::new();let p=t.0.join(n);fs::create_dir_all(p.parent().unwrap()).unwrap();let bytes=b"old or corrupt own fixture, must never be overwritten";fs::write(&p,bytes).unwrap();assert!(existing_profile(&t.0).unwrap());assert_eq!(fs::read(&p).unwrap(),bytes);}
 }
 #[test]fn non_directory_intermediate_path_is_an_error_not_fresh_profile(){
  let t=Temp::new();fs::write(t.0.join("library-v3"),b"preserved").unwrap();assert_eq!(existing_profile(&t.0),Err("windows_setup_unavailable"));assert_eq!(fs::read(t.0.join("library-v3")).unwrap(),b"preserved");
 }
}
