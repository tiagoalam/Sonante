//! Runtime-only comparison. No store, output switch, queue or transport commands.
use super::*;
use crate::audio::EqTransitionWatch;
use serde::Deserialize;
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum Side { Reference, Equalized }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Deserialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum Action { Reference, Equalized, Finish }
#[derive(Clone,Debug,Serialize)]
pub(crate) struct Comparison {
 pub token:String,
 pub side:Side,
 pub source:EqPreset,
 pub effective_gain_db:f64,
 pub interrupted:bool,
}
#[derive(Clone)]
pub(super) struct Active { view:Comparison, watch:EqTransitionWatch }
impl Session {
 pub(super) fn comparison_view(&self)->Option<Comparison>{self.comparison.as_ref().map(|a|{let mut v=a.view.clone();v.interrupted=!a.watch.current();v})}
 pub(super) fn compare(&mut self,old:&Snapshot,action:Action,token:Option<String>,expected:Option<EqPreset>,watch:Option<EqTransitionWatch>)->Result<Snapshot,Error>{
  if !old.verified||old.error.is_some()||self.current.load(Ordering::Acquire)!=self.id{return Err(Error::ComparisonUnavailable)}
  let (mut view,watch)=if let Some(active)=&self.comparison {
   if token.as_deref()!=Some(active.view.token.as_str()){return Err(Error::ComparisonStale)}
   (active.view.clone(),active.watch.clone())
  }else{
   if token.is_some()||action!=Action::Reference||expected.as_ref()!=Some(&self.preset)||old.applied.as_ref()!=Some(&self.preset){return Err(Error::ComparisonStale)}
   let watch=watch.ok_or(Error::ComparisonStale)?;
   let gain=safety::assess(&self.preset,48000).map_err(|_|Error::ApplyFailed)?.effective_preamp_db;
   static NEXT_COMPARISON:AtomicU64=AtomicU64::new(1);
   (Comparison{token:format!("cmp_{}_{}",self.id,NEXT_COMPARISON.fetch_add(1,Ordering::Relaxed)),source:self.preset.clone(),side:Side::Equalized,effective_gain_db:gain,interrupted:false},watch)
  };
  if !watch.current(){return Err(Error::ComparisonInterrupted)}
  self.health()?; // Owned child, exact rendered DSP and current sink/link identity.
  let reference=action==Action::Reference;
  if self.reference!=reference {
   self.controller.apply_comparison(self.id,&self.preset,pcm(),reference,&||watch.current()).map_err(|e|match e {
    runtime::RuntimeError::IntentChanged=>Error::ComparisonInterrupted,
    runtime::RuntimeError::SessionStale=>Error::Cancelled,
    runtime::RuntimeError::RollbackFailed|runtime::RuntimeError::RuntimePoisoned=>Error::RollbackFailed,
    _=>Error::ApplyFailed,
   })?;
  }
  self.reference=reference;
  self.health()?; // Do not publish a changed side on a lost or replaced route.
  view.side=if reference{Side::Reference}else{Side::Equalized};
  view.interrupted=!watch.current();
  self.comparison=if action==Action::Finish&&!view.interrupted{None}else{Some(Active{view,watch})};
  Ok(Snapshot{applied:if reference{None}else{Some(self.preset.clone())},comparison:self.comparison_view(),transition:None,error:None,verified:true,..old.clone()})
 }
}
