//! Startup diagnosis and explicit admission to the finite, proven-set executor.
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
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

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum OfferAssessmentStatus { EligibleAtObservation, Ineligible, Unconfirmed }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum OfferAssessmentReason {
    OwnershipUnconfirmed, PlanningUnconfirmed, PlanChanged, HistoricalMismatch,
    HistoricalProofIncomparable, OriginalRootUnavailable, OriginalRootPermissionDenied,
    OriginUnconfirmed, LedgerInvalid, LedgerExtensionUnconfirmed, ManifestUnconfirmed,
    UnsupportedFormat, UnsafeOrAmbiguous, SnapshotChanged, IncompleteSet, Cancelled,
    Unknown,
}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub(crate) enum OfferTargetRole { Previous, Completion, Confirmed, JournalResume, JournalCommitted }
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
enum OfferScope { ReadonlyOfferOnly }
/// Sanitized observation, not an execution capability. No paths, identifiers,
/// physical identity, witnesses, hashes, errors or effect/preservation claims.
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
pub(crate) struct OfferAssessment {
    status:OfferAssessmentStatus,
    #[serde(skip_serializing_if="Option::is_none")]
    reason:Option<OfferAssessmentReason>,
    #[serde(skip_serializing_if="Option::is_none")]
    target_role:Option<OfferTargetRole>,
    scope:OfferScope,
}
impl OfferAssessment {
    fn unconfirmed(reason:OfferAssessmentReason)->Self {
        Self{status:OfferAssessmentStatus::Unconfirmed,reason:Some(reason),target_role:None,scope:OfferScope::ReadonlyOfferOnly}
    }
    fn eligible_observation(&self)->bool {
        self.status==OfferAssessmentStatus::EligibleAtObservation && self.reason.is_none()
            && self.scope==OfferScope::ReadonlyOfferOnly
    }
    fn reason_from_core(reason:Option<crate::recovery_set::PreliminaryRecoveryReason>)->OfferAssessmentReason {
        use crate::{recovery_set::PreliminaryRecoveryReason as C,source_identity::PreliminaryHistoricalReason as H};
        use OfferAssessmentReason as R;
        match reason {
            Some(C::Historical(h))=>match h {
                H::HistoricalMismatch=>R::HistoricalMismatch,H::HistoricalIncomparable=>R::HistoricalProofIncomparable,
                H::RootNotFound=>R::OriginalRootUnavailable,H::RootPermissionDenied=>R::OriginalRootPermissionDenied,
                H::LedgerInvalid=>R::LedgerInvalid,H::LedgerExtensionUnconfirmed=>R::LedgerExtensionUnconfirmed,
                H::UnsupportedLedger=>R::UnsupportedFormat,H::ManifestBindingUnconfirmed=>R::ManifestUnconfirmed,
                H::RootUnsafe=>R::UnsafeOrAmbiguous,H::ObservationChanged=>R::SnapshotChanged,H::Cancelled=>R::Cancelled,
                H::RootAccessUnconfirmed|H::Unconfirmed=>R::OriginUnconfirmed,H::MatchesAtInspection=>R::Unknown,
            },
            Some(C::SnapshotChanged)=>R::SnapshotChanged,Some(C::UnsafeOrAmbiguous)=>R::UnsafeOrAmbiguous,
            Some(C::IncompleteSet)=>R::IncompleteSet,Some(C::Cancelled)=>R::Cancelled,
            Some(C::ReadUnconfirmed)|Some(C::StructuralUnconfirmed)=>R::PlanningUnconfirmed,None=>R::Unknown,
        }
    }
    // Planning already refused: this second observation is diagnostic only.
    // Even a later match cannot create an eligible assessment or a target role.
    fn planning_refusal(core:&crate::recovery_set::PreliminaryRecoveryAssessment)->Self {
        use crate::recovery_set::PreliminaryRecoveryOutcome as O;
        let status=match core.outcome() {
            O::Ineligible=>OfferAssessmentStatus::Ineligible,
            O::Unconfirmed|O::MatchesAtInspection=>OfferAssessmentStatus::Unconfirmed,
        };
        let reason=if core.outcome()==O::MatchesAtInspection || core.reason().is_none() {
            OfferAssessmentReason::PlanningUnconfirmed
        }else{Self::reason_from_core(core.reason())};
        Self{status,reason:Some(reason),target_role:None,scope:OfferScope::ReadonlyOfferOnly}
    }
    fn from_core(core:&crate::recovery_set::PreliminaryRecoveryAssessment,plan:&crate::recovery_set::Plan)->Self {
        use crate::recovery_set::{PreliminaryRecoveryOutcome as O,PreliminaryFinalRole as F};
        let target_role=core.role().map(|r|match r {F::Previous=>OfferTargetRole::Previous,F::Completion=>OfferTargetRole::Completion,
            F::Confirmed=>OfferTargetRole::Confirmed,F::JournalResume=>OfferTargetRole::JournalResume,F::JournalCommitted=>OfferTargetRole::JournalCommitted});
        let (status,reason)=match core.outcome() {
            O::MatchesAtInspection if core.matches_plan(plan)=>(OfferAssessmentStatus::EligibleAtObservation,None),
            O::MatchesAtInspection=>(OfferAssessmentStatus::Unconfirmed,Some(OfferAssessmentReason::PlanChanged)),
            O::Ineligible=>(OfferAssessmentStatus::Ineligible,Some(Self::reason_from_core(core.reason()))),
            O::Unconfirmed=>(OfferAssessmentStatus::Unconfirmed,Some(Self::reason_from_core(core.reason()))),
        };
        Self{status,reason,target_role,scope:OfferScope::ReadonlyOfferOnly}
    }
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
    #[serde(skip_serializing_if="Option::is_none")]
    pub assessment: Option<OfferAssessment>,
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
            assessment: None,
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

// One publication unit under last, with no independent binding store.
// Neither private record is serialized or Debug-formatted.
struct PublishedAnalysis {
    observation:Observation,
    binding:Option<Arc<crate::recovery_set::OfferedRecoveryBinding>>,
    attempt:Option<AttemptRecord>,
}
struct AttemptRecord { analysis_epoch:u64, plan_id:String, outcome:AttemptOutcome }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum AttemptOutcome { NotAttempted, Unconfirmed, Failed(crate::recovery_set::FailureSummary), ReturnedOk }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum AttemptFreshness { Current, Stale }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct AttemptView { freshness:AttemptFreshness, outcome:AttemptOutcome }
impl AttemptView {
    pub(crate) fn freshness(&self)->AttemptFreshness {self.freshness}
    pub(crate) fn outcome(&self)->AttemptOutcome {self.outcome}
}
struct AnalysisDraft {
    observation:Observation,
    binding:Option<Arc<crate::recovery_set::OfferedRecoveryBinding>>,
}
impl AnalysisDraft {
    fn unbound(observation:Observation)->Self {Self{observation,binding:None}}
}
// Backend completion data only; never serialized, Debug-formatted or a credential.
// Constructed in production only after the actual executor returns Ok.
#[derive(Clone)]
struct CompletedRecoveryProof {
    analysis_epoch:u64,
    plan_id:String,
    observed_profile:crate::recovery_fs::Identity,
}
impl CompletedRecoveryProof {
    fn same_context(&self,other:&Self)->bool {
        self.analysis_epoch==other.analysis_epoch && self.plan_id==other.plan_id
            && self.observed_profile==other.observed_profile
    }
}
pub(crate) struct State {
    blocked_at_startup: bool,
    epoch: AtomicU64,
    busy: AtomicBool,
    last: Mutex<Option<PublishedAnalysis>>,
    request: Mutex<Option<String>>,
    completed: Mutex<Option<CompletedRecoveryProof>>,
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
        {
            // Recover only to invalidate. The mutex stays poisoned, so future
            // publication/consumption/analysis still refuse via their normal map_err.
            let mut complete=self.completed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            self.epoch.fetch_add(1, Ordering::AcqRel);
            *complete=None;
        } // Never hold completed while acquiring last.
        if let Ok(mut last)=self.last.lock() {
            if last.as_ref().is_some_and(|r|r.attempt.is_some()) {
                if let Some(record)=last.as_mut(){record.binding=None;}
            }else{*last=None;}
        } // Epoch changed: retained record is diagnostic-only, report/admission reject old epoch.
    }
    pub(crate) fn analyze(
        &self,
        root: &std::path::Path,
        request_id: &str,
        owner: Option<std::sync::Arc<crate::profile_lease::Lease>>,
        shutdown: impl Fn() -> bool,
    ) -> Result<Observation, String> {
        // Borrow the real Arc into the whole operation, including publication.
        // The explicit drop is AFTER analyze_with_context returns, never at the
        // end of the inspection closure. No lock/permissions are acquired here.
        let result=self.analyze_with_context(request_id,shutdown,Some(root),owner.as_ref(),|id,cancelled| {
            let mut observation=crate::managed_save_intent::inspect_recovery(root,id,cancelled)?;
            let mut binding=None;
            observation.assessment=Some(OfferAssessment::unconfirmed(OfferAssessmentReason::OwnershipUnconfirmed));
            if !Self::owner_matches(owner.as_ref(),Some(root)) {return Ok(AnalysisDraft::unbound(observation));}
            let dir=match crate::recovery_fs::Dir::open(root) {
                Ok(dir)=>dir,Err(_)=>return Ok(AnalysisDraft::unbound(observation)),
            };
            if !Self::owner_matches_dir(owner.as_ref(),&dir) {return Ok(AnalysisDraft::unbound(observation));}
            match dir.exists(crate::recovery_set::PENDING) {
                Ok(true)=>match crate::recovery_set::plan_for_profile(&dir,&crate::supervisor::MpdSupervisor::socket_path(),cancelled) {
                    Ok(plan)=>{
                        let core=crate::recovery_set::assess_final_readonly(&dir,cancelled);
                        crate::recovery_fs::check(cancelled)?;
                        let assessment=OfferAssessment::from_core(&core,&plan);
                        observation.assessment=Some(assessment);
                        if assessment.eligible_observation() {
                            crate::recovery_fs::check(cancelled)?;
                            if let Some(actual_owner)=owner.as_ref() {
                                match core.into_offer_binding(id,&plan,actual_owner) {
                                    Ok(context)=>{
                                        binding=Some(Arc::new(context));
                                        observation.plan=Some(plan);observation.recovery_available=true;
                                        observation.reasons.retain(|r|*r!=Reason::OwnershipUnknown);
                                    }
                                    Err(_)=>Self::revoke_context(&mut observation),
                                }
                            }
                        }else{observation.note(Reason::RecoverySetUnproven);}
                    }
                    Err(_)=>{
                        crate::recovery_fs::check(cancelled)?;
                        let core=crate::recovery_set::assess_final_readonly(&dir,cancelled);
                        crate::recovery_fs::check(cancelled)?;
                        observation.assessment=Some(OfferAssessment::planning_refusal(&core));
                        observation.plan=None;observation.recovery_available=false;binding=None;
                        observation.note(Reason::RecoverySetUnproven);
                    }
                },
                Ok(false)=>{
                    observation.assessment=Some(OfferAssessment::unconfirmed(OfferAssessmentReason::IncompleteSet));
                }
                Err(_)=>{
                    crate::recovery_fs::check(cancelled)?;
                    observation.assessment=Some(OfferAssessment::unconfirmed(OfferAssessmentReason::PlanningUnconfirmed));
                    observation.note(Reason::RecoverySetUnproven);
                }
            }
            if !Self::owner_matches_dir(owner.as_ref(),&dir) {
                Self::revoke_offer(&mut observation);binding=None;
            }
            Ok(AnalysisDraft{observation,binding})
        });
        drop(owner);
        result
    }
    fn owner_matches(owner:Option<&std::sync::Arc<crate::profile_lease::Lease>>,root:Option<&std::path::Path>)->bool {
        let (Some(owner),Some(root))=(owner,root) else{return false};
        if owner.validate().is_err(){return false;}
        let Ok(dir)=crate::recovery_fs::Dir::open(root) else{return false};
        Self::owner_matches_dir(Some(owner),&dir)
    }
    fn owner_matches_dir(owner:Option<&std::sync::Arc<crate::profile_lease::Lease>>,dir:&crate::recovery_fs::Dir)->bool {
        let Some(owner)=owner else{return false};
        owner.validate().is_ok() && dir.id()==owner.root().id() && dir.validate().is_ok() && owner.validate().is_ok()
    }
    fn revoke_offer(observation:&mut Observation) {
        observation.plan=None;observation.recovery_available=false;
        observation.assessment=Some(OfferAssessment::unconfirmed(OfferAssessmentReason::OwnershipUnconfirmed));
        observation.note(Reason::OwnershipUnknown);
    }
    fn revoke_context(observation:&mut Observation) {
        observation.plan=None;observation.recovery_available=false;
        observation.assessment=Some(OfferAssessment::unconfirmed(OfferAssessmentReason::PlanChanged));
        observation.note(Reason::RecoverySetUnproven);
    }
    // Existing State unit tests keep a component-only inspection entry; it never
    // proves owner/legacy fence/core eligibility. Any synthetic offer is revoked.
    #[cfg(test)]
    fn analyze_with(
        &self,request_id:&str,shutdown:impl Fn()->bool,
        inspect:impl FnOnce(u64,&dyn Fn()->bool)->Result<Observation,String>,
     )->Result<Observation,String>{self.analyze_with_context(request_id,shutdown,None,None,
        |id,cancel|inspect(id,cancel).map(AnalysisDraft::unbound))}
    fn analyze_with_context(
        &self,
        request_id: &str,
        shutdown: impl Fn() -> bool,
        root: Option<&std::path::Path>,
        owner: Option<&std::sync::Arc<crate::profile_lease::Lease>>,
        inspect: impl FnOnce(u64, &dyn Fn() -> bool) -> Result<AnalysisDraft, String>,
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
        let id = {
            let mut request = self.request.lock().map_err(|_| UNAVAILABLE)?;
            let mut complete=self.completed.lock().map_err(|_|UNAVAILABLE)?;
            *request = Some(request_id.to_string());
            let id = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
            *complete=None;
            drop(complete); // Do not overlap completed and last.
            *self.last.lock().map_err(|_| UNAVAILABLE)? = None;
            id
        };
        let cancelled = || self.epoch.load(Ordering::Acquire) != id || shutdown();
        let mut result = inspect(id, &cancelled)?;
        // Publish under the same mutex used by cancel: either cancellation clears
        // this result or the epoch check prevents it being published afterwards.
        let mut last = self.last.lock().map_err(|_| UNAVAILABLE)?;
        if cancelled() {
            return Err(CANCELLED.into());
        }
        // This short publication region still uses real held owner context.
        // Metadata IO may block; cancellation is rechecked after it returns.
        if (root.is_some() || result.observation.recovery_available || result.observation.plan.is_some())
            && !Self::owner_matches(owner,root) {
            Self::revoke_offer(&mut result.observation);result.binding=None;
        } else {
            let coherent=result.observation.analysis_id==id
                && result.observation.recovery_available
                && result.observation.assessment.as_ref().is_some_and(OfferAssessment::eligible_observation)
                && result.observation.plan.as_ref().is_some_and(|plan|
                    result.binding.as_ref().is_some_and(|binding|binding.matches_offer(id,plan)
                        && owner.is_some_and(|owner|binding.matches_owner(owner))));
            if !coherent {
                // Preserve legitimate non-offer diagnostics with a valid owner.
                if result.observation.plan.is_some() || result.observation.recovery_available {
                    Self::revoke_context(&mut result.observation);
                }
                result.binding=None;
            }
        }
        if cancelled() {return Err(CANCELLED.into());}
        let observation=result.observation.clone();
        *last = Some(PublishedAnalysis{observation:result.observation,binding:result.binding,attempt:None});
        Ok(observation)
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
                o.observation.analysis_id == analysis_id && self.epoch.load(Ordering::Acquire) == analysis_id
            })
            .ok_or(UNAVAILABLE)?;
        // Only this DTO is exported. Never paths, config, manifests, hashes of
        // personal controls, queue metadata, raw errors, or Plex credentials.
        let mut sanitized=result.observation.clone();sanitized.plan=None;sanitized.assessment=None;
        serde_json::to_string_pretty(&serde_json::json!({
            "report_schema": 1, "product_version": env!("CARGO_PKG_VERSION"), "kind": "sonante_library_startup_diagnostic",
            "observation": sanitized, "catalog_coverage": "unproven",
            "audio_started_by_analysis": false, "profile_changed_by_analysis": false
        })).map_err(|_| UNAVAILABLE.into())
    }
}

