//! Configuration commit/rollback used by the real Windows control worker.
use super::{library::Sources,protocol::Result};
use std::path::PathBuf;
pub(crate) const UNCERTAIN:&str="windows_config_uncertain";
// Audio participates in the existing source transaction. Restore failures are
// uncertain, never a successful partial configuration.
pub(crate) fn with_audio<A:PartialEq>(before:&A,next:&A,mut save:impl FnMut(&A)->Result<()>,configure:impl FnOnce()->Result<()>)->Result<()>{
 if before==next{return configure();}
 if let Err(error)=save(next){return if save(before).is_err(){Err(UNCERTAIN)}else{Err(error)};}
 match configure(){Ok(())=>Ok(()),Err(error)=>if save(before).is_err(){Err(UNCERTAIN)}else{Err(error)}}
}
pub(crate) fn apply(
 before:(Vec<PathBuf>,Sources),roots:Option<Vec<PathBuf>>,sources:Sources,
 mut store:impl FnMut(Option<Vec<PathBuf>>,Sources)->Result<()>,
 mut navidrome:impl FnMut(bool)->Result<()>,healthy:impl Fn()->bool,
)->Result<()> {
 sources.validate()?;
 let changed=roots.is_some();
 if let Err(error)=store(roots,sources){
  let previous=if changed{Some(before.0.clone())}else{None};
  if store(previous,before.1).is_err(){return Err(UNCERTAIN);}
  return Err(error);
 }
 if sources.navidrome!=before.1.navidrome {
  if let Err(error)=navidrome(sources.navidrome){
   let previous=if changed{Some(before.0)}else{None};
   let disk=store(previous,before.1);let host=navidrome(before.1.navidrome);
   if disk.is_err()||host.is_err()||!healthy(){return Err(UNCERTAIN);}
   return Err(error);
  }
 }
 Ok(())
}
#[cfg(test)]
mod tests {
 use super::*;
 use std::cell::RefCell;
 #[derive(Clone)]struct State{roots:Vec<PathBuf>,flags:Sources,host:bool}
 fn old()->State{State{roots:vec![PathBuf::from("OLD")],flags:Sources::default(),host:false}}
 #[test]fn confirmed_folders_and_flags_are_one_commit_and_unchanged_nav_does_not_reconfigure_the_host(){
  let state=RefCell::new(old());let roots=vec![PathBuf::from("NEW")];let desired=Sources{local:true,navidrome:true,plex:false};let calls=RefCell::new(Vec::new());
  apply((old().roots,old().flags),Some(roots.clone()),desired,|roots,flags|{let mut s=state.borrow_mut();s.roots=roots.unwrap();s.flags=flags;Ok(())},|enabled|{state.borrow_mut().host=enabled;calls.borrow_mut().push(enabled);Ok(())},||true).unwrap();
  assert_eq!(state.borrow().roots,roots);assert_eq!(state.borrow().flags,desired);assert!(state.borrow().host);assert_eq!(*calls.borrow(),[true]);
  apply((roots,desired),None,desired,|roots,_|{assert!(roots.is_none());Ok(())},|_|panic!("unchanged host reconfigured"),||true).unwrap();
 }
 #[test]fn a_store_failure_after_modification_restores_exact_folders_and_flags_before_any_host_effect(){
  let state=RefCell::new(old());let mut writes=0;
  let error=apply((old().roots,old().flags),Some(vec![PathBuf::from("NEW")]),Sources{local:false,navidrome:true,plex:false},|roots,flags|{writes+=1;let mut s=state.borrow_mut();s.roots=roots.unwrap();s.flags=flags;if writes==1{Err("worker_failed")}else{Ok(())}},|_|panic!("store failure reached host"),||true).unwrap_err();
  assert_eq!(error,"worker_failed");assert_eq!(state.borrow().roots,old().roots);assert_eq!(state.borrow().flags,old().flags);assert_eq!(writes,2);
 }
 #[test]fn failed_host_activation_rolls_back_both_authorities_and_never_claims_completion(){
  let state=RefCell::new(old());let mut changes=0;
  let error=apply((old().roots,old().flags),None,Sources{local:true,navidrome:true,plex:false},|roots,flags|{assert!(roots.is_none());state.borrow_mut().flags=flags;Ok(())},|enabled|{changes+=1;state.borrow_mut().host=enabled;if changes==1{Err("NAVIDROME_STORAGE")}else{Ok(())}},||true).unwrap_err();
  assert_eq!(error,"NAVIDROME_STORAGE");assert_eq!(state.borrow().flags,old().flags);assert!(!state.borrow().host);assert_eq!(changes,2);
 }
 #[test]fn rollback_failure_or_retired_live_lease_is_uncertain_not_the_original_error_or_success(){
  for scenario in 0..3{
   let mut writes=0;let mut hosts=0;
   let error=apply((old().roots,old().flags),None,Sources{local:true,navidrome:true,plex:false},|_,_|{writes+=1;if scenario==0&&writes==2{Err("write_failed")}else{Ok(())}},|_|{hosts+=1;if hosts==1||scenario==1{Err("NAVIDROME_STORAGE")}else{Ok(())}},||scenario!=2).unwrap_err();
   assert_eq!(error,UNCERTAIN);
  }
 }
 #[test]fn no_sources_is_rejected_before_any_durable_or_host_effect(){assert_eq!(apply((old().roots,old().flags),None,Sources{local:false,navidrome:false,plex:false},|_,_|panic!("invalid write"),|_|panic!("invalid host"),||true),Err("windows_source_required"));}
}

