//! Original public EQ status contract. Windows supplies its owned route evidence.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Error {
    Busy,
    RuntimeMissing,
    RuntimeUntrusted,
    RuntimeStartFailed,
    RuntimeExited,
    ControlFailed,
    ApplyFailed,
    RollbackFailed,
    PipeWireUnavailable,
    SinkUnavailable,
    NodeMissing,
    GraphInvalid,
    Cancelled,
    CleanupFailed,
    ContinuityLost,
    ComparisonStale,
    ComparisonInterrupted,
    ComparisonUnavailable,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EQ_{self:?}")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Off,
    Preparing,
    Ready,
    Active,
    Updating,
    Stopping,
    Error,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Snapshot {
    pub comparison: Option<comparison::Comparison>,
    pub protection: Option<safety::Protection>,
    pub transition: Option<String>,
    pub in_path: Option<bool>,
    pub revision: u64,
    pub phase: Phase,
    pub applied: Option<EqPreset>,
    pub error: Option<Error>,
    pub pcm_rate: u32,
    pub pcm_channels: u8,
    pub verified: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            comparison: None,
            protection: None,
            transition: None,
            in_path: None,
            revision: 0,
            phase: Phase::Off,
            applied: None,
            error: None,
            pcm_rate: 48000,
            pcm_channels: 2,
            verified: false,
        }
    }
}
impl From<&'static str> for Error {
 fn from(code:&'static str)->Self{match code{
  "EQ_CANCELLED"=>Self::Cancelled,"EQ_CONTINUITY_LOST"=>Self::ContinuityLost,
  "EQ_ROLLBACK_FAILED"=>Self::RollbackFailed,"EQ_RUNTIME_EXITED"=>Self::RuntimeExited,
  "EQ_COMPARISON_STALE"=>Self::ComparisonStale,_=>Self::ControlFailed,
 }}
}
