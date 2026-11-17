//! Per-occurrence admission, shared by local and remote sessions.
//! No engine or MPD commands are issued while preparing these leases.
use crate::windows_file_info::FileIdentity;
use super::library::Selection;
use super::protocol::Result;
use std::sync::Arc;
pub(crate) enum MediaLease {
    Local(super::media_server::Lease),
    Remote(Arc<dyn super::library::RemoteLease>),
    Upnp(Arc<crate::windows_upnp::media::Lease>),
}
impl MediaLease {
    pub(crate) fn healthy(&self)->bool { match self {Self::Local(lease)=>lease.healthy(),Self::Upnp(lease)=>lease.healthy(),Self::Remote(lease)=>lease.healthy()} }
    pub(crate) fn failure_code(&self)->&'static str {match self {Self::Local(lease)=>lease.failure_code(),Self::Upnp(_)=>"UPNP_SESSION_EXPIRED",Self::Remote(lease)=>match lease.source(){super::queue_v3::Source::Plex{..}=>"PLEX_UNAVAILABLE",_=>"NAVIDROME_UNAVAILABLE"}}}
    pub(crate) fn health_signal(&self)->Result<super::liveness::Signal>{match self{
        Self::Local(lease)=>Ok(lease.health_signal()),
        Self::Upnp(lease)=>Ok(super::liveness::Signal::remote(lease.signal())),
        Self::Remote(lease)=>lease.health_signal().map(super::liveness::Signal::remote).ok_or("EQ_RUNTIME_START_FAILED"),
    }}
    pub(crate) fn identity(&self)->Option<FileIdentity> {match self {Self::Local(lease)=>lease.identity(),Self::Remote(_)|Self::Upnp(_)=>None}}
}
pub(crate) fn watch(leases:&[MediaLease])->Result<super::liveness::Watch>{
    super::liveness::Watch::new(leases.iter().map(MediaLease::health_signal).collect::<Result<Vec<_>>>()?)
}
pub(crate) fn admit_tracks(server: Option<&super::media_server::Server>, selected: &mut Selection, current: impl Fn() -> Result<()>) -> Result<(Vec<MediaLease>, Vec<String>)> {
    let mut leases = Vec::new(); let mut uris = Vec::new();
    for track in &mut selected.tracks {
        current()?;
        if let Some(source)=&track.remote_source {
            source.validate()?;
            if track.radio.is_some() || !track.path.as_os_str().is_empty() || track.media_root.is_some() {return Err("NAVIDROME_INVALID");}
            let lease=track.remote_lease.clone().ok_or("NAVIDROME_CONTEXT_CHANGED")?;
            if lease.source()!=source || !lease.healthy(){return Err("NAVIDROME_CONTEXT_CHANGED");}
            track.remote_connection_id=Some(lease.connection_id().to_owned());
            uris.push(lease.uri().to_owned());leases.push(MediaLease::Remote(lease));
        } else {
            let root=track.media_root.as_ref().unwrap_or(&selected.root).to_path_buf();
            let root=crate::windows_io::read(crate::windows_io::READ_TIMEOUT,move|job|{
                if !job.current(){return Err("file_unavailable");}
                let root=crate::windows_file_info::normalize_root(&root).map_err(|_|"file_unavailable")?;
                if !job.current(){return Err("file_unavailable");}
                let root=root.canonicalize().map_err(|_|"file_unavailable")?;
                if !job.current(){return Err("file_unavailable");}Ok(root)
            }).map_err(|e|if e==crate::windows_io::Error::Busy{"engine_busy"}else{"file_unavailable"})??;
            current()?;
            // Keep the exact admitted handle boundary of local playback.
            let lease=server.ok_or("windows_media_unavailable")?.local(&root,&track.path)?;
            current()?;
            uris.push(lease.url().to_owned());leases.push(MediaLease::Local(lease));track.media_root=Some(root);
        }
    }
    selected.root=selected.tracks.iter().find_map(|track|track.media_root.clone()).unwrap_or_default();
    Ok((leases,uris))
}

pub(crate) fn enabled(tracks:&[super::library::Track],sources:super::library::Sources)->Result<()> {
    sources.validate()?;
    for track in tracks {
        if track.radio.is_some()||track.upnp.is_some(){continue;}
        match &track.remote_source {
            Some(super::queue_v3::Source::Navidrome{..}) if sources.navidrome=>{},
            Some(super::queue_v3::Source::Plex{..}) if sources.plex=>{},
            Some(super::queue_v3::Source::Plex{..})=>return Err("PLEX_CONTEXT_CHANGED"),
            Some(_) =>return Err("NAVIDROME_CONTEXT_CHANGED"),
            None if sources.local=>{},
            None=>return Err("windows_source_disabled"),
        }
    }
    Ok(())
}

