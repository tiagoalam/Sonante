//! Pure, resource-free source signals for the PCM guardian. Probing never owns
//! media files, listeners, accounts, credentials or native process lifecycles.
use super::protocol::{self,Result};
use std::sync::{Arc,RwLock,atomic::{AtomicBool,Ordering}};
#[derive(Clone)]
pub(crate) struct Signal {live:Arc<AtomicBool>,failed:Option<Arc<AtomicBool>>,stopping:Option<Arc<AtomicBool>>}
impl Signal {
 pub(crate) fn local(live:Arc<AtomicBool>,failed:Arc<AtomicBool>,stopping:Arc<AtomicBool>)->Self{Self{live,failed:Some(failed),stopping:Some(stopping)}}
 pub(crate) fn remote(live:Arc<AtomicBool>)->Self{Self{live,failed:None,stopping:None}}
 fn healthy(&self)->bool{self.live.load(Ordering::Acquire)&&self.failed.as_ref().is_none_or(|v|!v.load(Ordering::Acquire))&&self.stopping.as_ref().is_none_or(|v|!v.load(Ordering::Acquire))}
}
#[derive(Clone)]
pub(crate) struct Watch(Arc<RwLock<Vec<Signal>>>);
impl Watch {
 pub(crate) fn new(signals:Vec<Signal>)->Result<Self>{
  if signals.len()>1000||signals.iter().any(|s|!s.healthy()){return Err("EQ_RUNTIME_START_FAILED");}
  Ok(Self(Arc::new(RwLock::new(signals))))
 }
 pub(crate) fn publish(&self,signals:Vec<Signal>)->Result<()>{
  if signals.len()>1000{return Err("EQ_CONTINUITY_LOST");}
  // Replacement drops only atomic flags. Never drop a file/account/Job while
  // holding this lock or from the guardian's read-only probe.
  *self.0.write().map_err(|_|"EQ_CONTINUITY_LOST")?=signals;Ok(())
 }
 pub(crate) fn healthy(&self)->bool{
  let Ok(signals)=self.0.read().map(|v|v.clone())else{return false};
  signals.iter().all(Signal::healthy)
 }
}
pub(crate) fn producer_status_healthy(lines:&[String])->bool{
 // Error text may contain a private URL: inspect only presence and discard it.
 matches!(protocol::field(lines,"error"),Ok(None))&&protocol::playback(lines).is_ok()
}
pub(crate) fn needs_current_probe(code:&str)->bool{
 matches!(code,"EQ_RUNTIME_UNCONFIRMED"|"EQ_CONTINUITY_LOST"|"EQ_RUNTIME_EXITED"|"EQ_ROLLBACK_FAILED"|"EQ_CURSOR_UNCONFIRMED")
}
pub(crate) fn retire_current(code:&str,uncertain:bool,eq:bool,current_ready:bool)->bool{
 uncertain||matches!(code,"PLAYBACK_QUEUE_UNCERTAIN"|"windows_config_uncertain"|"rollback_failed")||eq&&needs_current_probe(code)&&!current_ready
}
#[cfg(test)]mod tests{
 use super::*;
 fn live()->Arc<AtomicBool>{Arc::new(AtomicBool::new(true))}
 fn fault()->Arc<AtomicBool>{Arc::new(AtomicBool::new(false))}
 #[test]fn local_failure_and_remote_retirement_are_observed_without_metadata_or_command_effects(){
  let local=live();let failed=fault();let remote=live();let watch=Watch::new(vec![Signal::local(local.clone(),failed.clone(),fault()),Signal::remote(remote.clone())]).unwrap();assert!(watch.healthy());
  failed.store(true,Ordering::Release);assert!(!watch.healthy());failed.store(false,Ordering::Release);remote.store(false,Ordering::Release);assert!(!watch.healthy());remote.store(true,Ordering::Release);local.store(false,Ordering::Release);assert!(!watch.healthy());
 }
 #[test]fn publication_precedes_removal_of_old_lease_and_never_poisons_the_retained_queue(){
  let old=live();let retained=live();let watch=Watch::new(vec![Signal::remote(old.clone()),Signal::remote(retained.clone())]).unwrap();
  watch.publish(vec![Signal::remote(retained.clone())]).unwrap();old.store(false,Ordering::Release);assert!(watch.healthy());retained.store(false,Ordering::Release);assert!(!watch.healthy());
 }
 #[test]fn newly_added_occurrences_are_guarded_before_their_mpd_effect_and_empty_queue_is_valid(){
  let watch=Watch::new(Vec::new()).unwrap();assert!(watch.healthy());let added=live();watch.publish(vec![Signal::remote(added.clone())]).unwrap();added.store(false,Ordering::Release);assert!(!watch.healthy());watch.publish(Vec::new()).unwrap();assert!(watch.healthy());
 }
 #[test]fn only_atomic_flags_are_retained_not_a_source_owner_or_file_destructor(){
  struct Owner{signal:Arc<AtomicBool>,dropped:Arc<AtomicBool>}
  impl Drop for Owner{fn drop(&mut self){self.signal.store(false,Ordering::Release);self.dropped.store(true,Ordering::Release);}}
  let dropped=fault();let owner=Owner{signal:live(),dropped:dropped.clone()};let watch=Watch::new(vec![Signal::remote(owner.signal.clone())]).unwrap();drop(owner);assert!(dropped.load(Ordering::Acquire));assert!(!watch.healthy());
 }
 #[test]fn poisoned_or_oversized_watch_is_an_explicit_failure_not_a_healthy_guess(){
  let signal=Signal::remote(live());assert!(Watch::new(vec![signal.clone();1001]).is_err());let watch=Watch::new(vec![signal.clone()]).unwrap();assert!(watch.publish(vec![signal;1001]).is_err());
  let copy=watch.clone();let _=std::thread::spawn(move||{let _guard=copy.0.write().unwrap();panic!("own fixture poison")}).join();assert!(!watch.healthy());assert!(watch.publish(Vec::new()).is_err());
 }
 fn status(state:&str)->Vec<String>{vec![format!("state: {state}"),"volume: 50".into(),"elapsed: 0.000".into()]}
 #[test]fn startup_pause_and_normal_stop_are_healthy_but_error_or_invalid_status_is_not(){
  for state in ["play","pause","stop"]{assert!(producer_status_healthy(&status(state)));}
  let mut lines=status("pause");lines.push("error: private-fixture-capability-must-not-be-returned".into());assert!(!producer_status_healthy(&lines));lines.push("error: duplicate".into());assert!(!producer_status_healthy(&lines));assert!(!producer_status_healthy(&status("unknown")));assert!(!producer_status_healthy(&[]));
 }
 #[test]fn candidate_failure_preserves_healthy_old_route_while_a_fault_in_the_owned_current_route_retires_it(){
  for code in ["EQ_RUNTIME_UNCONFIRMED","EQ_CONTINUITY_LOST","EQ_RUNTIME_EXITED","EQ_ROLLBACK_FAILED","EQ_CURSOR_UNCONFIRMED"]{
   assert!(needs_current_probe(code));assert!(!retire_current(code,false,true,true));assert!(retire_current(code,false,true,false));assert!(!retire_current(code,false,false,false));
  }
 }
 #[test]fn unconfirmed_queue_or_config_always_retires_exact_owned_session_without_touching_journal_bytes(){
  for eq in [false,true]{for code in ["PLAYBACK_QUEUE_UNCERTAIN","windows_config_uncertain","rollback_failed"]{assert!(retire_current(code,false,eq,true));}assert!(retire_current("arbitrary_fixed_failure",true,eq,true));}
  for code in ["invalid_argument","QUEUE_EDIT_CHANGED","PLAYBACK_SUPERSEDED","EQ_COMPARISON_STALE","EQ_PRESET_INVALID","EQ_RUNTIME_START_FAILED"]{assert!(!needs_current_probe(code));assert!(!retire_current(code,false,true,false));}
 }
}
