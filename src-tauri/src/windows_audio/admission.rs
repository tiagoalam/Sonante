//! Match the original Linux volume contract: refuse an intervention while a
//! route transaction is busy; never queue a gain gesture behind activation.
#[derive(Default)]pub struct Gate{operation:std::sync::Mutex<()>,context:std::sync::atomic::AtomicU64}
impl Gate{
 pub fn operation(&self)->super::Result<std::sync::MutexGuard<'_,()>>{self.operation.lock().map_err(|_|"worker_failed")}
 pub fn context(&self)->u64{self.context.load(std::sync::atomic::Ordering::Acquire)}
 pub fn changed(&self){self.context.fetch_add(1,std::sync::atomic::Ordering::AcqRel);}
 pub fn accepts(&self,context:u64)->bool{self.context()==context}
 pub fn volume(&self,stopping:bool)->super::Result<std::sync::MutexGuard<'_,()>>{
  if stopping{return Err("shutting_down");}
  self.operation.try_lock().map_err(|_|"engine_busy")
 }
 pub fn volume_intervention(&self,epoch:&std::sync::atomic::AtomicU64,stopping:bool)->super::Result<std::sync::MutexGuard<'_,()>>{
  // Same Linux epoch invalidation BEFORE waiting for audio/config state. Even
  // a refused gesture must cancel an in-flight level-matched comparison.
  epoch.fetch_add(1,std::sync::atomic::Ordering::AcqRel);
  self.volume(stopping)
 }
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn volume_refuses_busy_and_shutdown_before_a_gesture_can_be_enqueued(){let gate=Gate::default();let active=gate.operation().unwrap();assert!(matches!(gate.volume(false),Err("engine_busy")));drop(active);assert!(gate.volume(false).is_ok());assert!(matches!(gate.volume(true),Err("shutting_down")));}
 #[test]fn rejected_gain_invalidates_the_actual_comparison_watch_without_waiting_for_the_held_operation(){
  use std::sync::{Arc,atomic::{AtomicU64,AtomicBool}};
  let epoch=Arc::new(AtomicU64::new(0));let watch=crate::audio::EqTransitionWatch{intent:Arc::new(AtomicU64::new(7)),intent_value:7,intervention:epoch.clone(),intervention_value:0,stopping:Arc::new(AtomicBool::new(false))};
  let gate=Gate::default();let _active=gate.operation().unwrap();assert!(watch.current());
  assert!(matches!(gate.volume_intervention(&epoch,false),Err("engine_busy")));assert!(!watch.current());
 }
 #[test]fn a_context_change_already_queued_ahead_of_gain_rejects_its_ticket(){
  let gate=Gate::default();let gesture=gate.volume(false).unwrap();let ticket=gate.context();drop(gesture);
  {let _route=gate.operation().unwrap();gate.changed();}
  let _gain=gate.operation().unwrap();assert!(!gate.accepts(ticket));
 }
 #[test]fn read_queries_and_consecutive_gains_do_not_expire_the_same_context(){
  let gate=Gate::default();let gesture=gate.volume(false).unwrap();let ticket=gate.context();drop(gesture);
  for _ in 0..3{let _query_or_gain=gate.operation().unwrap();assert!(gate.accepts(ticket));}
 }
}