/// Hold each distinct account fence once, in stable source/generation order.
/// Reacquiring a read lock for every duplicate could fail behind a pending writer.
pub(crate) fn command(leases:&[MediaLease],effect:&mut dyn FnMut()->Result<Vec<String>>)->Result<Vec<String>>{
 let mut remote=std::collections::BTreeMap::new();
 for lease in leases {if let MediaLease::Remote(lease)=lease{let kind=match lease.source(){super::queue_v3::Source::Navidrome{..}=>0,super::queue_v3::Source::Plex{..}=>1,_=>return Err("invalid_argument")};remote.entry((kind,lease.connection_id().to_owned())).or_insert_with(||lease.clone());}}
 fn run(leases:&[Arc<dyn super::library::RemoteLease>],effect:&mut dyn FnMut()->Result<Vec<String>>)->Result<Vec<String>>{match leases.split_first(){Some((first,rest))=>first.command(&mut||run(rest,effect)),None=>effect()}}
 run(&remote.into_values().collect::<Vec<_>>(),effect)
}
#[cfg(test)]mod mixed_fence_tests{
 use super::*;use crate::windows_mpd::library::RemoteLease;use crate::windows_mpd::queue_v3::Source;use std::sync::{Mutex,atomic::{AtomicBool,Ordering}};
 struct Fence{source:Source,id:&'static str,live:AtomicBool,held:Mutex<()>,calls:Mutex<usize>}
 #[test]fn local_admission_rechecks_intent_after_acquiring_the_last_file_lease(){
  use std::{cell::Cell,fs,time::{SystemTime,UNIX_EPOCH}};
  let root=std::env::temp_dir().join(format!("sonante-own-transport-cancel-{}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();let root=root.canonicalize().unwrap();
  let path=root.join("own.wav");fs::write(&path,b"own synthetic container bytes").unwrap();
  let mut track:super::super::library::Track=serde_json::from_value(serde_json::json!({"id":"own","title":"own","artist":"own","album":"own","disc":1,"number":1,"fileName":"own.wav"})).unwrap();
  track.path=path.clone();track.media_root=Some(root.clone());let mut selection=Selection{root:root.clone(),tracks:vec![track],start:0,album_id:None};
  let server=super::super::media_server::Server::new().unwrap();let calls=Cell::new(0);
  let result=admit_tracks(Some(&server),&mut selection,||{calls.set(calls.get()+1);if calls.get()==3{Err("PLAYBACK_SUPERSEDED")}else{Ok(())}});
  assert!(matches!(result,Err("PLAYBACK_SUPERSEDED")));assert_eq!(calls.get(),3);assert!(server.healthy());
  drop(server.local(&root,&path).unwrap());fs::rename(&path,root.join("retired.wav")).unwrap();drop(server);fs::remove_dir_all(root).unwrap();
 }
 impl RemoteLease for Fence{
  fn uri(&self)->&str{"fixture-no-listener"}fn source(&self)->&Source{&self.source}fn connection_id(&self)->&str{self.id}fn healthy(&self)->bool{self.live.load(Ordering::Acquire)}
  fn command(&self,effect:&mut dyn FnMut()->Result<Vec<String>>)->Result<Vec<String>>{let _held=self.held.try_lock().map_err(|_|"duplicate_fence")?;*self.calls.lock().unwrap()+=1;if !self.healthy(){return Err("retired_source");}effect()}
 }
 #[test]fn a_retired_second_source_prevents_mpd_effect_and_duplicate_rows_hold_each_account_once(){
  let nav=Arc::new(Fence{source:Source::Navidrome{account_id:"a".repeat(64),song_id:"one".into()},id:"same-generation",live:AtomicBool::new(true),held:Mutex::new(()),calls:Mutex::new(0)});
  let plex=Arc::new(Fence{source:Source::Plex{server_id:"fixture".into(),rating_key:"one".into(),part_key:"/library/parts/1/file.flac".into()},id:"same-generation",live:AtomicBool::new(false),held:Mutex::new(()),calls:Mutex::new(0)});
  let rows=vec![MediaLease::Remote(plex.clone()),MediaLease::Remote(nav.clone()),MediaLease::Remote(nav.clone()),MediaLease::Remote(plex.clone())];let mut effects=0;
  assert_eq!(command(&rows,&mut||{effects+=1;Ok(Vec::new())}),Err("retired_source"));assert_eq!(effects,0);assert_eq!(*nav.calls.lock().unwrap(),1);assert_eq!(*plex.calls.lock().unwrap(),1);
  plex.live.store(true,Ordering::Release);command(&rows,&mut||{effects+=1;Ok(Vec::new())}).unwrap();assert_eq!(effects,1);assert_eq!(*nav.calls.lock().unwrap(),2);assert_eq!(*plex.calls.lock().unwrap(),2);
 }
}
