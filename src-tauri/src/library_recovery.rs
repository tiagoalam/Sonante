//! Startup diagnosis and explicit admission to the finite, proven-set executor.
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reason {
    IntentValid,
    IntentInvalid,
    OrphanStaging,
    ConfigMissing,
    ConfigInvalid,
    ConfigDivergent,
    MirrorMissing,
    MirrorInvalid,
    GenerationDivergent,
    DatabaseMissing,
    DatabaseUnproven,
    LedgerMissing,
    LedgerDivergent,
    UnsafeEntry,
    FilesystemChanged,
    OwnershipUnknown,
    NoPendingMarker,
    RecoverySetUnproven,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Observation {
    pub analysis_id: u64,
    pub reasons: Vec<Reason>,
    pub intent_version: Option<u32>,
    pub queue_backup_version: Option<u32>,
    pub config_matches: ConfigMatch,
    pub recovery_available: bool,
    #[serde(skip_serializing_if="Option::is_none")]
    pub plan: Option<crate::recovery_set::Plan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigMatch {
    Previous,
    Next,
    Both,
    Unbound,
    Unknown,
}

impl Observation {
    pub(crate) fn new(analysis_id: u64) -> Self {
        Self {
            analysis_id,
            reasons: Vec::new(),
            intent_version: None,
            queue_backup_version: None,
            config_matches: ConfigMatch::Unknown,
            recovery_available: false,
            plan: None,
        }
    }
    pub(crate) fn note(&mut self, reason: Reason) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}

pub(crate) const CANCELLED: &str = "LIBRARY_RECOVERY_ANALYSIS_CANCELLED";
pub(crate) const UNAVAILABLE: &str = "LIBRARY_RECOVERY_ANALYSIS_UNAVAILABLE";
pub(crate) const BUSY: &str = "LIBRARY_RECOVERY_ANALYSIS_BUSY";

pub(crate) struct State {
    blocked_at_startup: bool,
    epoch: AtomicU64,
    busy: AtomicBool,
    last: Mutex<Option<Observation>>,
    request: Mutex<Option<String>>,
    completed: Mutex<Option<String>>,
}

struct Lease<'a>(&'a State);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

impl State {
    pub(crate) fn new(blocked_at_startup: bool) -> Self {
        Self {
            blocked_at_startup,
            epoch: AtomicU64::new(0),
            busy: AtomicBool::new(false),
            last: Mutex::new(None),
            request: Mutex::new(None),
            completed: Mutex::new(None),
        }
    }
    pub(crate) fn cancel(&self, request_id: &str) {
        let Ok(request) = self.request.lock() else {
            return;
        };
        if request.as_deref() != Some(request_id) {
            return;
        }
        self.epoch.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut last) = self.last.lock() { *last=None; }
        if let Ok(mut completed)=self.completed.lock(){*completed=None;}
    }
    pub(crate) fn analyze(
        &self,
        root: &std::path::Path,
        request_id: &str,
        owns_profile: bool,
        shutdown: impl Fn() -> bool,
    ) -> Result<Observation, String> {
        self.analyze_with(request_id, shutdown, |id, cancelled| {
            let mut observation=crate::managed_save_intent::inspect_recovery(root,id,cancelled)?;
            if let Ok(dir)=crate::recovery_fs::Dir::open(root){
                if owns_profile && dir.exists(crate::recovery_set::PENDING)? {
                    match crate::recovery_set::plan_for_profile(&dir,&crate::supervisor::MpdSupervisor::socket_path(),cancelled){
                        Ok(plan)=>{observation.plan=Some(plan);observation.recovery_available=true;observation.reasons.retain(|r|*r!=Reason::OwnershipUnknown);}
                        Err(_)=>{crate::recovery_fs::check(cancelled)?;observation.note(Reason::RecoverySetUnproven);}
                    }
                }
            }
            Ok(observation)
        })
    }
    fn analyze_with(
        &self,
        request_id: &str,
        shutdown: impl Fn() -> bool,
        inspect: impl FnOnce(u64, &dyn Fn() -> bool) -> Result<Observation, String>,
    ) -> Result<Observation, String> {
        if request_id.is_empty()
            || request_id.len() > 64
            || !request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || !self.blocked_at_startup
            || shutdown()
        {
            return Err(UNAVAILABLE.into());
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(BUSY.into());
        }
        let _lease = Lease(self);
        *self.completed.lock().map_err(|_|UNAVAILABLE)?=None;
        let id = {
            let mut request = self.request.lock().map_err(|_| UNAVAILABLE)?;
            *request = Some(request_id.to_string());
            let id = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            *self.last.lock().map_err(|_| UNAVAILABLE)? = None;
            id
        };
        let cancelled = || self.epoch.load(Ordering::Acquire) != id || shutdown();
        let result = inspect(id, &cancelled)?;
        // Publish under the same mutex used by cancel: either cancellation clears
        // this result or the epoch check prevents it being published afterwards.
        let mut last = self.last.lock().map_err(|_| UNAVAILABLE)?;
        if cancelled() {
            return Err(CANCELLED.into());
        }
        *last = Some(result.clone());
        Ok(result)
    }
    pub(crate) fn report(
        &self,
        analysis_id: u64,
        shutdown: impl Fn() -> bool,
    ) -> Result<String, String> {
        if shutdown() {
            return Err(CANCELLED.into());
        }
        let last = self.last.lock().map_err(|_| UNAVAILABLE)?;
        let result = last
            .as_ref()
            .filter(|o| {
                o.analysis_id == analysis_id && self.epoch.load(Ordering::Acquire) == analysis_id
            })
            .ok_or(UNAVAILABLE)?;
        // Only this DTO is exported. Never paths, config, manifests, hashes of
        // personal controls, queue metadata, raw errors, or Plex credentials.
        let mut sanitized=result.clone();sanitized.plan=None;
        serde_json::to_string_pretty(&serde_json::json!({
            "report_schema": 1, "product_version": env!("CARGO_PKG_VERSION"), "kind": "sonante_library_startup_diagnostic",
            "observation": sanitized, "catalog_coverage": "unproven",
            "audio_started_by_analysis": false, "profile_changed_by_analysis": false
        })).map_err(|_| UNAVAILABLE.into())
    }
}