pub(crate) struct RecoveryAdmission(());
impl State {
    fn record_matches(record:&PublishedAnalysis,analysis_id:u64,plan_id:&str)->bool {
        let o=&record.observation;
        record.attempt.is_none() && o.analysis_id==analysis_id && o.recovery_available
            && o.assessment.as_ref().is_some_and(OfferAssessment::eligible_observation)
            && o.plan.as_ref().is_some_and(|p|p.id==plan_id
                && record.binding.as_ref().is_some_and(|b|b.matches_offer(analysis_id,p)))
    }
    pub(crate) fn admission(&self,analysis_id:u64,plan_id:&str)->Result<RecoveryAdmission,String>{
        if !self.blocked_at_startup{return Err(UNAVAILABLE.into())}
        let last=self.last.lock().map_err(|_|UNAVAILABLE)?;
        if self.epoch.load(Ordering::Acquire)!=analysis_id
            || last.as_ref().is_none_or(|record|!Self::record_matches(record,analysis_id,plan_id)) {
            return Err(UNAVAILABLE.into());
        }
        Ok(RecoveryAdmission(()))
    }
    pub(crate) fn execute(&self,owner:&crate::profile_lease::Lease,analysis_id:u64,plan_id:&str,shutdown:impl Fn()->bool)->Result<crate::recovery_set::ResultDto,String>{
        self.admission(analysis_id,plan_id)?;
        if self.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err(BUSY.into())}let _lease=Lease(self);
        let cancelled=||shutdown()||self.epoch.load(Ordering::Acquire)!=analysis_id;
        // Do not invoke shutdown callback under request/last. claim rechecks epoch under request.
        if cancelled(){return Err(CANCELLED.into());}
        let expected=self.claim_attempt(analysis_id,plan_id)?;
        let mut captured=None;
        let outcome=(||->Result<crate::recovery_set::ResultDto,String>{
            if !expected.matches_owner(owner){return Err(UNAVAILABLE.into());}
            if cancelled(){return Err(CANCELLED.into());}
            let result=match crate::recovery_set::execute_projected(owner,plan_id,&expected,
                &crate::supervisor::MpdSupervisor::socket_path(),&cancelled,
                &crate::recovery_catalog::inspect,&|_|Ok(())) {
                Ok(result)=>result,
                Err(failure)=>{let(summary,legacy)=failure.into_parts();captured=Some(summary);return Err(legacy);}
            };
            if cancelled(){return Err(CANCELLED.into());}
            if !expected.matches_owner(owner){return Err(UNAVAILABLE.into());}
            if cancelled(){return Err(CANCELLED.into());}
            self.publish_completed(CompletedRecoveryProof{analysis_epoch:analysis_id,
                plan_id:plan_id.into(),observed_profile:owner.root().id()})?;
            Ok(result)
        })();
        let terminal=if outcome.is_ok(){AttemptOutcome::ReturnedOk}else{
            AttemptOutcome::Failed(captured.unwrap_or_else(crate::recovery_set::FailureSummary::unknown))
        };
        // Best effort diagnostics cannot change the legacy result or publish completion.
        // If this fails, the claimed slot remains Unconfirmed and still denies re-execution.
        let _=self.finish_attempt(analysis_id,plan_id,terminal);
        outcome
    }
    fn claim_attempt(&self,analysis_id:u64,plan_id:&str)->Result<Arc<crate::recovery_set::OfferedRecoveryBinding>,String> {
        let _request=self.request.lock().map_err(|_|UNAVAILABLE)?;
        let mut last=self.last.lock().map_err(|_|UNAVAILABLE)?;
        if self.epoch.load(Ordering::Acquire)!=analysis_id{return Err(CANCELLED.into());}
        let record=last.as_mut().filter(|r|Self::record_matches(r,analysis_id,plan_id)).ok_or(UNAVAILABLE)?;
        let expected=Arc::clone(record.binding.as_ref().ok_or(UNAVAILABLE)?);
        record.attempt=Some(AttemptRecord{analysis_epoch:analysis_id,plan_id:plan_id.into(),outcome:AttemptOutcome::Unconfirmed});
        Ok(expected)
    }
    fn finish_attempt(&self,analysis_id:u64,plan_id:&str,outcome:AttemptOutcome)->Result<(),String> {
        let _request=self.request.lock().map_err(|_|UNAVAILABLE)?;
        let mut last=self.last.lock().map_err(|_|UNAVAILABLE)?;
        // Never create/reinsert a record. A stale update may only fill its retained,
        // uniquely claimed slot; new analysis clears it and cannot be overwritten.
        let record=last.as_mut().filter(|r|r.observation.analysis_id==analysis_id).ok_or(UNAVAILABLE)?;
        let attempt=record.attempt.as_mut().filter(|a|a.analysis_epoch==analysis_id && a.plan_id==plan_id).ok_or(UNAVAILABLE)?;
        if attempt.outcome!=AttemptOutcome::Unconfirmed{return Err(UNAVAILABLE.into());}
        attempt.outcome=outcome;
        Ok(())
    }
    pub(crate) fn attempt_view(&self,analysis_id:u64)->Result<AttemptView,String> {
        let _request=self.request.lock().map_err(|_|UNAVAILABLE)?;
        let last=self.last.lock().map_err(|_|UNAVAILABLE)?;
        let record=last.as_ref().filter(|r|r.observation.analysis_id==analysis_id).ok_or(UNAVAILABLE)?;
        let freshness=if self.epoch.load(Ordering::Acquire)==analysis_id{AttemptFreshness::Current}else{AttemptFreshness::Stale};
        let outcome=record.attempt.as_ref().map(|a|a.outcome).unwrap_or(AttemptOutcome::NotAttempted);
        Ok(AttemptView{freshness,outcome})
    }
    fn publish_completed(&self,proof:CompletedRecoveryProof)->Result<(),String> {
        let mut complete=self.completed.lock().map_err(|_|UNAVAILABLE)?;
        if self.epoch.load(Ordering::Acquire)!=proof.analysis_epoch {
            return Err(CANCELLED.into());
        }
        *complete=Some(proof);
        Ok(())
    }
    fn completed_candidate(&self,id:&str)->Result<CompletedRecoveryProof,String> {
        let complete=self.completed.lock().map_err(|_|UNAVAILABLE)?;
        let proof=complete.as_ref().filter(|p|p.analysis_epoch==self.epoch.load(Ordering::Acquire)
            && p.plan_id==id).ok_or(UNAVAILABLE)?;
        Ok(proof.clone()) // Local immutable snapshot, not a second authority store.
    }
    fn consume_completed(&self,expected:&CompletedRecoveryProof)->Result<(),String> {
        let mut complete=self.completed.lock().map_err(|_|UNAVAILABLE)?;
        if self.epoch.load(Ordering::Acquire)!=expected.analysis_epoch
            || complete.as_ref().is_none_or(|p|!p.same_context(expected)) {
            return Err(UNAVAILABLE.into());
        }
        *complete=None; // Successful State admission linearizes here, exactly once.
        Ok(())
    }
    pub(crate) fn allow_reopen(&self,id:&str,root:&std::path::Path)->Result<(),String>{
        if self.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err(BUSY.into())}
        let _lease=Lease(self); // Existing State RAII guard, no new mutex or worker.
        let proof=self.completed_candidate(id)?;
        let dir=crate::recovery_fs::Dir::open(root).map_err(|_|UNAVAILABLE)?;
        if dir.id()!=proof.observed_profile {return Err(UNAVAILABLE.into());}
        // Keep both old disk gates and their exact legacy errors. No completed lock over IO.
        crate::managed_save_intent::check_startup(root)?;
        crate::recovery_set::validate_reopen_state(root)?;
        dir.validate().map_err(|_|UNAVAILABLE)?;
        self.consume_completed(&proof)
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

