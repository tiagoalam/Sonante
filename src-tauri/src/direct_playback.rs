//! Per-app binding between the committed route choice and owned MPD session.
//! Choice is durable policy; gain evidence remains per PID/session/readback.
use crate::{audio::AudioEngine,config::{AppConfig,DirectVolumeChoice},config_save_operation::Coordinator,direct_volume::{self,Session}};
use std::sync::{Arc,RwLock};
const NOT_ADMITTED:&str="DIRECT_PLAY_NOT_ADMITTED:";
#[derive(Clone)]
struct Owned {pid:u32,session:Arc<Session>,context:DirectVolumeChoice}
#[derive(Default)]
struct Binding {available:bool,direct:bool,owned:Option<Owned>,committed:Option<DirectVolumeChoice>}
pub(crate) struct Gate {binding:RwLock<Binding>,operations:Arc<Coordinator>}
impl Gate {
    pub(crate) fn new(operations:Arc<Coordinator>)->Self{Self{binding:RwLock::new(Binding::default()),operations}}
    pub(crate) fn bind(&self,cfg:&AppConfig,pid:u32,session:Option<Arc<Session>>) {
        let mut b=self.binding.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        b.available=true;b.direct=cfg.direct_context().is_some();
        b.owned=session.zip(cfg.direct_context()).map(|(session,context)|Owned{pid,session,context});
    }
    pub(crate) fn set_available(&self,value:bool){self.binding.write().unwrap_or_else(std::sync::PoisonError::into_inner).available=value;}
    pub(crate) fn clear(&self) {
        let mut b=self.binding.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        b.available=false;b.owned=None;
    }
    pub(crate) fn commit_choice(&self,cfg:&AppConfig) {
        self.binding.write().unwrap_or_else(std::sync::PoisonError::into_inner).committed=cfg.confirmed_direct_choice();
    }
    pub(crate) fn forget_choice(&self) {
        self.binding.write().unwrap_or_else(std::sync::PoisonError::into_inner).committed=None;
    }
    pub(crate) fn require_choice(&self)->Result<(),String> {
        let b=self.binding.read().map_err(|_|direct_volume::STALE)?;
        let owned=b.owned.as_ref().ok_or(direct_volume::STALE)?;
        if b.available&&b.committed.as_ref()==Some(&owned.context){Ok(())}else{Err(direct_volume::CONFIRM.into())}
    }
    pub(crate) fn send(&self,socket:&str,command:&str)->Result<Vec<String>,String> {
        self.send_with(command,|pid,cmd|match pid{Some(pid)=>AudioEngine::send_owned_command_to_socket(socket,pid,cmd),None=>AudioEngine::send_command_to_socket(socket,cmd)})
    }
    pub(crate) fn send_with(&self,command:&str,mut io:impl FnMut(Option<u32>,&str)->Result<Vec<String>,String>)->Result<Vec<String>,String> {
        if !may_start(command){return io(None,command);}
        // This lease belongs to this exact coordinator's explicit Check. Hold
        // it through gain readback AND the MPD command, fencing publication and
        // recovery at the effect boundary without another exclusive reservation.
        let check = self.operations.manual_check_playback.control()
            .map_err(|error| format!("{NOT_ADMITTED}{error}"))?;
        // Shared/EQ already owns its lifecycle reservation during restoration.
        // Leave that established path unchanged; only Direct needs this guard.
        let shared={let b=self.binding.read().map_err(|_|direct_volume::STALE)?;!b.direct && b.available};
        if shared {
            if !check.current() || self.operations.shutdown_requested() { return Err(format!("{NOT_ADMITTED}PLAYBACK_SUPERSEDED")); }
            return io(None,command);
        }
        let _permit = if check.is_check() { None } else {
            Some(self.operations.try_begin_background().map_err(|_|format!("{NOT_ADMITTED}CONFIG_SAVE_BUSY"))?)
        };
        self.send_admitted_with(command, &mut io, || check.current() && !self.operations.shutdown_requested(),false)
    }
    // Fixed resume effect for the existing Save owner; never a public bypass.
    pub(crate) fn start_saved_route(&self,socket:&str,owner:&crate::config_save_operation::Transaction<'_>,current:impl Fn()->bool,index:usize,elapsed:f64,verify:impl FnOnce()->Result<(),String>)->Result<Vec<String>,String>{
        self.start_saved_route_with(owner,current,index,elapsed,verify,|pid,cmd|match pid{Some(pid)=>AudioEngine::send_owned_command_to_socket(socket,pid,cmd),None=>AudioEngine::send_command_to_socket(socket,cmd)})
    }
    fn start_saved_route_with(&self,owner:&crate::config_save_operation::Transaction<'_>,current:impl Fn()->bool,index:usize,elapsed:f64,verify:impl FnOnce()->Result<(),String>,mut io:impl FnMut(Option<u32>,&str)->Result<Vec<String>,String>)->Result<Vec<String>,String>{
        if !owner.belongs_to(&self.operations)||!elapsed.is_finite()||elapsed<0. {return Err(format!("{NOT_ADMITTED}OUTPUT_SAVE_RESUME_INVALID"));}
        let command=format!("seek {index} {elapsed:.3}");let mut verify=Some(verify);
        self.send_admitted_with(&command,&mut |pid,cmd|{
            if cmd==command {
                verify.take().ok_or("OUTPUT_SAVE_RESUME_ALREADY_SENT")?()?;
                if !current()||self.operations.shutdown_requested(){return Err("PLAYBACK_SUPERSEDED".into());}
            }
            io(pid,cmd)
        },||current()&&!self.operations.shutdown_requested(),true)
    }
    fn send_admitted_with(&self,command:&str,io:&mut impl FnMut(Option<u32>,&str)->Result<Vec<String>,String>,current:impl Fn()->bool,explicit_resume:bool)->Result<Vec<String>,String>{
        let admission=(||{
            if !current(){return Err("PLAYBACK_SUPERSEDED".into());}
            let (direct,available,owned,choice)={let b=self.binding.read().map_err(|_|direct_volume::STALE)?;(b.direct,b.available,b.owned.clone(),b.committed.clone())};
            if !available{return Err(direct_volume::STALE.into());}
            if !direct{return Ok(None);}
            let owned=owned.ok_or(direct_volume::STALE)?;
            if choice.as_ref()!=Some(&owned.context){return Err(direct_volume::CONFIRM.into());}
            owned.session.before_explicit_play(explicit_resume||command.lines().any(|line|matches!(line.split_whitespace().next(),Some("play"|"playid"|"next"|"previous"))||line=="pause 0"||line=="pause"),|cmd|{
                if !current(){return Err("PLAYBACK_SUPERSEDED".into());}
                io(Some(owned.pid),cmd)
            })?;
            self.require_choice()?;owned.session.check_active()?;
            if !current(){return Err("PLAYBACK_SUPERSEDED".into());}
            Ok(Some(owned))
        })();
        let owned=admission.map_err(|e:String|format!("{NOT_ADMITTED}{e}"))?;
        if !current(){return Err(format!("{NOT_ADMITTED}PLAYBACK_SUPERSEDED"));}
        io(owned.map(|o|o.pid),command)
    }

}
pub(crate) fn is_admission_refusal(error:&str)->bool{error.starts_with(NOT_ADMITTED)}
fn may_start(command:&str)->bool {
    command.lines().any(|line|matches!(line.split_whitespace().next(),Some("play"|"playid"|"next"|"previous"|"seek"|"seekid"|"seekcur"))||line=="pause 0"||line=="pause")
}