pub(crate) struct RecoveryAdmission(());
impl State {
    pub(crate) fn admission(&self,analysis_id:u64,plan_id:&str)->Result<RecoveryAdmission,String>{
        if !self.blocked_at_startup{return Err(UNAVAILABLE.into())}
        let last=self.last.lock().map_err(|_|UNAVAILABLE)?;
        if self.epoch.load(Ordering::Acquire)!=analysis_id||last.as_ref().filter(|o|o.analysis_id==analysis_id).and_then(|o|o.plan.as_ref()).is_none_or(|p|p.id!=plan_id){return Err(UNAVAILABLE.into())}
        Ok(RecoveryAdmission(()))
    }
    pub(crate) fn execute(&self,owner:&crate::profile_lease::Lease,analysis_id:u64,plan_id:&str,shutdown:impl Fn()->bool)->Result<crate::recovery_set::ResultDto,String>{
        self.admission(analysis_id,plan_id)?;
        if self.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err(BUSY.into())}let _lease=Lease(self);
        let cancelled=||shutdown()||self.epoch.load(Ordering::Acquire)!=analysis_id;
        let result=crate::recovery_set::execute(owner,plan_id,&crate::supervisor::MpdSupervisor::socket_path(),&cancelled,&crate::recovery_catalog::inspect,&|_|Ok(()))?;
        if cancelled(){return Err(CANCELLED.into())}
        *self.completed.lock().map_err(|_|UNAVAILABLE)?=Some(plan_id.into());Ok(result)
    }
    pub(crate) fn allow_reopen(&self,id:&str,root:&std::path::Path)->Result<(),String>{
        if self.busy.load(Ordering::Acquire){return Err(BUSY.into())}
        let mut complete=self.completed.lock().map_err(|_|UNAVAILABLE)?;
        if complete.as_deref()!=Some(id){return Err(UNAVAILABLE.into())}
        crate::managed_save_intent::check_startup(root)?;crate::recovery_set::validate_reopen_state(root)?;*complete=None;Ok(())
    }
    pub(crate) fn should_persist_shutdown_queue(&self)->bool{!self.blocked_at_startup}
}

