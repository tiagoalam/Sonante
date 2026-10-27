//! A backend-only, revocable permission for the owned initial Shared MPD.
//! No mutex is retained across Plex HTTP. A synchronous effect holds only its
//! session read lease; revocation drains it before the owned child is stopped.
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::{cell::RefCell, sync::{Arc, Mutex, RwLock, RwLockReadGuard, atomic::{AtomicBool, Ordering}}};
pub(crate) const WAIT: &str = "FIRST_RUN_CATALOG_INDEXING";
const STALE: &str = "PLAYBACK_SUPERSEDED";
thread_local! { static EFFECT: RefCell<Option<Arc<Session>>> = const { RefCell::new(None) }; }

pub(crate) struct Session {
    pub(crate) operation: String,
    socket: String,
    pid: u32,
    process: OwnedFd,
    live: AtomicBool,
    effects: RwLock<()>,
    // Only URIs returned by the verified Plex resolver, never frontend URLs.
    // RAM only; never Debug/Serialize/log this list (it may contain credentials).
    queue: Mutex<Vec<String>>,
}
pub(crate) struct Effect<'a> { _lease: RwLockReadGuard<'a, ()> }
impl Drop for Effect<'_> { fn drop(&mut self) { EFFECT.with(|value| *value.borrow_mut() = None); } }
impl Session {
    fn available(&self) -> bool {
        if !self.live.load(Ordering::Acquire) { return false; }
        let mut observed = libc::pollfd { fd: self.process.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // A retained pidfd observes this process even if its numeric PID is reused.
        unsafe { libc::poll(&mut observed, 1, 0) == 0 }
    }
    pub(crate) fn enter(self: &Arc<Self>) -> Result<Effect<'_>, String> {
        let lease = self.effects.read().map_err(|_| STALE)?;
        if !self.available() { return Err(STALE.into()); }
        EFFECT.with(|value| {
            let mut value = value.borrow_mut();
            if value.is_some() { return Err(STALE.into()); }
            *value = Some(self.clone());
            Ok(Effect { _lease: lease })
        })
    }
    pub(crate) fn accepts_queue(&self, actual: &[String]) -> Result<(), String> {
        if *self.queue.lock().map_err(|_| STALE)? == actual { Ok(()) } else { Err("PLAYBACK_QUEUE_UNCERTAIN".into()) }
    }
    pub(crate) fn confirm_queue(&self, actual: Vec<String>) -> Result<(), String> {
        *self.queue.lock().map_err(|_| STALE)? = actual;
        Ok(())
    }
}
/// Consulted at every MPD command, including Shared seek and internal recovery.
/// A command cannot reconnect to another socket/PID after revocation.
pub(crate) fn check_effect() -> Result<(), String> {
    EFFECT.with(|value| if value.borrow().as_ref().is_none_or(|session| session.available()) { Ok(()) } else { Err(STALE.into()) })
}
pub(crate) fn command_peer(socket: &str) -> Result<Option<u32>, String> {
    EFFECT.with(|value| match value.borrow().as_ref() {
        None => Ok(None),
        Some(session) if session.available() && session.socket == socket => Ok(Some(session.pid)),
        Some(_) => Err(STALE.into()),
    })
}
#[derive(Default)]
struct Binding { current: Option<Arc<Session>>, intent: Option<(String, Arc<Session>)> }
#[derive(Default)]
pub(crate) struct Gate { binding: Mutex<Binding> }
impl Gate {
    pub(crate) fn publish(&self, operation: String, socket: String, pid: u32) -> Result<(), String> {
        if operation.is_empty() || socket.is_empty() || pid == 0 { return Err(WAIT.into()); }
        let process = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if process < 0 { return Err(WAIT.into()); }
        let process = unsafe { OwnedFd::from_raw_fd(process as i32) };
        let mut binding = self.binding.lock().map_err(|_| STALE)?;
        if binding.current.as_ref().is_some_and(|s| s.live.load(Ordering::Acquire)) { return Err(WAIT.into()); }
        binding.current = Some(Arc::new(Session { operation, socket, pid, process, live: AtomicBool::new(true), effects: RwLock::new(()), queue: Mutex::new(Vec::new()) }));
        binding.intent = None;
        Ok(())
    }
    pub(crate) fn current(&self) -> Result<Option<Arc<Session>>, String> {
        Ok(self.binding.lock().map_err(|_| STALE)?.current.clone().filter(|s| s.available()))
    }
    pub(crate) fn bind(&self, token: &str, session: Arc<Session>) -> Result<(), String> {
        let mut binding = self.binding.lock().map_err(|_| STALE)?;
        if !session.available() || !binding.current.as_ref().is_some_and(|s| Arc::ptr_eq(s, &session)) { return Err(STALE.into()); }
        binding.intent = Some((token.into(), session));
        Ok(())
    }
    pub(crate) fn bound(&self, token: &str) -> Result<Option<Arc<Session>>, String> {
        let binding = self.binding.lock().map_err(|_| STALE)?;
        match &binding.intent {
            Some((expected, session)) if expected == token => {
                if !session.available() || !binding.current.as_ref().is_some_and(|s| Arc::ptr_eq(s, session)) { return Err(STALE.into()); }
                Ok(Some(session.clone()))
            }
            _ => Ok(None),
        }
    }
    pub(crate) fn clear_intent(&self) { self.binding.lock().unwrap_or_else(std::sync::PoisonError::into_inner).intent = None; }
    /// Nonblocking: safe even when shutdown is requested by an effect callback.
    pub(crate) fn invalidate(&self) {
        if let Some(session) = self.binding.lock().unwrap_or_else(std::sync::PoisonError::into_inner).current.as_ref() {
            session.live.store(false, Ordering::Release);
        }
    }
    /// Failure revokes first, drains without application locks, invalidates the
    /// logical queue/intents and only then stops the owned child. Retain both
    /// cleanup errors when neither operation can be confirmed.
    pub(crate) fn revoke_and_stop(&self, invalidate: impl FnOnce() -> Result<(), String>,
        stop_owned: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        self.invalidate();
        self.drain()?;
        let invalidated = invalidate();
        let stopped = stop_owned();
        match (invalidated, stopped) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(first), Err(second)) => Err(format!("{first}; {second}")),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    /// Caller holds no Audio, Supervisor or Coordinator lock.
    pub(crate) fn drain(&self) -> Result<(), String> {
        let session = self.binding.lock().map_err(|_| STALE)?.current.clone();
        if let Some(session) = session { let _drained = session.effects.write().map_err(|_| STALE)?; }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Arc<Gate>, Arc<Session>) {
        let gate = Arc::new(Gate::default());
        gate.publish("initial-1".into(), "fixture.sock".into(), std::process::id()).unwrap();
        let session = gate.current().unwrap().unwrap();
        (gate, session)
    }
    #[test]
    fn permission_is_bound_to_current_session_token_and_peer() {
        let (gate, session) = fixture();
        gate.bind("first", session.clone()).unwrap();
        assert!(gate.bound("first").unwrap().is_some());
        gate.bind("second", session.clone()).unwrap();
        assert!(gate.bound("first").unwrap().is_none());
        let (_, other) = fixture();
        assert!(gate.bind("second", other).is_err());
        let lease = session.enter().unwrap();
        assert_eq!(command_peer("fixture.sock").unwrap(), Some(std::process::id()));
        assert!(command_peer("other.sock").is_err());
        assert!(session.enter().is_err());
        drop(lease);
        assert_eq!(command_peer("fixture.sock").unwrap(), None);
        gate.clear_intent();
        assert!(gate.bound("second").unwrap().is_none());
    }
    #[test]
    fn queue_proof_never_accepts_an_added_or_reordered_uri() {
        let (_, session) = fixture();
        session.accepts_queue(&[]).unwrap();
        let known = vec!["verified-one".into(), "verified-two".into()];
        session.confirm_queue(known.clone()).unwrap();
        session.accepts_queue(&known).unwrap();
        assert!(session.accepts_queue(&["unverified-http-or-local".into()]).is_err());
        assert!(session.accepts_queue(&[known[1].clone(), known[0].clone()]).is_err());
        assert!(session.accepts_queue(&[]).is_err());
    }
    #[test]
    fn revocation_is_immediate_and_drain_waits_for_the_scoped_effect() {
        let (gate, session) = fixture();
        gate.bind("pending", session.clone()).unwrap();
        let effect = session.enter().unwrap();
        gate.invalidate(); // Does not self-deadlock while this thread owns an effect.
        assert!(check_effect().is_err());
        assert!(command_peer("fixture.sock").is_err());
        assert!(gate.bound("pending").is_err());
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || { gate.drain().unwrap(); send.send(()).unwrap(); });
        assert!(receive.recv_timeout(std::time::Duration::from_millis(20)).is_err());
        drop(effect);
        receive.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        assert!(session.enter().is_err());
        assert!(check_effect().is_ok());
    }
    #[test]
    fn failure_invalidates_before_owned_stop_and_preserves_both_errors() {
        let (gate, session) = fixture();
        gate.bind("pending", session).unwrap();
        let order = RefCell::new(Vec::new());
        let result = gate.revoke_and_stop(|| {
            assert!(gate.bound("pending").is_err());
            order.borrow_mut().push("invalidate");
            Err("logical-state-unconfirmed".into())
        }, || {
            assert_eq!(*order.borrow(), vec!["invalidate"]);
            order.borrow_mut().push("stop-owned");
            Err("owned-stop-unconfirmed".into())
        });
        let error = result.unwrap_err();
        assert!(error.contains("logical-state-unconfirmed"));
        assert!(error.contains("owned-stop-unconfirmed"));
        assert_eq!(*order.borrow(), vec!["invalidate", "stop-owned"]);
    }
    #[test]
    fn exited_owned_process_cannot_be_replaced_by_a_reused_numeric_pid() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let gate = Gate::default();
        gate.publish("initial-child".into(), "fixture.sock".into(), child.id()).unwrap();
        let session = gate.binding.lock().unwrap().current.clone().unwrap();
        child.wait().unwrap();
        assert!(!session.available());
        assert!(session.enter().is_err());
    }
}