#[cfg(test)]
mod ff04_offer_tests {
    use super::*;
    use std::{path::PathBuf,sync::Arc,os::unix::fs::{DirBuilderExt,MetadataExt},sync::atomic::{AtomicU64,Ordering}};
    static NEXT:AtomicU64=AtomicU64::new(0);
    // Own real Lease fixtures, retained even on panic. No setup is executed by
    // this compile-only lot. Positive State doubles below are explicitly UNIT
    // state-machine tests, not proof of core+legacy_fence/native recovery.
    struct Own { root:PathBuf,_parent:crate::recovery_fs::Dir,_root:crate::recovery_fs::Dir,owner:Arc<crate::profile_lease::Lease> }
    impl Own {
        fn new()->Self {
            let parent=PathBuf::from(std::env::var_os("FF04_OFFER_OWN_FIXTURES").expect("separate own-FS runtime scope required"));
            assert!(parent.is_absolute());assert_eq!(std::fs::canonicalize(&parent).unwrap(),parent);
            let m=std::fs::symlink_metadata(&parent).unwrap();assert!(m.is_dir());assert_eq!(m.uid(),unsafe{libc::geteuid()});assert_eq!(m.mode()&0o7777,0o700);
            let parent_dir=crate::recovery_fs::Dir::open(&parent).unwrap();
            let name=format!("offer-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::SeqCst));
            std::fs::DirBuilder::new().mode(0o700).create(parent_dir.entry(&name).unwrap()).unwrap();
            let root=parent.join(&name);let dir=crate::recovery_fs::Dir::open(&root).unwrap();parent_dir.validate().unwrap();
            let held=dir.file.metadata().unwrap();let named=std::fs::symlink_metadata(&root).unwrap();assert_eq!((held.dev(),held.ino()),(named.dev(),named.ino()));
            let owner=Arc::new(crate::profile_lease::Lease::acquire(&root).unwrap());
            Self{root,_parent:parent_dir,_root:dir,owner}
        }
    }
    fn unit_plan(id:&str)->crate::recovery_set::Plan {crate::recovery_set::Plan{id:id.into(),choice:crate::recovery_set::Choice::RestorePrevious,
        previous_tracks:0,backup_bytes:0,preserves_current_references:true,resumes_interrupted:false}}
    fn unit_offer(id:u64)->Observation {
        let mut o=Observation::new(id);o.plan=Some(unit_plan("unit-plan-not-physical-proof"));o.recovery_available=true;
        o.assessment=Some(OfferAssessment{status:OfferAssessmentStatus::EligibleAtObservation,reason:None,target_role:Some(OfferTargetRole::Previous),scope:OfferScope::ReadonlyOfferOnly});o
    }
    #[test] fn ff04_offer_none_owner_keeps_readonly_diagnosis_no_plan(){
        let own=Own::new();let state=State::new(true);
        let o=state.analyze(&own.root,"none-owner",None,||false).unwrap();
        assert!(!o.recovery_available&&o.plan.is_none());assert!(o.reasons.contains(&Reason::OwnershipUnknown));
        assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::OwnershipUnconfirmed));
        assert!(state.admission(o.analysis_id,"anything").is_err());
    }
    #[test] fn ff04_offer_wrong_profile_and_invalid_owner_refuse(){
        let a=Own::new();let b=Own::new();
        let o=State::new(true).analyze(&a.root,"wrong-profile",Some(b.owner.clone()),||false).unwrap();
        assert!(!o.recovery_available&&o.plan.is_none());assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::OwnershipUnconfirmed));
        std::fs::rename(a.root.join(".sonante-profile.lock"),a.root.join("owner-lock-away")).unwrap();
        let o=State::new(true).analyze(&a.root,"invalid-owner",Some(a.owner.clone()),||false).unwrap();
        assert!(!o.recovery_available&&o.plan.is_none());assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::OwnershipUnconfirmed));
    }
    #[test] fn ff04_offer_real_arc_no_pending_retains_until_return(){
        let own=Own::new();let state=State::new(true);let count=Arc::strong_count(&own.owner);
        let o=state.analyze(&own.root,"real-owner",Some(own.owner.clone()),||false).unwrap();
        assert_eq!(Arc::strong_count(&own.owner),count);assert!(own.owner.validate().is_ok());
        assert!(!o.recovery_available&&o.plan.is_none());assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::IncompleteSet));
        // No pending set: this path cannot call production legacy_fence/planner.
        // Lifetime through the publication statement is additionally source-audited.
    }
    #[test] fn ff04_offer_unit_owner_change_before_publication_revokes(){
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("owner-change",||false,Some(&own.root),Some(&own.owner),|id,_|{
            assert!(Arc::strong_count(&own.owner)>=1);
            std::fs::rename(own.root.join(".sonante-profile.lock"),own.root.join("owner-lock-away")).unwrap();
            Ok(AnalysisDraft::unbound(unit_offer(id))) // UNIT double only, not a planner/core success.
        }).unwrap();
        assert!(!o.recovery_available&&o.plan.is_none());assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::OwnershipUnconfirmed));
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());
    }
    #[test] fn ff04_offer_unit_cancel_late_result_and_epoch_plan_stale(){
        let own=Own::new();let state=State::new(true);
        let r=state.analyze_with_context("cancel",||false,Some(&own.root),Some(&own.owner),|id,_|{state.cancel("cancel");Ok(AnalysisDraft::unbound(unit_offer(id)))});
        assert_eq!(r.unwrap_err(),CANCELLED);assert!(state.admission(1,"unit-plan-not-physical-proof").is_err());
        let old=state.analyze_with_context("old",||false,Some(&own.root),Some(&own.owner),|id,_|Ok(AnalysisDraft::unbound(unit_offer(id)))).unwrap();
        assert!(state.admission(old.analysis_id,"different-plan").is_err());
        state.analyze_with("new",||false,|id,_|Ok(Observation::new(id))).unwrap();
        assert!(state.admission(old.analysis_id,"unit-plan-not-physical-proof").is_err());
    }
    #[test] fn ff04_offer_unit_admission_requires_same_eligible_observation(){
        let state=State::new(true);let id=7;state.epoch.store(id,Ordering::Release);
        for assessment in [None,Some(OfferAssessment::unconfirmed(OfferAssessmentReason::Unknown)),
            Some(OfferAssessment{status:OfferAssessmentStatus::Ineligible,reason:Some(OfferAssessmentReason::HistoricalMismatch),target_role:None,scope:OfferScope::ReadonlyOfferOnly})] {
            let mut o=unit_offer(id);o.assessment=assessment;*state.last.lock().unwrap()=Some(PublishedAnalysis{observation:o,binding:None,attempt:None});
            assert!(state.admission(id,"unit-plan-not-physical-proof").is_err());
        }
        let mut o=unit_offer(id);o.recovery_available=false;*state.last.lock().unwrap()=Some(PublishedAnalysis{observation:o,binding:None,attempt:None});
        assert!(state.admission(id,"unit-plan-not-physical-proof").is_err());
        let own=Own::new();let o=unit_offer(id);
        let binding=crate::recovery_set::unit_only_synthetic_offer_binding(id,o.plan.as_ref().unwrap(),&own.owner);
        *state.last.lock().unwrap()=Some(PublishedAnalysis{observation:o,binding:Some(Arc::new(binding)),attempt:None});
        assert!(state.admission(id,"unit-plan-not-physical-proof").is_ok()); // UNIT classification gate only
        assert!(state.admission(id+1,"unit-plan-not-physical-proof").is_err());
    }
    #[test] fn ff04_offer_cancelled_core_cannot_match_plan_and_mapping_is_finite(){
        let own=Own::new();let core=crate::recovery_set::assess_final_readonly(own.owner.root(),&||true);
        let p=unit_plan("/public-sentinel-path/X-Plex-Token=not-a-secret");
        assert!(!core.matches_plan(&p));let a=OfferAssessment::from_core(&core,&p);
        assert_eq!(a.status,OfferAssessmentStatus::Unconfirmed);assert_eq!(a.reason,Some(OfferAssessmentReason::Cancelled));
        let json=serde_json::to_string(&a).unwrap();assert!(!json.contains("public-sentinel"));assert!(!json.contains("X-Plex-Token"));
        use crate::{recovery_set::PreliminaryRecoveryReason as C,source_identity::PreliminaryHistoricalReason as H};
        assert_eq!(OfferAssessment::reason_from_core(Some(C::Historical(H::LedgerInvalid))),OfferAssessmentReason::LedgerInvalid);
        assert_eq!(OfferAssessment::reason_from_core(Some(C::Historical(H::HistoricalIncomparable))),OfferAssessmentReason::HistoricalProofIncomparable);
        assert_eq!(OfferAssessment::reason_from_core(Some(C::StructuralUnconfirmed)),OfferAssessmentReason::PlanningUnconfirmed);
        assert_eq!(OfferAssessment::reason_from_core(None),OfferAssessmentReason::Unknown);
    }
    #[test] fn ff04_offer_report_v1_byte_shape_and_no_assessment_leak(){
        let state=State::new(true);let id=9;state.epoch.store(id,Ordering::Release);
        let mut o=unit_offer(id);o.note(Reason::IntentInvalid);
        o.plan.as_mut().unwrap().id="/public-sentinel-profile/Origin/FSID/X-Plex-Token=sentinel".into();
        *state.last.lock().unwrap()=Some(PublishedAnalysis{observation:o,binding:None,attempt:None});
        let report=state.report(id,||false).unwrap();
        let old_shape=serde_json::to_string_pretty(&serde_json::json!({
            "report_schema":1,"product_version":env!("CARGO_PKG_VERSION"),"kind":"sonante_library_startup_diagnostic",
            "observation":{"analysis_id":id,"reasons":["intent_invalid"],"intent_version":null,"queue_backup_version":null,"config_matches":"unknown","recovery_available":true},
            "catalog_coverage":"unproven","audio_started_by_analysis":false,"profile_changed_by_analysis":false
        })).unwrap();
        assert_eq!(report,old_shape);
        for forbidden in ["assessment","eligible_at_observation","target_role","readonly_offer_only","public-sentinel","Origin","FSID","X-Plex-Token"] {assert!(!report.contains(forbidden));}
        assert!(state.last.lock().unwrap().as_ref().unwrap().observation.assessment.is_some());
        assert!(Observation::new(1).assessment.is_none());
    }
    #[test] fn ff04_offer_reanalysis_is_explicit_no_blacklist_or_auto_retry(){
        let own=Own::new();let state=State::new(true);
        let a=state.analyze(&own.root,"first",None,||false).unwrap();let b=state.analyze(&own.root,"second",None,||false).unwrap();
        assert!(b.analysis_id>a.analysis_id);assert!(!a.recovery_available&&!b.recovery_available);
        assert_eq!(a.assessment.unwrap().reason,b.assessment.unwrap().reason);
        assert!(state.admission(a.analysis_id,"anything").is_err());
    }
    fn unit_bound_draft(id:u64,owner:&crate::profile_lease::Lease)->AnalysisDraft {
        let observation=unit_offer(id);
        let binding=crate::recovery_set::unit_only_synthetic_offer_binding(id,observation.plan.as_ref().unwrap(),owner);
        AnalysisDraft{observation,binding:Some(Arc::new(binding))}
    }
    #[test] fn ff04_binding_atomic_record_none_stale_epoch_and_choice_refuse() {
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("unit-bound",||false,Some(&own.root),Some(&own.owner),
            |id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_ok());
        {let mut last=state.last.lock().unwrap();last.as_mut().unwrap().observation.plan.as_mut().unwrap().choice=crate::recovery_set::Choice::FinishConfirmed;}
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());
        let o=state.analyze_with_context("unit-none",||false,Some(&own.root),Some(&own.owner),
            |id,_|Ok(AnalysisDraft::unbound(unit_offer(id)))).unwrap();
        assert!(o.plan.is_none()&&!o.recovery_available);assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::PlanChanged));
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());
        let o=state.analyze_with_context("unit-old-epoch",||false,Some(&own.root),Some(&own.owner),|id,_|{
            let mut d=unit_bound_draft(id,&own.owner);
            d.binding=Some(Arc::new(crate::recovery_set::unit_only_synthetic_offer_binding(id+1,d.observation.plan.as_ref().unwrap(),&own.owner)));Ok(d)
        }).unwrap();
        assert!(o.plan.is_none()&&!o.recovery_available);
    }
    #[test] fn ff04_binding_cancel_clears_record_but_worker_arc_is_only_data() {
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("unit-arc",||false,Some(&own.root),Some(&own.owner),
            |id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        let worker=Arc::clone(state.last.lock().unwrap().as_ref().unwrap().binding.as_ref().unwrap());
        state.cancel("unit-arc");assert!(state.last.lock().unwrap().is_none());
        assert_eq!(Arc::strong_count(&worker),1);assert_ne!(state.epoch.load(Ordering::Acquire),o.analysis_id);
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());
        assert!(state.execute(&own.owner,o.analysis_id,"unit-plan-not-physical-proof",||false).is_err());
    }
    #[test] fn ff04_binding_private_payload_not_exported_in_report_v1() {
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("unit-report",||false,Some(&own.root),Some(&own.owner),
            |id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        let report=state.report(o.analysis_id,||false).unwrap();
        for secret in ["binding","assessment","plan","unit-private-Tree-sentinel","unit-private-manifest-not-history","unit-plan-not-physical-proof","observed_profile","final_live"] {
            assert!(!report.contains(secret));
        }
        assert_eq!(serde_json::from_str::<serde_json::Value>(&report).unwrap()["report_schema"],1);
        assert!(state.last.lock().unwrap().as_ref().unwrap().binding.is_some());
    }
    #[test] fn ff04_binding_valid_owner_incomplete_diagnosis_not_overwritten() {
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("unit-incomplete",||false,Some(&own.root),Some(&own.owner),|id,_|{
            let mut o=Observation::new(id);o.assessment=Some(OfferAssessment::unconfirmed(OfferAssessmentReason::IncompleteSet));
            Ok(AnalysisDraft::unbound(o))
        }).unwrap();
        assert_eq!(o.assessment.unwrap().reason,Some(OfferAssessmentReason::IncompleteSet));
        assert!(!o.reasons.contains(&Reason::OwnershipUnknown));
    }

    #[test] fn projection_unit_claim_once_consumes_offer_independent_of_summary() {
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("projection-claim",||false,Some(&own.root),Some(&own.owner),
            |id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        let expected=match state.claim_attempt(o.analysis_id,"unit-plan-not-physical-proof"){Ok(b)=>b,Err(_)=>panic!("unit claim required")};
        assert!(expected.matches_owner(&own.owner));
        assert!(state.claim_attempt(o.analysis_id,"unit-plan-not-physical-proof").is_err());
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());
        assert!(state.finish_attempt(o.analysis_id,"unit-plan-not-physical-proof",
            AttemptOutcome::Failed(crate::recovery_set::FailureSummary::unknown())).is_ok());
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());
    }
    #[test] fn projection_unit_same_plan_new_epoch_cannot_receive_old_worker_finish() {
        let own=Own::new();let state=State::new(true);
        let a=state.analyze_with_context("projection-old",||false,Some(&own.root),Some(&own.owner),|id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        assert!(state.claim_attempt(a.analysis_id,"unit-plan-not-physical-proof").is_ok());state.cancel("projection-old");
        let b=state.analyze_with_context("projection-new",||false,Some(&own.root),Some(&own.owner),|id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        assert!(a.plan.as_ref().unwrap().id==b.plan.as_ref().unwrap().id && a.analysis_id!=b.analysis_id);
        assert!(state.finish_attempt(a.analysis_id,"unit-plan-not-physical-proof",AttemptOutcome::ReturnedOk).is_err());
        assert!(state.attempt_view(a.analysis_id).is_err());
        assert!(state.claim_attempt(b.analysis_id,"unit-plan-not-physical-proof").is_ok());
    }
    #[test] fn projection_unit_last_poison_finish_refuses_without_reenabling_offer() {
        let own=Own::new();let state=State::new(true);
        let o=state.analyze_with_context("projection-poison",||false,Some(&own.root),Some(&own.owner),|id,_|Ok(unit_bound_draft(id,&own.owner))).unwrap();
        assert!(state.claim_attempt(o.analysis_id,"unit-plan-not-physical-proof").is_ok());
        let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{let _g=state.last.lock().unwrap();panic!("public unit last poison");}));
        assert!(caught.is_err());assert!(state.finish_attempt(o.analysis_id,"unit-plan-not-physical-proof",AttemptOutcome::ReturnedOk).is_err());
        assert!(state.admission(o.analysis_id,"unit-plan-not-physical-proof").is_err());assert!(state.last.is_poisoned());
    }

}