/// Export only to a new user-selected file. Never overwrite or follow a symlink.
/// No profile/control writer is used for a diagnostic report.
pub(crate) fn save_report(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if bytes.len() > 32 * 1024 {
        return Err(UNAVAILABLE.into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| UNAVAILABLE)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| UNAVAILABLE.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    #[test]
    fn cancellation_and_shutdown_discard_late_result_and_report() {
        let state = State::new(true);
        let result = state.analyze_with(
            "test",
            || false,
            |id, cancel| {
                state.cancel("test");
                assert!(cancel());
                Ok(Observation::new(id))
            },
        );
        assert_eq!(result.unwrap_err(), CANCELLED);
        assert!(state.report(1, || false).is_err());
        let stopping = AtomicBool::new(false);
        assert_eq!(
            state
                .analyze_with(
                    "test",
                    || stopping.load(Ordering::Acquire),
                    |id, _| {
                        stopping.store(true, Ordering::Release);
                        Ok(Observation::new(id))
                    }
                )
                .unwrap_err(),
            CANCELLED
        );
        stopping.store(false, Ordering::Release);
        let next = state
            .analyze_with("test", || false, |id, _| Ok(Observation::new(id)))
            .unwrap();
        assert!(state.report(next.analysis_id, || false).is_ok());
        state.cancel("test");
        assert!(state.report(next.analysis_id, || false).is_err());
    }
    #[test]
    fn one_analysis_owner_and_normal_bootstrap_cannot_run_diagnostic() {
        let state = State::new(true);
        let result = state
            .analyze_with(
                "test",
                || false,
                |id, _| {
                    assert_eq!(
                        state
                            .analyze_with("test", || false, |_, _| panic!("busy must not inspect"))
                            .unwrap_err(),
                        BUSY
                    );
                    Ok(Observation::new(id))
                },
            )
            .unwrap();
        assert!(!result.recovery_available);
        assert!(State::new(false)
            .analyze_with("test", || false, |_, _| panic!("normal startup"))
            .is_err());
        assert!(state
            .analyze_with("test", || true, |_, _| panic!("shutdown"))
            .is_err());
    }
    #[test]
    fn export_uses_only_sanitized_dto_and_never_old_analysis() {
        let state = State::new(true);
        let first = state
            .analyze_with("test", || false, |id, _| Ok(Observation::new(id)))
            .unwrap();
        let second = state
            .analyze_with(
                "test",
                || false,
                |id, _| {
                    let mut value = Observation::new(id);
                    value.note(Reason::IntentInvalid);
                    Ok(value)
                },
            )
            .unwrap();
        assert!(state.report(first.analysis_id, || false).is_err());
        let report = state.report(second.analysis_id, || false).unwrap();
        assert!(report.contains("intent_invalid"));
        for secret in ["/home/", "X-Plex-Token", "local_folders", "queue_backup\":"] {
            assert!(!report.contains(secret));
        }
    }
    #[test]
    fn stale_cancellation_does_not_clear_newer_analysis() {
        let state = State::new(true);
        let first = state
            .analyze_with("old", || false, |id, _| Ok(Observation::new(id)))
            .unwrap();
        let second = state
            .analyze_with("new", || false, |id, _| Ok(Observation::new(id)))
            .unwrap();
        state.cancel("old");
        assert!(state.report(first.analysis_id, || false).is_err());
        assert!(state.report(second.analysis_id, || false).is_ok());
    }
    #[test]
    fn report_export_creates_private_new_file_and_refuses_existing_or_symlink() {
        use std::os::unix::fs::MetadataExt;
        let path = std::env::temp_dir().join(format!("sonante-report-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let report = path.join("report.json");
        save_report(&report, b"{}").unwrap();
        assert_eq!(std::fs::metadata(&report).unwrap().mode() & 0o777, 0o600);
        assert!(save_report(&report, b"replacement").is_err());
        let link = path.join("symlink");
        std::os::unix::fs::symlink(&report, &link).unwrap();
        assert!(save_report(&link, b"replacement").is_err());
        assert_eq!(std::fs::read(&report).unwrap(), b"{}");
        std::fs::remove_dir_all(path).unwrap();
    }
}
