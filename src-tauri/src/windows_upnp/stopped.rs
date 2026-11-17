//! SetAVTransportURI is preparation, never implicit Play or Seek. A failed
//! cut leaves the previous route paused; uncertain I/O never auto-resumes it.
use crate::windows_mpd::{
    protocol::{Playback, Result},
    transaction::Route,
};
pub(crate) struct Outcome {
    pub result: Result<Playback>,
    pub retired_previous: bool,
}
pub(crate) fn prepare<T: Route>(
    next: &mut T,
    mut old: Option<&mut T>,
    current: impl Fn() -> bool,
    commit: impl FnOnce() -> Result<()>,
) -> Outcome {
    let mut retired = false;
    let mut close_attempted = false;
    let mut touched = false;
    let result = (|| {
        if !current() {
            return Err("UPNP_SESSION_CHANGED");
        }
        let before = next.observe()?;
        if before.state != "stop" {
            return Err("UPNP_PREPARE_UNCONFIRMED");
        }
        // Candidate proven before disturbing the old session. Never start/resume/seek.
        if let Some(old) = old.as_deref_mut() {
            touched = true;
            if old.observe()?.state == "play" {
                old.hold_for_replacement()?;
            }
            old.suspend_output()?;
        }
        if !current() {
            return Err("UPNP_SESSION_CHANGED");
        }
        // Do not erase the durable queue until ownership cleanup is confirmed. If a
        // later durable write fails, the actor falls back to that old logical queue
        // PAUSED, without replaying it or claiming the receiver accepted new media.
        if let Some(old) = old.as_deref_mut() {
            close_attempted = true;
            old.abort()?;
            retired = true;
        }
        if !current() {
            return Err("UPNP_SESSION_CHANGED");
        }
        commit()?;
        Ok(before)
    })();
    let result = if result.is_err() {
        match next.abort() {
            Err(_) => Err("rollback_failed"),
            Ok(()) => {
                if touched && !close_attempted {
                    if let Some(old) = old {
                        if old.restore_output().is_err() {
                            return Outcome {
                                result: Err("rollback_failed"),
                                retired_previous: retired,
                            };
                        }
                    }
                }
                result
            }
        }
    } else {
        result
    };
    Outcome {
        result,
        retired_previous: retired,
    }
}
#[cfg(test)]
#[path = "stopped_tests.rs"]
mod tests;
