//! Windows system-media adapter: bounded messages and existing transport only.
//! Publication is informational and never dispatches a playback command.
use crate::windows_mpd::library::Track;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
type Result<T> = std::result::Result<T, &'static str>;
pub const ERROR_EVENT: &str = "windows-system-media-error";
const UNAVAILABLE: &str = "windows_system_media_unavailable";
const LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Play,
    Pause,
    Next,
    Previous,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Playing(bool),
    Next,
    Previous,
}
impl Button {
    pub(crate) fn target(self) -> Target {
        match self {
            Self::Play => Target::Playing(true),
            Self::Pause => Target::Playing(false),
            Self::Next => Target::Next,
            Self::Previous => Target::Previous,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Playback {
    Playing,
    Paused,
    Stopped,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Projection {
    Unavailable,
    Track {
        generation: u64,
        index: usize,
        playback: Playback,
        title: String,
        artist: String,
        album: String,
    },
}
fn text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(512)
        .collect()
}
impl Projection {
    fn from_snapshot(
        generation: u64,
        running: bool,
        state: &str,
        track: Option<&Track>,
        index: usize,
    ) -> Self {
        let Some(track) = track else {
            return Self::Unavailable;
        };
        let playback = match state {
            "play" | "pause" | "stop" | "idle" if !running => Playback::Paused,
            "play" => Playback::Playing,
            "pause" => Playback::Paused,
            "stop" => Playback::Stopped,
            _ => return Self::Unavailable,
        };
        Self::Track {
            generation,
            index,
            playback,
            title: text(&track.title),
            artist: text(&track.artist),
            album: text(&track.album),
        }
    }
    fn has_track(&self) -> bool {
        matches!(self, Self::Track { .. })
    }
}
#[derive(Clone, Debug)]
pub struct Request {
    button: Button,
    token: String,
}
struct Inner {
    revision: u64,
    snapshot: Projection,
    commands: VecDeque<Request>,
}
pub struct View {
    inner: Mutex<Inner>,
    changed: Condvar,
    stopping: AtomicBool,
    ready: AtomicBool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                revision: 0,
                snapshot: Projection::Unavailable,
                commands: VecDeque::new(),
            }),
            changed: Condvar::new(),
            stopping: AtomicBool::new(false),
            ready: AtomicBool::new(false),
        }
    }
}
struct Work {
    revision: u64,
    snapshot: Projection,
    request: Option<Request>,
}
struct ReadyOnDrop(Arc<View>);
impl Drop for ReadyOnDrop {
    fn drop(&mut self) {
        self.0.ready(false);
    }
}
pub struct ClearOnDrop(pub Arc<View>);
impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        if let Err(code) = self.0.invalidate() {
            if code != "shutting_down" {
                eprintln!("Windows system media: worker_invalidation_failed");
            }
        }
    }
}
impl View {
    pub fn publish(
        &self,
        generation: u64,
        running: bool,
        state: &str,
        track: Option<&Track>,
        index: usize,
    ) -> Result<()> {
        self.update(Projection::from_snapshot(
            generation, running, state, track, index,
        ))
    }
    pub fn invalidate(&self) -> Result<()> {
        self.update(Projection::Unavailable)
    }
    fn update(&self, snapshot: Projection) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| UNAVAILABLE)?;
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        if inner.snapshot != snapshot {
            inner.snapshot = snapshot;
            inner.revision = inner.revision.wrapping_add(1);
            self.changed.notify_one();
        }
        Ok(())
    }
    fn ready(&self, ready: bool) {
        if !ready {
            self.ready.store(false, Ordering::Release);
        }
        // Serialize the availability fence with callback admission. Teardown
        // clears pending commands before native cleanup can block.
        if let Ok(mut inner) = self.inner.lock() {
            self.ready.store(
                ready && !self.stopping.load(Ordering::Acquire),
                Ordering::Release,
            );
            if !ready {
                inner.commands.clear();
            }
        }
    }
    fn enqueue(&self, button: Button, admit: impl FnOnce() -> String) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| UNAVAILABLE)?;
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        if !self.ready.load(Ordering::Acquire) || !inner.snapshot.has_track() {
            return Err(UNAVAILABLE);
        }
        if inner.commands.len() >= LIMIT {
            return Err("engine_busy");
        }
        // The only operation inside this short critical section is atomic
        // intent admission. No MPD, WinRT, process wait or I/O is performed.
        inner.commands.push_back(Request {
            button,
            token: admit(),
        });
        self.changed.notify_one();
        Ok(())
    }
    fn work(&self, seen: u64) -> Result<Option<Work>> {
        let mut inner = self.inner.lock().map_err(|_| UNAVAILABLE)?;
        while !self.stopping.load(Ordering::Acquire)
            && inner.revision == seen
            && inner.commands.is_empty()
        {
            inner = self.changed.wait(inner).map_err(|_| UNAVAILABLE)?;
        }
        if self.stopping.load(Ordering::Acquire) {
            return Ok(None);
        }
        Ok(Some(Work {
            revision: inner.revision,
            snapshot: inner.snapshot.clone(),
            request: inner.commands.pop_front(),
        }))
    }
    fn stop(&self) {
        // Hold the same short gate used for admission/wait registration, so a
        // close cannot lose its wakeup. The atomic also fences callbacks even
        // if a poisoned lock or a blocked native call prevents normal cleanup.
        self.stopping.store(true, Ordering::Release);
        self.ready(false);
        if let Ok(mut inner) = self.inner.lock() {
            inner.commands.clear();
            inner.snapshot = Projection::Unavailable;
        }
        self.changed.notify_all();
    }
}
trait Transport {
    fn current(&self, token: &str) -> bool;
    fn generation(&self) -> Result<u64>;
    fn apply(&self, generation: u64, target: Target, token: &str) -> Result<()>;
}
fn execute(transport: &impl Transport, request: &Request) -> Result<()> {
    if !transport.current(&request.token) {
        return Err("PLAYBACK_SUPERSEDED");
    }
    let generation = transport.generation()?;
    if !transport.current(&request.token) {
        return Err("PLAYBACK_SUPERSEDED");
    }
    transport.apply(generation, request.button.target(), &request.token)
}