// Shared actor-side configuration transaction. Credential preparation happens
// before this function; Plex publication is the last fallible store/host step.
pub(crate) fn apply_all(
 before:(Vec<PathBuf>,Sources),roots:Option<Vec<PathBuf>>,sources:Sources,account_changed:bool,
 mut store:impl FnMut(Option<Vec<PathBuf>>,Sources)->Result<()>,
 navidrome:impl FnMut(bool)->Result<()>,plex:impl FnMut(bool)->Result<()>,healthy:impl Fn()->bool,
)->Result<()> {
 apply_all_options((before.0,before.1,()),roots,sources,(),account_changed,|r,s,_|store(r,s),navidrome,plex,healthy)
}
pub(crate) fn apply_all_options<T:Copy>(
 before:(Vec<PathBuf>,Sources,T),roots:Option<Vec<PathBuf>>,sources:Sources,options:T,account_changed:bool,
 mut store:impl FnMut(Option<Vec<PathBuf>>,Sources,T)->Result<()>,
 mut navidrome:impl FnMut(bool)->Result<()>,mut plex:impl FnMut(bool)->Result<()>,healthy:impl Fn()->bool,
)->Result<()> {
 sources.validate()?;let changed=roots.is_some();let previous=||if changed{Some(before.0.clone())}else{None};
 if let Err(error)=store(roots,sources,options){return if store(previous(),before.1,before.2).is_err(){Err(UNCERTAIN)}else{Err(error)};}
 let nav_changed=sources.navidrome!=before.1.navidrome;
 if nav_changed {if let Err(error)=navidrome(sources.navidrome){let disk=store(previous(),before.1,before.2);let host=navidrome(before.1.navidrome);return if disk.is_err()||host.is_err()||!healthy(){Err(UNCERTAIN)}else{Err(error)};}}
 if account_changed || sources.plex!=before.1.plex {if let Err(error)=plex(sources.plex){let disk=store(previous(),before.1,before.2);let nav=if nav_changed{navidrome(before.1.navidrome)}else{Ok(())};let host=plex(before.1.plex);return if disk.is_err()||nav.is_err()||host.is_err()||!healthy(){Err(UNCERTAIN)}else{Err(error)};}}
 Ok(())
}
#[cfg(test)]mod all_source_tests{
 use super::*;use std::cell::RefCell;
 #[test]fn plex_credential_publication_is_last_and_failure_rolls_back_flags_roots_and_nav(){
  for unhealthy in [false,true]{
   let calls=RefCell::new(Vec::new());let roots=vec![PathBuf::from("OLD")];let mut plex_calls=0;
   let result=apply_all((roots.clone(),Sources::default()),Some(vec![PathBuf::from("NEW")]),Sources{local:true,navidrome:true,plex:true},true,|r,s|{calls.borrow_mut().push(format!("disk:{:?}:{}:{}",r,s.navidrome,s.plex));Ok(())},|yes|{calls.borrow_mut().push(format!("nav:{yes}"));Ok(())},|yes|{plex_calls+=1;calls.borrow_mut().push(format!("plex:{yes}"));if plex_calls==1{Err("PLEX_STORAGE")}else{Ok(())}},||!unhealthy);
   assert_eq!(result,Err(if unhealthy{UNCERTAIN}else{"PLEX_STORAGE"}));let calls=calls.borrow();assert!(calls[0].contains("NEW"));assert_eq!(calls[1],"nav:true");assert_eq!(calls[2],"plex:true");assert!(calls[3].contains("OLD"));assert_eq!(calls[4],"nav:false");assert_eq!(calls[5],"plex:false");
  }
 }
 #[test]fn unchanged_flags_still_publish_an_explicit_account_edit_but_not_an_unrelated_folder_edit(){
  for changed in [false,true]{let mut calls=0;apply_all((Vec::new(),Sources::default()),None,Sources::default(),changed,|_,_|Ok(()),|_|panic!("unchanged Nav"),|yes|{assert!(!yes);calls+=1;Ok(())},||true).unwrap();assert_eq!(calls,usize::from(changed));}
 }
}
#[cfg(test)]mod audio_tests{
 use super::*;
 #[test]fn sources_failure_restores_audio_and_restore_failure_is_uncertain(){
  for failure in [false,true]{let mut values=Vec::new();let result=with_audio(&1,&2,|v|{values.push(*v);if failure&&*v==1{Err("audio_disk")}else{Ok(())}},||Err("PLEX_STORAGE"));assert_eq!(values,[2,1]);assert_eq!(result,Err(if failure{UNCERTAIN}else{"PLEX_STORAGE"}));}
 }
 #[test]fn failed_audio_write_rolls_back_before_any_source_effect_and_unchanged_audio_has_no_write(){
  let mut calls=0;assert_eq!(with_audio(&1,&2,|_|{calls+=1;if calls==1{Err("audio_disk")}else{Ok(())}},||panic!("partial audio reached sources")),Err("audio_disk"));assert_eq!(calls,2);
  assert_eq!(with_audio(&1,&1,|_|panic!("unchanged audio written"),||Ok(())),Ok(()));
 }
}

#[cfg(test)]mod edition_options_tests{
 use super::*;
 #[test]fn experimental_setting_is_in_the_same_rollback_and_unchanged_root_transaction(){
  for stage in [0,1,2]{let mut calls=Vec::new();let mut writes=0;let mut nav=0;let mut plex=0;
   let result=apply_all_options((Vec::new(),Sources::default(),false),None,Sources{local:true,navidrome:true,plex:true},true,true,|r,s,b|{assert!(r.is_none());calls.push((s,b));writes+=1;if stage==0&&writes==1{Err("disk")}else{Ok(())}},|_|{nav+=1;if stage==1&&nav==1{Err("nav")}else{Ok(())}},|_|{plex+=1;if stage==2&&plex==1{Err("plex")}else{Ok(())}},||true);
   assert_eq!(result,Err(["disk","nav","plex"][stage]));assert_eq!(calls.len(),2);assert_eq!(calls[0].1,true);assert_eq!(calls[1],(Sources::default(),false));
  }
 }
}