#[cfg(test)]
mod completion_publication_protocol_tests {
    use super::*;
    // Pure UNIT records: no backend/Lease/origin/disk/restart authority proof.
    fn unit_state()->State {
        let s=State::new(true);
        *s.request.lock().unwrap()=Some("unit-request".into());
        {let _c=s.completed.lock().unwrap();s.epoch.store(1,Ordering::Release);}
        s
    }
    fn unit_record(epoch:u64)->CompletedRecoveryProof {
        CompletedRecoveryProof{analysis_epoch:epoch,plan_id:"fixed-unit-plan".into(),
            observed_profile:crate::recovery_fs::Identity{dev:1,ino:2}}
    }
    #[test] fn completion_unit_cancel_before_publish_refuses_obsolete_worker() {
        let s=unit_state();let proof=unit_record(1);
        s.cancel("unit-request");assert!(s.publish_completed(proof).is_err());
        assert!(s.completed_candidate("fixed-unit-plan").is_err());
    }
    #[test] fn completion_unit_cancel_after_publish_revokes_local_snapshot() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        let snapshot=match s.completed_candidate("fixed-unit-plan"){Ok(p)=>p,Err(_)=>panic!("unit candidate required")};
        s.cancel("unit-request");assert!(s.consume_completed(&snapshot).is_err());
    }
    #[test] fn completion_unit_fixed_plan_new_analysis_epoch_cannot_reuse_old_proof() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        let old=match s.completed_candidate("fixed-unit-plan"){Ok(p)=>p,Err(_)=>panic!("unit candidate required")};
        let observation=s.analyze_with("next-unit",||false,|id,_|Ok(Observation::new(id))).unwrap();
        assert!(observation.analysis_id!=old.analysis_epoch);
        assert!(s.completed_candidate("fixed-unit-plan").is_err());
        assert!(s.publish_completed(unit_record(observation.analysis_id)).is_ok());
        assert!(s.consume_completed(&old).is_err());
    }
    #[test] fn completion_unit_profile_context_and_one_shot_consumption() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        assert!(s.completed_candidate("wrong-plan").is_err());
        let snapshot=match s.completed_candidate("fixed-unit-plan"){Ok(p)=>p,Err(_)=>panic!("unit candidate required")};
        let mut wrong=snapshot.clone();wrong.observed_profile.ino=3;
        assert!(s.consume_completed(&wrong).is_err());
        assert!(s.consume_completed(&snapshot).is_ok());
        assert!(s.consume_completed(&snapshot).is_err());
    }
    #[test] fn completion_unit_publish_before_cancel_is_cleared() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        s.cancel("unit-request");assert!(s.completed.lock().unwrap().is_none());
        assert!(s.completed_candidate("fixed-unit-plan").is_err());
    }
    #[test] fn completion_unit_missing_completion_and_wrong_plan_refuse() {
        let s=unit_state();assert!(s.completed_candidate("fixed-unit-plan").is_err());
        assert!(s.publish_completed(unit_record(1)).is_ok());
        assert!(s.completed_candidate("wrong-unit-plan").is_err());
        assert!(s.completed_candidate("fixed-unit-plan").is_ok());
    }
    #[test] fn completion_unit_stale_cancel_request_does_not_revoke_newer_request() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        s.cancel("different-unit-request");
        assert!(s.epoch.load(Ordering::Acquire)==1);
        assert!(s.completed_candidate("fixed-unit-plan").is_ok());
    }
    #[test] fn completion_unit_epoch_validation_happens_at_locked_publish_and_consume() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        let snapshot=match s.completed_candidate("fixed-unit-plan"){Ok(p)=>p,Err(_)=>panic!("unit candidate required")};
        {let mut c=s.completed.lock().unwrap();s.epoch.store(2,Ordering::Release);*c=None;}
        assert!(s.publish_completed(unit_record(1)).is_err());
        assert!(s.publish_completed(unit_record(2)).is_ok());
        assert!(s.consume_completed(&snapshot).is_err());
        assert!(s.completed_candidate("fixed-unit-plan").is_ok());
    }
    #[test] fn completion_unit_busy_refusal_and_existing_raii_release() {
        let s=unit_state();s.busy.store(true,Ordering::Release);
        let result=s.allow_reopen("unit-plan",std::path::Path::new("/unit-path-never-opened-busy"));
        assert!(matches!(result,Err(ref code) if code==BUSY));
        assert!(s.busy.load(Ordering::Acquire));s.busy.store(false,Ordering::Release);
        {assert!(s.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_ok());let _guard=Lease(&s);}
        assert!(!s.busy.load(Ordering::Acquire));
    }
    #[test] fn completion_unit_poison_refuses_but_cancel_still_invalidates() {
        let s=unit_state();assert!(s.publish_completed(unit_record(1)).is_ok());
        // cfg UNIT only: no production catch-unwind or worker lifecycle promise.
        let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
            let _guard=s.completed.lock().unwrap();panic!("public unit poison sentinel");
        }));
        assert!(caught.is_err()&&s.completed.is_poisoned());
        assert!(s.publish_completed(unit_record(1)).is_err());
        assert!(s.completed_candidate("fixed-unit-plan").is_err());
        assert!(s.consume_completed(&unit_record(1)).is_err());
        s.cancel("unit-request");assert!(s.epoch.load(Ordering::Acquire)==2);
        assert!(s.completed.is_poisoned());
        let guard=match s.completed.lock(){Ok(g)=>g,Err(p)=>p.into_inner()};assert!(guard.is_none());
    }

}

