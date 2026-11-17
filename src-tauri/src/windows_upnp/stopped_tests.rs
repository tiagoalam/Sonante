use super::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
struct Recorder {
    state: &'static str,
    role: &'static str,
    log: Rc<RefCell<Vec<String>>>,
    fail: Option<&'static str>,
}
impl Recorder {
    fn new(state: &'static str, role: &'static str, log: &Rc<RefCell<Vec<String>>>) -> Self {
        Self {
            state,
            role,
            log: log.clone(),
            fail: None,
        }
    }
    fn event(&self, method: &'static str) -> Result<()> {
        self.log
            .borrow_mut()
            .push(format!("{}:{method}", self.role));
        if self.fail == Some(method) {
            Err("own_fixture_failure")
        } else {
            Ok(())
        }
    }
}
impl Route for Recorder {
    fn observe(&mut self) -> Result<Playback> {
        self.event("observe")?;
        Ok(Playback {
            state: self.state.into(),
            elapsed: 12.,
            duration: 90.,
            volume: 42,
            audio: String::new(),
        })
    }
    fn stop(&mut self) -> Result<()> {
        self.event("stop")?;
        self.state = "stop";
        Ok(())
    }
    fn pause(&mut self) -> Result<()> {
        self.event("pause")?;
        self.state = "pause";
        Ok(())
    }
    fn start(&mut self) -> Result<()> {
        self.event("FORBIDDEN_PLAY")
    }
    fn seek(&mut self, _: f64) -> Result<()> {
        self.event("FORBIDDEN_SEEK")
    }
    fn resume_at(&mut self, _: f64) -> Result<()> {
        self.event("FORBIDDEN_RESUME")
    }
    fn abort(&mut self) -> Result<()> {
        self.event("abort")?;
        self.state = "stop";
        Ok(())
    }
    fn hold_for_replacement(&mut self) -> Result<()> {
        self.event("hold")?;
        self.state = "pause";
        Ok(())
    }
    fn suspend_output(&mut self) -> Result<()> {
        self.event("suspend")
    }
    fn restore_output(&mut self) -> Result<()> {
        self.event("restore_output")
    }
}
#[test]
fn receiving_a_uri_is_stopped_and_never_plays_or_seeks_either_route() {
    for state in ["play", "pause", "stop"] {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut old = Recorder::new(state, "old", &log);
        let mut next = Recorder::new("stop", "new", &log);
        let outcome = prepare(
            &mut next,
            Some(&mut old),
            || true,
            || {
                log.borrow_mut().push("durable_commit".into());
                Ok(())
            },
        );
        assert_eq!(outcome.result.unwrap().state, "stop");
        assert!(outcome.retired_previous);
        assert!(log.borrow().iter().all(|e| !e.contains("FORBIDDEN")));
        let events = log.borrow();
        assert!(
            events.iter().position(|e| e == "old:abort").unwrap()
                < events.iter().position(|e| e == "durable_commit").unwrap()
        );
    }
}
#[test]
fn candidate_failure_never_disturbs_the_old_queue_or_checkpoint() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut old = Recorder::new("play", "old", &log);
    let mut next = Recorder::new("stop", "new", &log);
    next.fail = Some("observe");
    let committed = Cell::new(false);
    let result = prepare(
        &mut next,
        Some(&mut old),
        || true,
        || {
            committed.set(true);
            Ok(())
        },
    );
    assert!(result.result.is_err());
    assert!(!result.retired_previous);
    assert!(!committed.get());
    assert_eq!(old.state, "play");
    assert!(log
        .borrow()
        .iter()
        .all(|e| !e.contains("FORBIDDEN") && !e.starts_with("old:")));
}
#[test]
fn an_uncertain_old_cleanup_keeps_its_durable_checkpoint_and_never_accepts_new_uri() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut old = Recorder::new("play", "old", &log);
    old.fail = Some("abort");
    let mut next = Recorder::new("stop", "new", &log);
    let committed = Cell::new(false);
    let outcome = prepare(
        &mut next,
        Some(&mut old),
        || true,
        || {
            committed.set(true);
            Ok(())
        },
    );
    assert!(outcome.result.is_err());
    assert!(!outcome.retired_previous);
    assert!(!committed.get());
    assert_eq!(old.state, "pause");
    assert!(log.borrow().contains(&"new:abort".into()));
}
#[test]
fn failed_durable_write_after_cut_requires_paused_logical_fallback_and_no_replay() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut old = Recorder::new("play", "old", &log);
    let mut next = Recorder::new("stop", "new", &log);
    let outcome = prepare(
        &mut next,
        Some(&mut old),
        || true,
        || Err("queue_save_failed"),
    );
    assert_eq!(outcome.result.err(), Some("queue_save_failed"));
    assert!(outcome.retired_previous);
    assert_eq!(old.state, "stop");
    assert!(log.borrow().iter().all(|e| !e.contains("FORBIDDEN")));
}
#[test]
fn revoked_while_holding_the_old_route_never_erases_the_checkpoint_or_autoplays() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut old = Recorder::new("play", "old", &log);
    let mut next = Recorder::new("stop", "new", &log);
    let calls = Cell::new(0);
    let committed = Cell::new(false);
    let outcome = prepare(
        &mut next,
        Some(&mut old),
        || {
            calls.set(calls.get() + 1);
            calls.get() == 1
        },
        || {
            committed.set(true);
            Ok(())
        },
    );
    assert_eq!(outcome.result.err(), Some("UPNP_SESSION_CHANGED"));
    assert!(!outcome.retired_previous);
    assert!(!committed.get());
    assert_eq!(old.state, "pause");
    assert!(log.borrow().iter().all(|e| !e.contains("FORBIDDEN")));
}
#[test]
fn failed_candidate_cleanup_is_explicit_and_does_not_report_the_original_error_as_safe() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut next = Recorder::new("play", "new", &log);
    next.fail = Some("abort");
    let outcome = prepare(&mut next, None, || true, || panic!("must not commit"));
    assert_eq!(outcome.result.err(), Some("rollback_failed"));
}