#[cfg(test)]
mod tests {
 use super::*;
 struct Mpd{volume:u32,state:&'static str,commands:Vec<String>,fail_gain:bool,readback_bad:bool}
 impl Mpd {
  fn new()->Self{Self{volume:100,state:"stop",commands:vec![],fail_gain:false,readback_bad:false}}
  fn io(&mut self,cmd:&str)->Result<Vec<String>,String>{
   self.commands.push(cmd.into());
   for line in cmd.lines(){match line{
    "setvol 0"=>self.volume=0,"setvol 100"=>{if self.fail_gain{return Err("lost reply".into());}self.volume=if self.readback_bad{41}else{100};},
    line if line.starts_with("seek ")=>self.state="play",
    "stop"=>self.state="stop","pause 1"=>self.state="pause","pause 0"|"play"|"play 0"|"playid 2"=>self.state="play",_=>{}
   }}
   Ok(if cmd=="status"||cmd.contains("\nstatus\n"){vec![format!("volume: {}",self.volume),format!("state: {}",self.state),"songid: 7".into(),"playlist: 2".into()]}else{vec![]})
  }
 }
 fn fixture(confirmed:bool,muted:bool)->(Gate,Arc<Session>,Mpd,AppConfig){
  let mut m=Mpd::new();let session=Session::initialize(|c|m.io(c)).unwrap();
  let mut cfg=AppConfig::default();cfg.first_run=false;cfg.audio_output_type="alsa".into();cfg.alsa_device="hw:CARD=Fixture,DEV=0".into();cfg.direct_muted=muted;
  if confirmed{cfg.direct_volume_choice=cfg.direct_context();}
  if muted{session.set_mute(true,false,&session.token(),|c|m.io(c)).unwrap();}
  let gate=Gate::new(Arc::new(Coordinator::new()));gate.commit_choice(&cfg);gate.bind(&cfg,7,Some(session.clone()));m.commands.clear();(gate,session,m,cfg)
 }
 #[test]
 fn manual_check_preparation_and_bound_catalog_allow_explicit_direct_transport_without_releasing_writer() {
  let(g,_,mut m,_)=fixture(true,false);
  let check=g.operations.manual_check_playback.clone();let owner=g.operations.begin().unwrap();
  let session=check.begin_preparing(&owner,&g.operations).unwrap();
  assert!(check.album(|files|Ok(files.is_none())).unwrap());
  g.send_with("play",|_,cmd|m.io(cmd)).unwrap();
  assert_eq!(m.volume,100);assert!(g.operations.try_begin_background().is_err());assert!(g.operations.reserve().is_err());
  session.bind_catalog(&["file: Music/track.flac".into()].into()).unwrap();
  for command in ["next","previous","play 0","seekcur 2"] {g.send_with(command,|_,cmd|m.io(cmd)).unwrap();}
  assert_eq!(m.commands.iter().filter(|c|c.contains("setvol 100")).count(),1);
  assert!(session.bind_catalog(&Default::default()).is_err());
  session.confirm();
 }
 #[test]
 fn manual_check_never_overrides_route_choice_mute_health_session_or_shutdown() {
  for mode in 0..5 {
   let(g,volume,mut m,_)=fixture(mode!=0,false);let check=g.operations.manual_check_playback.clone();let owner=g.operations.begin().unwrap();
   let session=check.begin_preparing(&owner,&g.operations).unwrap();
   match mode {1=>g.set_available(false),2=>volume.invalidate(),3=>g.operations.request_shutdown(||{}),4=>session.revoke(),_=>{}}
   assert!(g.send_with("play",|_,cmd|m.io(cmd)).is_err());
   assert!(!m.commands.iter().any(|c|c=="play"||c.contains("setvol 100")));
  }
  let(g,_,mut m,_)=fixture(true,true);let check=g.operations.manual_check_playback.clone();let owner=g.operations.begin().unwrap();
  let _session=check.begin_preparing(&owner,&g.operations).unwrap();g.send_with("play",|_,cmd|m.io(cmd)).unwrap();
  assert_eq!(m.volume,0);assert!(!m.commands.iter().any(|c|c.contains("setvol 100")));
 }
 #[test]
 fn manual_check_keeps_shared_commands_without_gain_and_revokes_them_on_failure() {
  let(g,_,mut m,mut config)=fixture(true,false);config.audio_output_type="pipewire".into();config.alsa_device="default".into();config.direct_volume_choice=None;
  g.bind(&config,7,None);g.commit_choice(&config);
  let check=g.operations.manual_check_playback.clone();let owner=g.operations.begin().unwrap();let session=check.begin_preparing(&owner,&g.operations).unwrap();
  g.send_with("play",|pid,command|{assert_eq!(pid,None);m.io(command)}).unwrap();assert_eq!(m.commands,vec!["play"]);
  session.revoke();assert!(g.send_with("next",|_,cmd|m.io(cmd)).is_err());assert_eq!(m.commands,vec!["play"]);
 }
 #[test]
 fn manual_check_revocation_during_direct_gain_readback_drains_without_sending_play() {
  use std::{thread,sync::mpsc,time::{Duration,Instant}};
  let(g,_,mut m,_)=fixture(true,false);let check=g.operations.manual_check_playback.clone();let owner=g.operations.begin().unwrap();
  let session=check.begin_preparing(&owner,&g.operations).unwrap();
  thread::scope(|scope| {
   let(start,started)=mpsc::channel();let(done,finished)=mpsc::channel();
   let worker=scope.spawn(move||{started.recv_timeout(Duration::from_secs(1)).unwrap();session.revoke();done.send(()).unwrap();session});
   let result=g.send_with("play",|_,cmd|{
    let result=m.io(cmd);
    if cmd.contains("setvol 100") {
     start.send(()).unwrap();let deadline=Instant::now()+Duration::from_secs(1);
     while check.active()&&Instant::now()<deadline {thread::yield_now();}
     assert!(!check.active());assert!(finished.try_recv().is_err());
    }
    result
   });
   assert!(result.is_err());assert!(!m.commands.iter().any(|c|c=="play"));
   finished.recv_timeout(Duration::from_secs(1)).unwrap();drop(worker.join().unwrap());
  });
 }

