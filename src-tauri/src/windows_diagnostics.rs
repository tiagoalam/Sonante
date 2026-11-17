//! The private Windows preview does not enable developer contract collection.
//! Shared production modules may query this capability without using Linux
//! diagnostic/profile files. Normal safe aggregate debug logs remain unchanged.
pub(crate) fn dev_enabled()->bool { false }

#[path="windows_diagnostic_policy.rs"]mod policy;
#[path="windows_diagnostic_engine.rs"]mod engine;
#[path="windows_network_diagnostic.rs"]mod network;
pub(crate) use network::{failure as network_failure,Step as NetworkStep};
/// An already received status error, never an extra engine query or conversion.
pub(crate) fn playback_error(direct:bool,eq:bool,raw:&str){
 let code=engine::code(direct,eq,raw);
 crate::local_diagnostics::record(crate::local_diagnostics::Stage::QueueApply,std::time::Duration::ZERO,Some(code));
}
pub(crate) fn error_summary(error:impl std::fmt::Display)->String{
 let text=error.to_string();let mut summary=policy::error_summary(&text);
 if let Some(code)=network::known_code(&text){if !summary.contains(" code="){summary.push_str(" code=");summary.push_str(code);}}
 // Exact native application codes only, never an arbitrary prefix/path/URL.
 if (engine::known(&text)||matches!(text.as_str(),"engine_exited"|"runtime_missing"|"runtime_changed"|"spawn_failed"|"engine_connect_failed"|"engine_start_timeout"|"worker_failed"|"queue_unconfirmed"|"queue_state_unconfirmed"|"PLAYBACK_RANDOM_ORDER_UNAVAILABLE"|"PLAYBACK_SUPERSEDED"|"shutting_down"|"EQ_SHARED_SWITCH_UNCONFIRMED"|"EQ_RANDOM_STEP_UNAVAILABLE"|"EQ_RUNTIME_EXITED"|"EQ_RUNTIME_START_FAILED"|"EQ_CONTINUITY_LOST"|"EQ_RUNTIME_UNCONFIRMED"|"EQ_APPLY_FAILED"|"windows_direct_unconfirmed"|"mpd_playback_error"|"windows_media_unavailable"|"windows_config_uncertain"))&&!summary.contains(" code="){summary.push_str(" code=");summary.push_str(&text);}
 summary
}
/// Measurements add no I/O, worker, engine query, event or error conversion.
/// The exact original Result and all RAII cleanup stay with the caller.
pub(crate) fn measured<T,E:std::fmt::Display>(stage:crate::local_diagnostics::Stage,run:impl FnOnce()->Result<T,E>)->Result<T,E>{
 let start=std::time::Instant::now();let result=run();let error=result.as_ref().err().map(ToString::to_string);crate::local_diagnostics::record(stage,start.elapsed(),error.as_deref());result
}
#[cfg(test)]mod tests{
 use super::*;use crate::local_diagnostics::{Stage,get_local_diagnostics};
 #[test]fn observation_preserves_exact_success_error_and_single_effect_without_dev_collection(){
  let mut calls=0;let value=Box::new([1,2,3]);let address=(&*value)as*const _;let result:Result<_,&str>=measured(Stage::QueueApply,||{calls+=1;Ok(value)});assert_eq!(calls,1);assert_eq!((&*result.unwrap())as*const _,address);
  let mut calls=0;let original=String::from("PRIVATE_METADATA https://fixture.invalid/?token=PRIVATE_SECRET C:\\PRIVATE\\file.flac (os error 5)");let expected=original.clone();let result:Result<(),String>=measured(Stage::StartupEngine,||{calls+=1;Err(original)});assert_eq!(calls,1);assert_eq!(result.unwrap_err(),expected);assert!(!dev_enabled());
  let report=serde_json::to_value(get_local_diagnostics()).unwrap();assert_eq!(report["schema"],1);assert_eq!(report["version"],env!("CARGO_PKG_VERSION"));let entries=report["entries"].as_array().unwrap();let last=entries.last().unwrap();assert_eq!(last["stage"],"startup_engine");assert_eq!(last["outcome"],"error");assert!(last["error_summary"].as_str().unwrap().contains("os_code=5"));let text=serde_json::to_string(&report).unwrap();for secret in ["PRIVATE_METADATA","fixture.invalid","PRIVATE_SECRET","file.flac"]{assert!(!text.contains(secret));}
 }
 #[test]fn only_exact_native_codes_are_retained_and_untrusted_prefixes_never_become_data(){
  assert!(error_summary("engine_exited").contains(" code=engine_exited"));assert!(error_summary("EQ_SHARED_SWITCH_UNCONFIRMED").contains(" code=EQ_SHARED_SWITCH_UNCONFIRMED"));
  for raw in ["engine_exited?token=PRIVATE_SECRET","runtime_changed C:\\PRIVATE\\file.flac","https://fixture.invalid/?token=PRIVATE_SECRET","PRIVATE_METADATA\nACK [5@0] {command} PRIVATE_SECRET"]{let safe=error_summary(raw);assert!(!safe.contains("PRIVATE"));assert!(!safe.contains("fixture.invalid"));assert!(!safe.contains(" code=engine_exited"));}
 }
 #[test]fn corrupted_own_settings_report_actual_restore_error_without_repair_or_engine(){
  let mut n=[0;12];getrandom::fill(&mut n).unwrap();let dir=std::env::temp_dir().join(format!("sonante-passive-diagnostic-own-{n:x?}"));std::fs::create_dir(&dir).unwrap();let path=dir.join("settings.json");std::fs::write(&path,b"PRIVATE_INVALID_OWN_FIXTURE").unwrap();let library=crate::windows_mpd::library::Library::new(path.clone());assert_eq!(library.status().unwrap().phase,"failed");assert_eq!(std::fs::read(&path).unwrap(),b"PRIVATE_INVALID_OWN_FIXTURE");let report=serde_json::to_value(get_local_diagnostics()).unwrap();let last=report["entries"].as_array().unwrap().last().unwrap();assert_eq!(last["stage"],"startup_config");assert_eq!(last["outcome"],"error");assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_INVALID_OWN_FIXTURE"));library.stop();std::fs::remove_dir_all(dir).unwrap();
 }
}
