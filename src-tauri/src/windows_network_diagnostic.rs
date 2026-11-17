//! Passive file/network admission observations. Only fixed stage labels and an
//! optional numeric Win32 error cross into the original bounded diagnostics.
//! Never accept paths, hostnames, identities, protocol payloads or credentials.
use std::time::Duration;
#[derive(Clone,Copy)]
pub(crate) enum Step { Normalize,Canonicalize,Root,Metadata,Attributes,Open,HandleType,FinalPath,PathMismatch,ProtocolQuery,ProtocolRejected,ProtocolBuffer,ProtocolVersion,ProtocolProvider,ProtocolOffline,IdentityQuery,IdentityRejected,ProviderBuffer,ProviderPath }
impl Step {
    fn code(self)->&'static str {match self {
        Self::Normalize=>"WIN_LIBRARY_ROOT_NORMALIZE_FAILED",Self::Canonicalize=>"WIN_LIBRARY_ROOT_CANONICALIZE_FAILED",Self::Root=>"WIN_NETWORK_ROOT_VERIFY_FAILED",
        Self::Metadata=>"WIN_FILE_METADATA_FAILED",Self::Attributes=>"WIN_FILE_ATTRIBUTES_REJECTED",Self::Open=>"WIN_FILE_OPEN_FAILED",Self::HandleType=>"WIN_FILE_HANDLE_TYPE_REJECTED",
        Self::FinalPath=>"WIN_FILE_FINAL_PATH_FAILED",Self::PathMismatch=>"WIN_FILE_FINAL_PATH_MISMATCH",Self::ProtocolQuery=>"WIN_NETWORK_PROTOCOL_QUERY_FAILED",Self::ProtocolRejected=>"WIN_NETWORK_PROTOCOL_REJECTED",
        Self::ProtocolBuffer=>"WIN_NETWORK_PROTOCOL_BUFFER_REJECTED",Self::ProtocolVersion=>"WIN_NETWORK_PROTOCOL_VERSION_REJECTED",Self::ProtocolProvider=>"WIN_NETWORK_PROTOCOL_PROVIDER_REJECTED",Self::ProtocolOffline=>"WIN_NETWORK_PROTOCOL_OFFLINE_REJECTED",
        Self::IdentityQuery=>"WIN_FILE_ID_QUERY_FAILED",Self::IdentityRejected=>"WIN_FILE_ID_REJECTED",Self::ProviderBuffer=>"WIN_NETWORK_PROVIDER_BUFFER_REJECTED",Self::ProviderPath=>"WIN_NETWORK_PROVIDER_PATH_REJECTED",
    }}
}
const STEPS:[Step;19]=[Step::Normalize,Step::Canonicalize,Step::Root,Step::Metadata,Step::Attributes,Step::Open,Step::HandleType,Step::FinalPath,Step::PathMismatch,Step::ProtocolQuery,Step::ProtocolRejected,Step::ProtocolBuffer,Step::ProtocolVersion,Step::ProtocolProvider,Step::ProtocolOffline,Step::IdentityQuery,Step::IdentityRejected,Step::ProviderBuffer,Step::ProviderPath];
fn text(step:Step,os:Option<u32>)->String{match os{Some(os)=>format!("{} (os error {os})",step.code()),None=>step.code().to_owned()}}
pub(crate) fn failure(step:Step,os:Option<u32>){
    let message=text(step,os);
    crate::local_diagnostics::record(crate::local_diagnostics::Stage::StartupSourceAdmission,Duration::ZERO,Some(&message));
}
/// Recognize only a complete static label or our canonical numeric suffix.
/// This is also safe for arbitrary untrusted errors passed to error_summary.
pub(crate) fn known_code(raw:&str)->Option<&'static str>{
    for step in STEPS {
        let code=step.code();if raw==code{return Some(code);}
        if let Some(number)=raw.strip_prefix(code).and_then(|v|v.strip_prefix(" (os error ")).and_then(|v|v.strip_suffix(')')) {
            if let Ok(value)=number.parse::<u32>(){if number==value.to_string(){return Some(code);}}
        }
    }
    None
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn every_stage_and_optional_win32_code_are_finite_and_canonical(){
        for step in STEPS {for number in [None,Some(0),Some(5),Some(50),Some(u32::MAX)]{let raw=text(step,number);assert_eq!(known_code(&raw),Some(step.code()));let summary=crate::diagnostics::error_summary(&raw);assert!(summary.contains(&format!(" code={}",step.code())));if let Some(number)=number{assert!(summary.contains(&format!("os_code={number}")));}}}
    }
    #[test]fn observation_reaches_original_bounded_report_with_only_fixed_stage_and_numeric_error(){
        failure(Step::ProtocolQuery,Some(53));
        failure(Step::ProtocolProvider,None);
        let report=serde_json::to_value(crate::local_diagnostics::get_local_diagnostics()).unwrap();
        assert_eq!(report["schema"],1);
        assert!(report["entries"].as_array().unwrap().iter().any(|entry|entry["stage"]=="startup_source_admission"&&entry["outcome"]=="error"&&entry["error_summary"].as_str().is_some_and(|s|s.contains(" code=WIN_NETWORK_PROTOCOL_QUERY_FAILED")&&s.contains("os_code=53"))));
        assert!(report["entries"].as_array().unwrap().iter().any(|entry|entry["error_summary"].as_str().is_some_and(|s|s.contains(" code=WIN_NETWORK_PROTOCOL_PROVIDER_REJECTED")&&!s.contains("os_code="))));
        assert!(!text(Step::Open,None).contains("os error"));
    }
    #[test]fn injected_private_paths_protocol_or_malformed_numbers_never_gain_a_stage(){
        for step in STEPS{for tail in [" C:\\PRIVATE\\music.flac"," https://PRIVATE.invalid/?token=SECRET"," (os error 05)","\nPRIVATE_SHARE"]{let raw=format!("{}{tail}",step.code());assert_eq!(known_code(&raw),None);let safe=crate::diagnostics::error_summary(&raw);assert!(!safe.contains("PRIVATE")&&!safe.contains("SECRET")&&!safe.contains(" code="));}}
        for raw in ["WIN_FILE_OPEN_FAILED C:\\PRIVATE\\music.flac","WIN_FILE_OPEN_FAILED https://PRIVATE.invalid/token=SECRET","WIN_FILE_OPEN_FAILED (os error 5) PRIVATE","WIN_FILE_OPEN_FAILED (os error -1)","WIN_FILE_OPEN_FAILED (os error 05)","WIN_FILE_OPEN_FAILED (os error 4294967296)","WIN_FILE_OPEN_FAILED (os error ５)","WIN_NETWORK_PROTOCOL_QUERY_FAILED\nPRIVATE_SHARE"] {
            assert_eq!(known_code(raw),None);let summary=crate::diagnostics::error_summary(raw);assert!(!summary.contains("PRIVATE")&&!summary.contains("SECRET")&&!summary.contains("music.flac")&&!summary.contains(" code="));
        }
    }
}
