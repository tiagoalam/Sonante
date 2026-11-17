//! Presentation-host leases for the original shared Now Playing UI.
//! Leases authorize presentation only; native observations still require a real source.
use serde::Serialize;
use std::sync::Mutex;
pub type Result<T> = std::result::Result<T, &'static str>;
const INVALID: &str = "windows_player_host_invalid";
const STALE: &str = "windows_player_host_stale";
const UNAVAILABLE: &str = "windows_player_host_unavailable";
pub const STATUS_EVENT: &str = "now-playing://analyzer-status";
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub host: String,
    pub session_id: String,
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Status {
    available: bool,
    reason: &'static str,
}
#[derive(Clone, Serialize)]
pub struct Envelope {
    pub(crate) owner: Owner,
    data: Status,
}
fn authorized(host: &str) -> bool {
    matches!(host, "main" | "now-playing")
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128
}
pub fn exits_on_close(host: &str) -> bool {
    host == "main"
}
#[derive(Default)]
struct Ownership {
    owner: Option<Owner>,
    generation: u64,
    active_lease: Option<String>,
    stopping: bool,
}
impl Ownership {
    fn matches(&self, host: &str, session: &str, generation: u64) -> bool {
        self.owner.as_ref().is_some_and(|o| {
            o.host == host && o.session_id == session && o.generation == generation
        })
    }
    fn claim(&mut self, caller: &str, host: &str, session: String, exists: bool) -> Result<Owner> {
        if self.stopping {
            return Err(UNAVAILABLE);
        }
        if caller != "main" || !authorized(host) || !valid_id(&session) || !exists {
            return Err(INVALID);
        }
        self.generation = self.generation.checked_add(1).ok_or(UNAVAILABLE)?;
        let owner = Owner {
            host: host.into(),
            session_id: session,
            generation: self.generation,
            lease_id: None,
        };
        self.owner = Some(owner.clone());
        self.active_lease = None;
        Ok(owner)
    }
    fn start(
        &mut self,
        caller: &str,
        session: &str,
        generation: u64,
        lease: String,
    ) -> Result<Envelope> {
        if self.stopping {
            return Err(UNAVAILABLE);
        }
        if !self.matches(caller, session, generation) || !valid_id(&lease) {
            return Err(STALE);
        }
        self.active_lease = Some(lease.clone());
        let mut owner = self.owner.clone().ok_or(STALE)?;
        owner.lease_id = Some(lease);
        Ok(Envelope {
            owner,
            data: Status {
                available: false,
                reason: "windows_pending",
            },
        })
    }
    fn stop(&mut self, caller: &str, session: &str, generation: u64, lease: &str) -> Result<()> {
        if !authorized(caller) {
            return Err(INVALID);
        }
        // Old StrictMode cleanup and handoff events must never clear the new lease.
        if self.matches(caller, session, generation) && self.active_lease.as_deref() == Some(lease)
        {
            self.active_lease = None;
        }
        Ok(())
    }
    fn release(&mut self, caller: &str, session: &str, generation: u64) -> Result<()> {
        if caller != "main" {
            return Err(INVALID);
        }
        if self
            .owner
            .as_ref()
            .is_some_and(|o| o.session_id == session && o.generation == generation)
        {
            self.owner = None;
            self.active_lease = None;
        }
        Ok(())
    }
    fn closed(&mut self, host: &str) {
        if self.owner.as_ref().is_some_and(|o| o.host == host) {
            self.owner = None;
            self.active_lease = None;
        }
    }
}
#[derive(Default)]
pub struct State(Mutex<Ownership>);
impl State {
    /// Observation polling must never wait for IPC ownership mutations.
    pub(crate) fn is_current(&self,owner:&Owner)->bool{
        self.0.try_lock().ok().is_some_and(|s|!s.stopping&&s.matches(&owner.host,&owner.session_id,owner.generation)&&owner.lease_id.is_some()&&s.active_lease==owner.lease_id)
    }
    pub fn claim(
        &self,
        caller: &str,
        host: &str,
        session: String,
        exists: impl FnOnce() -> bool,
    ) -> Result<Owner> {
        self.0
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .claim(caller, host, session, exists())
    }
    pub fn start(
        &self,
        caller: &str,
        session: &str,
        generation: u64,
        lease: String,
    ) -> Result<Envelope> {
        self.0
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .start(caller, session, generation, lease)
    }
    pub fn stop(&self, caller: &str, session: &str, generation: u64, lease: &str) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .stop(caller, session, generation, lease)
    }
    pub fn release(&self, caller: &str, session: &str, generation: u64) -> Result<()> {
        self.0
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .release(caller, session, generation)
    }
    pub fn closed(&self, host: &str) -> Result<()> {
        self.0.lock().map_err(|_| UNAVAILABLE)?.closed(host);
        Ok(())
    }
    pub fn shutdown(&self) -> Result<()> {
        let mut state = self.0.lock().map_err(|_| UNAVAILABLE)?;
        state.stopping = true;
        state.owner = None;
        state.active_lease = None;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn observation_checks_the_actual_current_lease_without_waiting_for_ipc(){
        let state=State::default();let owner=state.claim("main","main","fixture".into(),||true).unwrap();assert!(!state.is_current(&owner));
        let first=state.start("main","fixture",owner.generation,"first".into()).unwrap().owner;assert!(state.is_current(&first));
        let second=state.start("main","fixture",owner.generation,"second".into()).unwrap().owner;assert!(!state.is_current(&first));assert!(state.is_current(&second));
        {let _held=state.0.lock().unwrap();let at=std::time::Instant::now();assert!(!state.is_current(&second));assert!(at.elapsed()<std::time::Duration::from_millis(20));}
        state.stop("main","fixture",owner.generation,"first").unwrap();assert!(state.is_current(&second));state.shutdown().unwrap();assert!(!state.is_current(&second));
    }
    #[test]
    fn main_is_only_transfer_authority_and_hosts_ids_and_presence_are_checked() {
        let state = State::default();
        for caller in ["now-playing", "other"] {
            assert_eq!(
                state
                    .claim(caller, "main", "s".into(), || true)
                    .unwrap_err(),
                INVALID
            );
        }
        for host in ["", "other"] {
            assert_eq!(
                state.claim("main", host, "s".into(), || true).unwrap_err(),
                INVALID
            );
        }
        for id in [String::new(), "x".repeat(129)] {
            assert_eq!(
                state.claim("main", "main", id, || true).unwrap_err(),
                INVALID
            );
        }
        assert_eq!(
            state
                .claim("main", "now-playing", "s".into(), || false)
                .unwrap_err(),
            INVALID
        );
        let owner = state.claim("main", "main", "s".into(), || true).unwrap();
        assert_eq!(owner.generation, 1);
        assert!(owner.lease_id.is_none());
    }
    #[test]
    fn capability_ack_is_explicitly_unavailable_with_real_lease_and_no_frames() {
        let state = State::default();
        let owner = state.claim("main", "main", "s".into(), || true).unwrap();
        let status = state
            .start("main", "s", owner.generation, "lease".into())
            .unwrap();
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["data"]["available"], false);
        assert_eq!(value["data"]["reason"], "windows_pending");
        assert_eq!(value["owner"]["leaseId"], "lease");
        assert!(value.get("levels").is_none());
        for (caller, session, generation) in [
            ("now-playing", "s", 1),
            ("main", "old", 1),
            ("main", "s", 0),
        ] {
            assert!(state
                .start(caller, session, generation, "lease".into())
                .is_err());
        }
        for lease in [String::new(), "x".repeat(129)] {
            assert!(state.start("main", "s", 1, lease).is_err());
        }
    }
    #[test]
    fn handoff_fences_old_starts_stops_release_and_window_close() {
        let state = State::default();
        let old = state.claim("main", "main", "one".into(), || true).unwrap();
        state
            .start("main", "one", old.generation, "lease-old".into())
            .unwrap();
        let next = state
            .claim("main", "now-playing", "two".into(), || true)
            .unwrap();
        assert!(next.generation > old.generation);
        state
            .start("now-playing", "two", next.generation, "lease-new".into())
            .unwrap();
        assert!(state
            .start("main", "one", old.generation, "late".into())
            .is_err());
        state
            .stop("main", "one", old.generation, "lease-old")
            .unwrap();
        state.release("main", "one", old.generation).unwrap();
        state.closed("main").unwrap();
        let current = state.0.lock().unwrap();
        assert_eq!(current.owner.as_ref().unwrap().session_id, "two");
        assert_eq!(current.active_lease.as_deref(), Some("lease-new"));
        drop(current);
        assert!(state
            .release("now-playing", "two", next.generation)
            .is_err());
        state.closed("now-playing").unwrap();
        assert!(state.0.lock().unwrap().owner.is_none());
    }
    #[test]
    fn strict_mode_lease_cleanup_cannot_stop_replacement_and_shutdown_cannot_rearm() {
        let state = State::default();
        state.claim("main", "main", "s".into(), || true).unwrap();
        state.start("main", "s", 1, "first".into()).unwrap();
        state.start("main", "s", 1, "second".into()).unwrap();
        state.stop("main", "s", 1, "first").unwrap();
        assert_eq!(
            state.0.lock().unwrap().active_lease.as_deref(),
            Some("second")
        );
        state.stop("main", "s", 1, "second").unwrap();
        assert!(state.0.lock().unwrap().active_lease.is_none());
        state.shutdown().unwrap();
        assert!(state.claim("main", "main", "s".into(), || true).is_err());
        assert!(state.start("main", "s", 1, "late".into()).is_err());
        state.stop("main", "s", 1, "second").unwrap();
        state.release("main", "s", 1).unwrap();
        assert!(state.0.lock().unwrap().owner.is_none());
    }
    #[test]
    fn main_close_ends_application_detached_close_preserves_audio_session() {
        assert!(exits_on_close("main"));
        assert!(!exits_on_close("now-playing"));
        assert!(!exits_on_close("other"));
    }
}