 #[test]fn unconfirmed_legacy_and_mpris_play_have_no_gain_or_transport_effects(){
  let(g,_,mut m,_)=fixture(false,false);
  for cmd in["play","play 0","playid 2","pause 0","next","previous","seekcur 3","command_list_begin\nclear\nadd x\nplay 0\ncommand_list_end"]{
   assert!(g.send_with(cmd,|_,c|m.io(c)).unwrap_err().contains(direct_volume::CONFIRM));
  }assert!(m.commands.is_empty());assert_eq!(m.volume,0);
 }
 #[test]fn confirmed_reopening_uses_real_zero_to_unity_before_only_requested_play(){
  let(g,_,mut m,_)=fixture(true,false);g.send_with("play",|pid,c|{assert_eq!(pid,Some(7));m.io(c)}).unwrap();
  assert_eq!(m.state,"play");assert_eq!(m.volume,100);assert_eq!(m.commands.last().unwrap(),"play");
  let count=m.commands.iter().filter(|c|c.contains("setvol 100")).count();assert_eq!(count,1);
  g.send_with("next",|_,c|m.io(c)).unwrap();assert_eq!(m.commands.iter().filter(|c|c.contains("setvol 100")).count(),1);
 }
 #[test]fn intentional_mute_survives_play_track_changes_and_reopening(){
  let(g,_,mut m,_)=fixture(true,true);for cmd in["play","next","previous","play 0"]{g.send_with(cmd,|_,c|m.io(c)).unwrap();assert_eq!(m.volume,0);}
  assert!(!m.commands.iter().any(|c|c.contains("setvol 100")));
  let(g,_,mut m,_)=fixture(true,true);g.send_with("play",|_,c|m.io(c)).unwrap();assert_eq!(m.volume,0);
 }
 #[test]fn state_or_context_change_refuses_without_automatic_repair(){
  for playing in[false,true]{let(g,_,mut m,_)=fixture(true,false);m.state=if playing{"play"}else{"pause"};if !playing{m.volume=43;}
   assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert!(!m.commands.iter().any(|c|c.contains("setvol 100")));}
  let(g,_,mut m,mut cfg)=fixture(true,false);cfg.alsa_device="hw:CARD=Other,DEV=0".into();g.commit_choice(&cfg);
  assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert!(m.commands.is_empty());
 }
 #[test]fn ack_lost_or_bad_unity_readback_never_sends_play_or_retries_gain(){
  for bad in[false,true]{let(g,_,mut m,_)=fixture(true,false);m.fail_gain=!bad;m.readback_bad=bad;
   assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert!(!m.commands.iter().any(|c|c=="play"));
   let count=m.commands.iter().filter(|c|c.contains("setvol 100")).count();assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert_eq!(m.commands.iter().filter(|c|c.contains("setvol 100")).count(),count);
  }
 }
 #[test]fn shared_eq_restoration_keeps_its_existing_lifecycle_owner_and_seek_cannot_prepare_direct(){
  let(g,_,mut m,_)=fixture(true,false);let permit=g.operations.try_begin_background().unwrap();g.bind(&AppConfig::default(),7,None);g.send_with("pause 0",|pid,c|{assert!(pid.is_none());g.set_available(true);m.io(c)}).unwrap();drop(permit);
  let(g,_,mut m,_)=fixture(true,false);assert!(g.send_with("seekcur 2",|_,c|m.io(c)).is_err());assert_eq!(m.volume,0);assert!(!m.commands.iter().any(|c|c.contains("setvol 100")));
 }
 #[test]fn unavailable_health_does_not_allow_gain_until_same_owned_route_is_available(){
  let(g,_,mut m,_)=fixture(true,false);g.set_available(false);assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert!(m.commands.is_empty());g.set_available(true);g.send_with("play",|_,c|m.io(c)).unwrap();assert_eq!(m.volume,100);
 }
 #[test]fn save_shutdown_and_stale_owned_session_fence_gain_and_play(){
  let(g,s,mut m,_)=fixture(true,false);let permit=g.operations.try_begin_background().unwrap();assert!(g.send_with("play",|_,c|m.io(c)).is_err());drop(permit);assert!(m.commands.is_empty());
  s.invalidate();assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert!(m.commands.is_empty());
  let(g,_,mut m,_)=fixture(true,false);g.operations.request_shutdown(||{});assert!(g.send_with("play",|_,c|m.io(c)).is_err());assert!(m.commands.is_empty());
 }
 #[test]fn shutdown_during_gain_readback_cannot_issue_delayed_play(){
  let(g,_,mut m,_)=fixture(true,false);
  assert!(g.send_with("play",|_,c|{let r=m.io(c);if c.contains("setvol 100"){g.operations.request_shutdown(||{});}r}).is_err());
  assert!(!m.commands.iter().any(|c|c=="play"));assert_ne!(m.state,"play");
 }
 #[test]fn route_save_resume_uses_same_owner_and_observed_unity_or_mute(){
  for muted in [false,true]{let(g,_,mut m,_)=fixture(true,muted);m.state="pause";
   let owner=g.operations.begin().unwrap();g.start_saved_route_with(&owner,||true,0,17.,||Ok(()),|_,c|m.io(c)).unwrap();
   assert_eq!(m.state,"play");assert_eq!(m.volume,if muted{0}else{100});assert_eq!(m.commands.last().unwrap(),"seek 0 17.000");
   assert_eq!(m.commands.iter().filter(|c|c.contains("setvol 100")).count(),if muted{0}else{1});
  }
 }
 #[test]fn route_save_resume_rejects_other_owner_cancel_shutdown_and_bad_gain(){
  let(g,_,mut m,_)=fixture(true,false);let other=Coordinator::new();let owner=other.begin().unwrap();
  assert!(g.start_saved_route_with(&owner,||true,0,17.,||Ok(()),|_,c|m.io(c)).is_err());assert!(m.commands.is_empty());drop(owner);
  for mode in 0..5{let(g,_,mut m,_)=fixture(mode!=4,false);m.state="pause";let owner=g.operations.begin().unwrap();
   if mode==1{g.operations.request_shutdown(||{});}if mode==2{m.fail_gain=true;}if mode==3{m.readback_bad=true;}
   assert!(g.start_saved_route_with(&owner,||mode!=0,0,17.,||Ok(()),|_,c|m.io(c)).is_err());assert!(!m.commands.iter().any(|c|c.starts_with("seek ")));
  }
 }
 #[test]fn route_save_intervention_during_gain_readback_never_sends_resume(){
  use std::cell::Cell;let(g,_,mut m,_)=fixture(true,false);m.state="pause";let owner=g.operations.begin().unwrap();let current=Cell::new(true);
  assert!(g.start_saved_route_with(&owner,||current.get(),0,17.,||Ok(()),|_,c|{let r=m.io(c);if c.contains("setvol 100"){current.set(false);}r}).is_err());
  assert!(!m.commands.iter().any(|c|c.starts_with("seek ")));assert_ne!(m.state,"play");
 }
 #[test]fn route_save_shared_resume_has_no_gain_write_and_requires_current_owner(){
  let(g,_,mut m,mut cfg)=fixture(true,false);cfg.audio_output_type="pipewire".into();cfg.alsa_device="default".into();cfg.direct_volume_choice=None;g.bind(&cfg,7,None);g.commit_choice(&cfg);m.state="pause";
  let owner=g.operations.begin().unwrap();g.start_saved_route_with(&owner,||true,0,17.,||Ok(()),|pid,c|{assert_eq!(pid,None);m.io(c)}).unwrap();
  assert_eq!(m.commands,vec!["seek 0 17.000"]);
 }

 #[test]fn route_save_last_queue_or_mute_validation_refuses_after_gain_without_transport(){
  let(g,_,mut m,_)=fixture(true,false);m.state="stop";let owner=g.operations.begin().unwrap();
  assert!(g.start_saved_route_with(&owner,||true,0,17.,||Err("OUTPUT_SAVE_CONTEXT_CHANGED".into()),|_,c|m.io(c)).is_err());
  assert!(!m.commands.iter().any(|c|c.starts_with("seek ")));assert_ne!(m.state,"play");
 }

}
