//! Windows bindings for the original qualified Shared equalizer. Merely
//! loading this module/preset data starts no engine, listener or audio stream.
// Candidate3 has audited corresponding sources, pinned imports and cross-build
// gates. Native Windows IPC/WASAPI and hardware are separate pending gates.
pub(crate) const CAMILLA_SHA:Option<&str>=Some("3f6c997114204aba09368eb8b198f8c6ea6a8768d10ad463b560bfb54fbfac1d");
// Exact corresponding candidate3 source/build qualified; native testing pending.
pub(crate) const CAMILLA_ANALYZER_PROTOCOL:Option<u8>=Some(1);
#[path = "../shared_dsp/peq.rs"]
pub(crate) mod peq;
#[path = "../shared_dsp/safety.rs"]
pub(crate) mod safety;
pub(crate) mod binding;
pub(crate) mod runtime;
pub(crate) mod pcm_core;
pub(crate) mod ring;
pub(crate) mod chain;
pub(crate) mod staged;
use crate::equalizer::EqPreset;
use serde::Serialize;
use std::sync::atomic::{AtomicU64,Ordering};
use chain::Chain as Session;
pub(crate) mod comparison;
mod snapshot;
pub(crate) use snapshot::{Error,Phase,Snapshot};
fn pcm()->peq::PcmFormat{chain::pcm()}
pub(crate) fn compare(chain:&mut chain::Chain,old:&Snapshot,action:comparison::Action,token:Option<String>,expected:Option<EqPreset>,watch:Option<crate::audio::EqTransitionWatch>)->Result<Snapshot,Error>{
    chain.compare(old,action,token,expected,watch)
}
pub(crate) fn error_code(error:Error)->&'static str{
    match error {
        Error::Busy=>"EQ_BUSY",Error::Cancelled=>"EQ_CANCELLED",
        Error::ComparisonStale=>"EQ_COMPARISON_STALE",
        Error::ComparisonInterrupted=>"EQ_COMPARISON_INTERRUPTED",
        Error::ComparisonUnavailable=>"EQ_COMPARISON_UNAVAILABLE",
        Error::ApplyFailed=>"EQ_APPLY_FAILED",Error::RollbackFailed=>"EQ_ROLLBACK_FAILED",
        Error::RuntimeExited=>"EQ_RUNTIME_EXITED",Error::ContinuityLost=>"EQ_CONTINUITY_LOST",
        _=>"EQ_RUNTIME_UNCONFIRMED",
    }
}
pub(crate) mod action;
pub(crate) mod catalog;
pub(crate) mod cleanup;
#[cfg(all(test,target_os="windows"))]
pub(crate) mod qualification;