#[cfg(test)]
mod projection_state_protocol_units {
    use super::*;
    // Synthetic already-claimed finite protocol data ONLY; no execution/Lease/history proof.
    fn started()->State {
        let s=State::new(true);*s.request.lock().unwrap()=Some("projection-unit".into());
        {let _c=s.completed.lock().unwrap();s.epoch.store(1,Ordering::Release);}
        *s.last.lock().unwrap()=Some(PublishedAnalysis{observation:Observation::new(1),binding:None,
            attempt:Some(AttemptRecord{analysis_epoch:1,plan_id:"fixed-unit-plan".into(),outcome:AttemptOutcome::Unconfirmed})});s
    }
    fn view(s:&State,id:u64)->AttemptView {match s.attempt_view(id){Ok(v)=>v,Err(_)=>panic!("unit view required")}}
    fn failed()->AttemptOutcome {AttemptOutcome::Failed(crate::recovery_set::FailureSummary::unknown())}
    #[test] fn projection_unit_finish_current_then_cancel_keeps_only_stale_cause() {
        let s=started();assert!(view(&s,1).freshness()==AttemptFreshness::Current);
        assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_ok());s.cancel("projection-unit");
        assert!(view(&s,1).freshness()==AttemptFreshness::Stale && view(&s,1).outcome()==failed());
        assert!(s.last.lock().unwrap().as_ref().unwrap().binding.is_none());
        assert!(s.admission(1,"fixed-unit-plan").is_err());assert!(s.completed_candidate("fixed-unit-plan").is_err());
    }
    #[test] fn projection_unit_cancel_then_finish_fills_retained_slot_without_offer() {
        let s=started();s.cancel("projection-unit");assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_ok());
        assert!(view(&s,1).freshness()==AttemptFreshness::Stale);assert!(view(&s,1).outcome()==failed());
        assert!(s.admission(1,"fixed-unit-plan").is_err());
    }
    #[test] fn projection_unit_cancel_without_claim_clears_analysis_record() {
        let s=started();s.last.lock().unwrap().as_mut().unwrap().attempt=None;
        s.cancel("projection-unit");assert!(s.last.lock().unwrap().is_none());assert!(s.attempt_view(1).is_err());
    }
    #[test] fn projection_unit_new_analysis_drops_slot_and_rejects_late_finish() {
        let s=started();let observation=s.analyze_with("new-unit",||false,|id,_|Ok(Observation::new(id))).unwrap();
        assert!(observation.analysis_id!=1);assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_err());
        assert!(s.attempt_view(1).is_err());
        assert!(view(&s,observation.analysis_id).outcome()==AttemptOutcome::NotAttempted);
        assert!(s.finish_attempt(observation.analysis_id,"fixed-unit-plan",failed()).is_err());
    }
    #[test] fn projection_unit_wrong_ids_missing_slot_and_terminal_overwrite_refuse() {
        let s=started();assert!(s.finish_attempt(2,"fixed-unit-plan",failed()).is_err());
        assert!(s.finish_attempt(1,"wrong-plan",failed()).is_err());
        assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_ok());
        assert!(s.finish_attempt(1,"fixed-unit-plan",AttemptOutcome::ReturnedOk).is_err());
        assert!(view(&s,1).outcome()==failed());*s.last.lock().unwrap()=None;
        assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_err());
    }
    #[test] fn projection_unit_request_poison_keeps_claim_consumed_and_no_repair() {
        let s=started();let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
            let _r=s.request.lock().unwrap();panic!("public unit poison");
        }));assert!(caught.is_err());assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_err());
        assert!(s.attempt_view(1).is_err());assert!(s.request.is_poisoned());
        assert!(s.last.lock().unwrap().as_ref().unwrap().attempt.is_some());
        assert!(s.admission(1,"fixed-unit-plan").is_err());
    }
    #[test] fn projection_unit_returned_ok_view_is_not_completion_authority() {
        let s=started();assert!(s.finish_attempt(1,"fixed-unit-plan",AttemptOutcome::ReturnedOk).is_ok());
        assert!(view(&s,1).outcome()==AttemptOutcome::ReturnedOk);
        assert!(s.completed_candidate("fixed-unit-plan").is_err());
        assert!(s.allow_reopen("fixed-unit-plan",std::path::Path::new("/unit-path-never-opened-no-proof")).is_err());
    }
    #[test] fn projection_unit_report_v1_exact_shape_ignores_private_attempt() {
        let s=started();assert!(s.finish_attempt(1,"fixed-unit-plan",failed()).is_ok());
        let report=s.report(1,||false).unwrap();
        let expected=serde_json::to_string_pretty(&serde_json::json!({
            "report_schema":1,"product_version":env!("CARGO_PKG_VERSION"),"kind":"sonante_library_startup_diagnostic",
            "observation":{"analysis_id":1,"reasons":[],"intent_version":null,"queue_backup_version":null,"config_matches":"unknown","recovery_available":false},
            "catalog_coverage":"unproven","audio_started_by_analysis":false,"profile_changed_by_analysis":false
        })).unwrap();assert!(report==expected);
        for forbidden in ["attempt","FailureSummary","fixed-unit-plan","boundary","possible"] {assert!(!report.contains(forbidden));}
        s.cancel("projection-unit");assert!(s.report(1,||false).is_err());
    }
}