#[cfg(windows)]
#[path = "windows_system_media_native.rs"]
mod native;
#[cfg(windows)]
pub use native::Controller;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        sync::{atomic::AtomicU64, Arc},
        time::Duration,
    };
    fn track() -> Track {
        serde_json::from_value(serde_json::json!({"id":"t","title":"Title","artist":"Artist","album":"Album","disc":1,"number":1,"fileName":"one.flac"})).unwrap()
    }
    fn active() -> Arc<View> {
        let v = Arc::new(View::default());
        v.publish(1, true, "play", Some(&track()), 0).unwrap();
        v.ready(true);
        v
    }
    #[test]
    fn restore_and_publication_never_admit_or_execute_a_command() {
        let v = View::default();
        v.publish(7, false, "idle", Some(&track()), 2).unwrap();
        let work = v.work(u64::MAX).unwrap().unwrap();
        assert!(work.request.is_none());
        assert!(matches!(
            work.snapshot,
            Projection::Track {
                playback: Playback::Paused,
                generation: 7,
                index: 2,
                ..
            }
        ));
        assert_eq!(
            v.enqueue(Button::Play, || panic!("native service not initialized")),
            Err(UNAVAILABLE)
        );
        v.ready(true);
        let admissions = RefCell::new(0);
        v.enqueue(Button::Play, || {
            *admissions.borrow_mut() += 1;
            "1".into()
        })
        .unwrap();
        assert_eq!(*admissions.borrow(), 1);
        v.stop();
    }
    #[test]
    fn confirmed_states_are_distinct_and_invalid_or_empty_is_unavailable() {
        for (running, state, expected) in [
            (true, "play", Playback::Playing),
            (true, "pause", Playback::Paused),
            (true, "stop", Playback::Stopped),
            (false, "stop", Playback::Paused),
        ] {
            assert!(
                matches!(Projection::from_snapshot(1,running,state,Some(&track()),0),Projection::Track{playback,..} if playback==expected)
            );
        }
        assert_eq!(
            Projection::from_snapshot(1, true, "nonsense", Some(&track()), 0),
            Projection::Unavailable
        );
        assert_eq!(
            Projection::from_snapshot(1, true, "play", None, 0),
            Projection::Unavailable
        );
    }
    #[test]
    fn latest_snapshot_coalesces_without_a_message_backlog_or_extra_poll() {
        let v = active();
        for index in 0..10000 {
            v.publish(2, true, "play", Some(&track()), index).unwrap();
        }
        let work = v.work(u64::MAX).unwrap().unwrap();
        assert!(work.request.is_none());
        assert!(matches!(
            work.snapshot,
            Projection::Track { index: 9999, .. }
        ));
        v.stop();
    }
    #[test]
    fn queue_saturation_refuses_before_intent_admission_and_retains_fifo() {
        let v = active();
        for i in 0..LIMIT {
            v.enqueue(Button::Next, || i.to_string()).unwrap();
        }
        assert_eq!(
            v.enqueue(Button::Pause, || panic!("busy must not admit")),
            Err("engine_busy")
        );
        for i in 0..LIMIT {
            assert_eq!(
                v.work(u64::MAX).unwrap().unwrap().request.unwrap().token,
                i.to_string()
            );
        }
        v.enqueue(Button::Pause, || "new".into()).unwrap();
        v.stop();
    }
    #[test]
    fn invalidation_refuses_callbacks_without_turning_failure_into_stopped_or_empty_success() {
        let v = active();
        v.invalidate().unwrap();
        assert_eq!(
            v.enqueue(Button::Play, || panic!("unavailable")),
            Err(UNAVAILABLE)
        );
        assert_eq!(
            v.work(u64::MAX).unwrap().unwrap().snapshot,
            Projection::Unavailable
        );
        v.stop();
    }
    #[test]
    fn close_wakes_owner_discards_queued_actions_and_seals_late_callbacks() {
        let v = active();
        let revision = v.work(u64::MAX).unwrap().unwrap().revision;
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = v.clone();
        let owner = std::thread::spawn(move || {
            tx.send(worker.work(revision).unwrap().is_none()).unwrap();
        });
        v.stop();
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
        owner.join().unwrap();
        assert_eq!(
            v.enqueue(Button::Play, || panic!("late callback")),
            Err("shutting_down")
        );
        assert_eq!(
            v.publish(2, true, "play", Some(&track()), 0),
            Err("shutting_down")
        );
    }
    #[test]
    fn metadata_is_bounded_no_paths_or_urls_are_carried_by_projection() {
        let mut t = track();
        t.title = "é".repeat(600) + "\0\n";
        t.path = "private-path-not-for-os".into();
        let p = Projection::from_snapshot(1, true, "play", Some(&t), 0);
        if let Projection::Track { title, .. } = &p {
            assert_eq!(title.chars().count(), 512);
            assert!(!title.chars().any(char::is_control));
        } else {
            panic!()
        }
        assert!(!format!("{p:?}").contains("private-path"));
    }
    #[test]
    fn readiness_drop_seals_callbacks_before_native_resource_cleanup_can_block() {
        let v = active();
        let lease = ReadyOnDrop(v.clone());
        v.enqueue(Button::Play, || "1".into()).unwrap();
        drop(lease);
        assert_eq!(
            v.enqueue(Button::Next, || panic!("callback after availability fence")),
            Err(UNAVAILABLE)
        );
        assert!(v.inner.lock().unwrap().commands.is_empty());
        v.stop();
    }
    #[test]
    fn late_readiness_cannot_reopen_a_stopped_service() {
        let v = active();
        v.stop();
        v.ready(true);
        assert!(!v.ready.load(Ordering::Acquire));
        assert_eq!(
            v.enqueue(Button::Play, || panic!("reopened")),
            Err("shutting_down")
        );
    }
    #[test]
    fn producer_invalidation_on_drop_is_visible_before_more_callbacks() {
        let v = active();
        drop(ClearOnDrop(v.clone()));
        assert_eq!(
            v.enqueue(Button::Play, || panic!("producer already gone")),
            Err(UNAVAILABLE)
        );
        v.stop();
    }
    #[test]
    fn metadata_change_and_duplicate_occurrence_get_new_revisions_without_mutating_transport() {
        let v = active();
        let before = v.work(u64::MAX).unwrap().unwrap();
        v.publish(1, true, "play", Some(&track()), 0).unwrap();
        assert_eq!(v.inner.lock().unwrap().revision, before.revision);
        v.publish(1, true, "play", Some(&track()), 1).unwrap();
        let duplicate = v.work(before.revision).unwrap().unwrap();
        assert_ne!(duplicate.revision, before.revision);
        assert!(duplicate.request.is_none());
        let mut t = track();
        t.title = "New tag".into();
        v.publish(1, true, "play", Some(&t), 1).unwrap();
        let changed = v.work(duplicate.revision).unwrap().unwrap();
        assert!(matches!(changed.snapshot,Projection::Track{title,..} if title=="New tag"));
        assert!(changed.request.is_none());
        v.stop();
    }
    #[test]
    fn blocked_consumer_does_not_hold_publication_or_close_gate() {
        let v = active();
        v.enqueue(Button::Play, || "1".into()).unwrap();
        let (captured_tx, captured_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = v.clone();
        let consumer = std::thread::spawn(move || {
            let _owned_work = worker.work(u64::MAX).unwrap().unwrap();
            captured_tx.send(()).unwrap();
            // Stand-in for blocked MPD/WinRT after taking owned work. No actual
            // OS/media call, and no view mutex can survive this handoff.
            release_rx.recv().unwrap();
        });
        captured_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let publisher = v.clone();
        let producer = std::thread::spawn(move || {
            publisher
                .publish(2, true, "pause", Some(&track()), 1)
                .unwrap();
            publisher.stop();
            closed_tx.send(()).unwrap();
        });
        let completed = closed_rx.recv_timeout(Duration::from_secs(1));
        release_tx.send(()).unwrap();
        consumer.join().unwrap();
        producer.join().unwrap();
        assert!(
            completed.is_ok(),
            "publication/close waited for the consumer"
        );
        assert_eq!(
            v.enqueue(Button::Next, || panic!("closed")),
            Err("shutting_down")
        );
    }
    #[test]
    fn competing_callbacks_are_bounded_and_readiness_fence_closes_admission() {
        use std::sync::Barrier;
        let v = active();
        let barrier = Arc::new(Barrier::new(33));
        let admissions = Arc::new(AtomicU64::new(0));
        let mut callbacks = Vec::new();
        for _ in 0..32 {
            let (v, barrier, admissions) = (v.clone(), barrier.clone(), admissions.clone());
            callbacks.push(std::thread::spawn(move || {
                barrier.wait();
                v.enqueue(Button::Next, || {
                    admissions.fetch_add(1, Ordering::AcqRel).to_string()
                })
            }));
        }
        barrier.wait();
        for callback in callbacks {
            let result = callback.join().unwrap();
            assert!(result.is_ok() || result == Err("engine_busy"));
        }
        assert_eq!(admissions.load(Ordering::Acquire), LIMIT as u64);
        v.ready(false);
        assert_eq!(
            v.enqueue(Button::Play, || panic!("fenced callback")),
            Err(UNAVAILABLE)
        );
        assert!(v.inner.lock().unwrap().commands.is_empty());
        v.stop();
    }

    struct Fake {
        current: AtomicU64,
        applied: RefCell<Vec<Target>>,
        during_query: bool,
        fail: Option<&'static str>,
    }
    impl Transport for Fake {
        fn current(&self, token: &str) -> bool {
            token.parse::<u64>().ok() == Some(self.current.load(Ordering::Acquire))
        }
        fn generation(&self) -> Result<u64> {
            if self.during_query {
                self.current.fetch_add(1, Ordering::AcqRel);
            }
            if self.fail == Some("query_failed") {
                Err("query_failed")
            } else {
                Ok(42)
            }
        }
        fn apply(&self, generation: u64, target: Target, token: &str) -> Result<()> {
            assert_eq!(generation, 42);
            assert!(self.current(token));
            if let Some(error) = self.fail {
                return Err(error);
            }
            self.applied.borrow_mut().push(target);
            Ok(())
        }
    }
    fn fake() -> Fake {
        Fake {
            current: AtomicU64::new(2),
            applied: RefCell::new(vec![]),
            during_query: false,
            fail: None,
        }
    }
    #[test]
    fn event_time_intent_prevents_delayed_os_action_overwriting_newer_gui_action() {
        let f = fake();
        let old = Request {
            button: Button::Play,
            token: "1".into(),
        };
        assert_eq!(execute(&f, &old), Err("PLAYBACK_SUPERSEDED"));
        assert!(f.applied.borrow().is_empty());
    }
    #[test]
    fn intent_is_rechecked_after_query_and_before_dispatch() {
        let mut f = fake();
        f.during_query = true;
        assert_eq!(
            execute(
                &f,
                &Request {
                    button: Button::Next,
                    token: "2".into()
                }
            ),
            Err("PLAYBACK_SUPERSEDED")
        );
        assert!(f.applied.borrow().is_empty());
    }
    #[test]
    fn four_basic_commands_are_idempotent_targets_not_blind_toggle_and_failures_are_not_success() {
        let f = fake();
        for button in [Button::Play, Button::Pause, Button::Next, Button::Previous] {
            execute(
                &f,
                &Request {
                    button,
                    token: "2".into(),
                },
            )
            .unwrap();
        }
        assert_eq!(
            *f.applied.borrow(),
            vec![
                Target::Playing(true),
                Target::Playing(false),
                Target::Next,
                Target::Previous
            ]
        );
        for error in [
            "query_failed",
            "engine_busy",
            "mpd_disconnected",
            "stale_request",
        ] {
            let mut f = fake();
            f.fail = Some(error);
            assert_eq!(
                execute(
                    &f,
                    &Request {
                        button: Button::Play,
                        token: "2".into()
                    }
                ),
                Err(error)
            );
            assert!(f.applied.borrow().is_empty());
        }
    }
}