// PROPOSED wire-only whitelist. Internal summary/view/carriers never gain Serialize.
#[derive(serde::Serialize)]
pub(crate) struct AttemptDiagnosticDto {
    schema_version:u8,
    analysis_key:Option<String>, // association only, not an execution capability
    availability:&'static str,
    unavailable_reason:Option<&'static str>,
    freshness:Option<&'static str>,
    outcome:Option<&'static str>,
    failure:Option<AttemptFailureDto>,
}
#[derive(serde::Serialize)]
struct AttemptFailureDto {
    class:&'static str,boundary:&'static str,knowledge:&'static str,
    possible_effects:Vec<&'static str>,inherited_observed:Vec<&'static str>,
}
fn diagnostic_lookup(key:&str)->Option<u64> {
    if key.is_empty() || key.len()>20 || (key.len()>1 && key.starts_with('0'))
        || !key.bytes().all(|b|b.is_ascii_digit()) {return None;}
    key.parse::<u64>().ok()
}
fn unavailable_diagnostic(key:Option<String>,reason:&'static str)->AttemptDiagnosticDto {
    AttemptDiagnosticDto{schema_version:1,analysis_key:key,availability:"unavailable",
        unavailable_reason:Some(reason),freshness:None,outcome:None,failure:None}
}
fn diagnostic_class(value:crate::recovery_set::FailureClass)->&'static str {
    match value {
        crate::recovery_set::FailureClass::LegacyUnknown=>"legacy_unknown",
        crate::recovery_set::FailureClass::CancellationObserved=>"cancellation_observed",
        crate::recovery_set::FailureClass::PreflightUnconfirmed=>"preflight_unconfirmed",
        crate::recovery_set::FailureClass::PlanContextChanged=>"plan_context_changed",
        crate::recovery_set::FailureClass::OfferedContextUnconfirmed=>"offered_context_unconfirmed",
        crate::recovery_set::FailureClass::LocalJournalContextUnconfirmed=>"local_journal_context_unconfirmed",
        crate::recovery_set::FailureClass::CatalogMismatchObserved=>"catalog_mismatch_observed",
    }
}
fn diagnostic_boundary(value:crate::recovery_set::FailureBoundary)->&'static str {
    match value {
        crate::recovery_set::FailureBoundary::ProtocolCheck=>"protocol_check",
        crate::recovery_set::FailureBoundary::ReadonlyPreflight=>"readonly_preflight",
        crate::recovery_set::FailureBoundary::ProtectionRecords=>"protection_records",
        crate::recovery_set::FailureBoundary::LocalJournalBinding=>"local_journal_binding",
        crate::recovery_set::FailureBoundary::CatalogCheck=>"catalog_check",
        crate::recovery_set::FailureBoundary::GenerationNamespace=>"generation_namespace",
        crate::recovery_set::FailureBoundary::ControlWrites=>"control_writes",
        crate::recovery_set::FailureBoundary::FinalValidation=>"final_validation",
        crate::recovery_set::FailureBoundary::CommitRetention=>"commit_retention",
        crate::recovery_set::FailureBoundary::Unknown=>"unknown",
    }
}
fn diagnostic_knowledge(value:crate::recovery_set::FailureKnowledge)->&'static str {
    match value {
        crate::recovery_set::FailureKnowledge::ExecutorCaptured=>"executor_captured",
        crate::recovery_set::FailureKnowledge::UnknownBoundary=>"unknown_boundary",
    }
}
fn diagnostic_failure(summary:crate::recovery_set::FailureSummary)->AttemptFailureDto {
    use crate::recovery_set::{PossibleEffect as E,ObservedInherited as I};
    let mut possible_effects=Vec::new();
    for(effect,label)in [(E::Protection,"protection"),(E::LiveGenerationNamespace,"live_generation_namespace"),(E::ControlTemporary,"control_temporary"),(E::CanonicalControl,"canonical_control"),(E::Retention,"retention"),(E::BackupNamespace,"backup_namespace"),(E::ValidatorOrChildUnknown,"validator_or_child_unknown"),(E::OpaqueCallback,"opaque_callback")] {
        if summary.may_include(effect){possible_effects.push(label);}
    }
    let mut inherited_observed=Vec::new();
    for(fact,label)in [(I::JournalAndBlockedRead,"journal_and_blocked_read"),(I::CommittedNameNotCompleted,"committed_name_not_completed")] {
        if summary.observed(fact){inherited_observed.push(label);}
    }
    AttemptFailureDto{class:diagnostic_class(summary.class()),boundary:diagnostic_boundary(summary.boundary()),
        knowledge:diagnostic_knowledge(summary.knowledge()),possible_effects,inherited_observed}
}
pub(crate) fn attempt_diagnostic_response(state:Option<&State>,key:String,shutdown:bool)->AttemptDiagnosticDto {
    let Some(id)=diagnostic_lookup(&key) else{return unavailable_diagnostic(None,"invalid_lookup");};
    // Echo only a canonical numeric association, never arbitrary input text.
    let association=Some(id.to_string());
    if shutdown{return unavailable_diagnostic(association,"shutdown");}
    let view=match state.and_then(|s|s.attempt_view(id).ok()) {
        Some(view)=>view,None=>return unavailable_diagnostic(association,"not_captured_or_unavailable"),
    }; // Getter locks are released before projection/serialization.
    let freshness=match view.freshness(){AttemptFreshness::Current=>"current",AttemptFreshness::Stale=>"stale"};
    let(outcome,failure)=match view.outcome(){
        AttemptOutcome::NotAttempted=>("not_attempted",None),
        AttemptOutcome::Unconfirmed=>("unconfirmed",None),
        AttemptOutcome::ReturnedOk=>("state_returned_ok",None),
        AttemptOutcome::Failed(summary)=>("failed",Some(diagnostic_failure(summary))),
    };
    AttemptDiagnosticDto{schema_version:1,analysis_key:association,availability:"available",
        unavailable_reason:None,freshness:Some(freshness),outcome:Some(outcome),failure}
}
#[cfg(test)]
mod diagnostic_wire_unit_text {
    use super::*;
    #[test] fn diagnostic_unit_canonical_lookup_and_u64_boundaries() {
        assert!(diagnostic_lookup("0")==Some(0));
        assert!(diagnostic_lookup("18446744073709551615")==Some(u64::MAX));
        for key in ["","01","-1","+1"," 1","1.0","18446744073709551616","/public-sentinel"] {assert!(diagnostic_lookup(key).is_none());}
    }
    #[test] fn diagnostic_unit_missing_state_never_reflects_private_input() {
        let bad=serde_json::to_value(attempt_diagnostic_response(None,"/public-sentinel-token".into(),false)).unwrap();
        assert!(bad["schema_version"]==1 && bad["availability"]=="unavailable" && bad["analysis_key"].is_null());
        assert!(!bad.to_string().contains("public-sentinel-token"));
        let missing=serde_json::to_value(attempt_diagnostic_response(None,"42".into(),false)).unwrap();
        assert!(missing["analysis_key"]=="42" && missing["unavailable_reason"]=="not_captured_or_unavailable");
        assert!(missing["outcome"].is_null()&&missing["failure"].is_null());
    }
    #[test] fn diagnostic_unit_unknown_projection_whitelist_only() {
        let value=serde_json::to_value(diagnostic_failure(crate::recovery_set::FailureSummary::unknown())).unwrap();
        assert!(value["class"]=="legacy_unknown"&&value["boundary"]=="unknown"&&value["knowledge"]=="unknown_boundary");
        assert!(value["possible_effects"].as_array().unwrap().len()==8);
        assert!(value["inherited_observed"].as_array().unwrap().is_empty());
        assert!(value.as_object().unwrap().len()==5);
    }
    // UNIT protocol state only: no backend/Lease/history/native authority.
    fn diagnostic_state(outcome:Option<AttemptOutcome>)->State {
        let s=State::new(true);*s.request.lock().unwrap()=Some("diagnostic-unit".into());
        {let _c=s.completed.lock().unwrap();s.epoch.store(42,Ordering::Release);}
        *s.last.lock().unwrap()=Some(PublishedAnalysis{observation:Observation::new(42),binding:None,
            attempt:outcome.map(|outcome|AttemptRecord{analysis_epoch:42,plan_id:"public-unit-plan-not-authority".into(),outcome})});s
    }
    #[test] fn diagnostic_unit_invalid_ascii_lookup_variants_never_echo() {
        for key in ["+0","-0","00","1 ","\t1","1\0","１２","1e2","999999999999999999999"] {
            let value=serde_json::to_value(attempt_diagnostic_response(None,key.into(),false)).unwrap();
            assert!(value["analysis_key"].is_null()&&value["unavailable_reason"]=="invalid_lookup");
            assert!(value["freshness"].is_null()&&value["outcome"].is_null()&&value["failure"].is_null());
        }
    }
    #[test] fn diagnostic_unit_missing_and_shutdown_envelopes_exact_seven_fields() {
        let missing=serde_json::to_value(attempt_diagnostic_response(None,"42".into(),false)).unwrap();
        let expected=serde_json::json!({"schema_version":1,"analysis_key":"42","availability":"unavailable",
            "unavailable_reason":"not_captured_or_unavailable","freshness":null,"outcome":null,"failure":null});
        assert!(missing==expected&&missing.as_object().unwrap().len()==7);
        let state=diagnostic_state(Some(AttemptOutcome::Unconfirmed));
        let shutdown=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),true)).unwrap();
        assert!(shutdown["availability"]=="unavailable"&&shutdown["unavailable_reason"]=="shutdown");
        assert!(shutdown["freshness"].is_null()&&shutdown["outcome"].is_null()&&shutdown["failure"].is_null());
        assert!(shutdown.as_object().unwrap().len()==7);
    }
    #[test] fn diagnostic_unit_class_boundary_knowledge_whitelist_exhaustive() {
        use crate::recovery_set::{FailureClass as C,FailureBoundary as B,FailureKnowledge as K};
        for(value,label)in [(C::LegacyUnknown,"legacy_unknown"),(C::CancellationObserved,"cancellation_observed"),
            (C::PreflightUnconfirmed,"preflight_unconfirmed"),(C::PlanContextChanged,"plan_context_changed"),
            (C::OfferedContextUnconfirmed,"offered_context_unconfirmed"),
            (C::LocalJournalContextUnconfirmed,"local_journal_context_unconfirmed"),
            (C::CatalogMismatchObserved,"catalog_mismatch_observed")] {assert!(diagnostic_class(value)==label);}
        for(value,label)in [(B::ProtocolCheck,"protocol_check"),(B::ReadonlyPreflight,"readonly_preflight"),
            (B::ProtectionRecords,"protection_records"),(B::LocalJournalBinding,"local_journal_binding"),
            (B::CatalogCheck,"catalog_check"),(B::GenerationNamespace,"generation_namespace"),
            (B::ControlWrites,"control_writes"),(B::FinalValidation,"final_validation"),
            (B::CommitRetention,"commit_retention"),(B::Unknown,"unknown")] {assert!(diagnostic_boundary(value)==label);}
        assert!(diagnostic_knowledge(K::ExecutorCaptured)=="executor_captured");
        assert!(diagnostic_knowledge(K::UnknownBoundary)=="unknown_boundary");
    }
    #[test] fn diagnostic_unit_unknown_labels_unique_bounded_and_only_five_fields() {
        let value=serde_json::to_value(diagnostic_failure(crate::recovery_set::FailureSummary::unknown())).unwrap();
        let effects=value["possible_effects"].as_array().unwrap();
        let labels=effects.iter().map(|x|x.as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();
        let expected=std::collections::BTreeSet::from(["protection","live_generation_namespace","control_temporary",
            "canonical_control","retention","backup_namespace","validator_or_child_unknown","opaque_callback"]);
        assert!(labels==expected&&effects.len()==8&&value.as_object().unwrap().len()==5);
        assert!(value["inherited_observed"].as_array().unwrap().is_empty());
    }
    #[test] fn diagnostic_unit_real_getter_outcomes_current_and_stale_no_authority() {
        for(outcome,label)in [(None,"not_attempted"),(Some(AttemptOutcome::Unconfirmed),"unconfirmed"),
            (Some(AttemptOutcome::ReturnedOk),"state_returned_ok"),
            (Some(AttemptOutcome::Failed(crate::recovery_set::FailureSummary::unknown())),"failed")] {
            let state=diagnostic_state(outcome);
            let value=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),false)).unwrap();
            assert!(value["availability"]=="available"&&value["freshness"]=="current"&&value["outcome"]==label);
            assert!(value["failure"].is_null()==(label!="failed"));
            assert!(value.as_object().unwrap().len()==7&&value.get("can_execute").is_none()&&value.get("can_reopen").is_none());
            assert!(state.completed_candidate("public-unit-plan-not-authority").is_err());
            if outcome.is_some() {
                state.cancel("diagnostic-unit");
                let stale=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),false)).unwrap();
                assert!(stale["freshness"]=="stale"&&stale["outcome"]==label);
            }
        }
    }
    #[test] fn diagnostic_unit_wronglookup_and_request_poison_coalesce_no_terminal() {
        let state=diagnostic_state(Some(AttemptOutcome::Unconfirmed));
        let wrong=serde_json::to_value(attempt_diagnostic_response(Some(&state),"43".into(),false)).unwrap();
        assert!(wrong["analysis_key"]=="43"&&wrong["unavailable_reason"]=="not_captured_or_unavailable"&&wrong["outcome"].is_null());
        let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{let _g=state.request.lock().unwrap();panic!("public unit request poison");}));assert!(caught.is_err());
        let poisoned=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),false)).unwrap();
        assert!(poisoned["unavailable_reason"]=="not_captured_or_unavailable"&&poisoned["outcome"].is_null()&&poisoned["failure"].is_null());
        assert!(state.request.is_poisoned());
    }
    #[test] fn diagnostic_unit_last_poison_unavailable_and_shutdown_shortcircuits() {
        let state=diagnostic_state(Some(AttemptOutcome::Unconfirmed));
        let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{let _g=state.last.lock().unwrap();panic!("public unit last poison");}));assert!(caught.is_err());
        let poisoned=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),false)).unwrap();
        assert!(poisoned["unavailable_reason"]=="not_captured_or_unavailable"&&poisoned["freshness"].is_null());
        let shutdown=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),true)).unwrap();
        assert!(shutdown["unavailable_reason"]=="shutdown"&&state.last.is_poisoned());
    }
    #[test] fn diagnostic_unit_query_does_not_mutate_claim_or_completion_or_report_v1() {
        let state=diagnostic_state(Some(AttemptOutcome::Failed(crate::recovery_set::FailureSummary::unknown())));
        let before=state.report(42,||false).unwrap();let value=serde_json::to_value(attempt_diagnostic_response(Some(&state),"42".into(),false)).unwrap();
        assert!(state.report(42,||false).unwrap()==before&&state.epoch.load(Ordering::Acquire)==42);
        assert!(state.admission(42,"public-unit-plan-not-authority").is_err());
        assert!(state.completed_candidate("public-unit-plan-not-authority").is_err());
        let object=value.as_object().unwrap();
        for forbidden in ["plan_id","observed_profile","root","path","origin","binding","rawerror","token","config","queue","can_execute","can_reopen","completed"] {assert!(!object.contains_key(forbidden));}
        assert!(!value.to_string().contains("public-unit-plan-not-authority"));
        state.cancel("diagnostic-unit");assert!(state.report(42,||false).is_err());
    }

}
