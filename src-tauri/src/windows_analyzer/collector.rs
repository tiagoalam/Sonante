//! One lazy observer. Neither the actor nor an IPC thread polls the consumer.
//! The physical request stays on this sole worker until its bounded return.
use crate::windows_player::Owner;
use crate::windows_shared::analyzer::{Action, Binding, Frame, Receive, Sample, View};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, Instant};
type Check = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) struct Target {
    pub owner: Owner,
    pub current: Check,
    pub status: Arc<dyn Fn(bool) + Send + Sync>,
    pub frame: Arc<dyn Fn(Frame) + Send + Sync>,
}
#[derive(Default)]
struct State {
    desired: Option<Arc<Target>>,
    stopping: bool,
    running: bool,
}
struct Shared {
    view: Arc<View>,
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Clone)]
pub(crate) struct Collector(Arc<Shared>);
struct Active {
    target: Arc<Target>,
    source: Arc<Binding>,
    epoch: u64,
    token: String,
    receive: Receive,
    started: bool,
    status: Option<bool>,
    retry: Instant,
}
impl Active {
    fn current(&self, shared: &Shared) -> bool {
        (self.target.current)()
            && self.source.current()
            && shared.state.lock().ok().is_some_and(|s| {
                !s.stopping
                    && s.desired
                        .as_ref()
                        .is_some_and(|t| Arc::ptr_eq(t, &self.target))
            })
    }
    fn status(&mut self, shared: &Shared, available: bool) {
        if self.status != Some(available) && self.current(shared) {
            (self.target.status)(available);
            self.status = Some(available);
        }
    }
    fn end(self) {
        self.source.end(self.epoch, &self.token);
    }
}
impl Collector {
    pub(crate) fn new(view: Arc<View>) -> Self {
        let this = Self(Arc::new(Shared {
            view: view.clone(),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }));
        let weak = Arc::downgrade(&this.0);
        view.listen(Arc::new(move || {
            if let Some(shared) = weak.upgrade() {
                Self(shared).wake();
            }
        }));
        this
    }
    pub(crate) fn start(&self, target: Target) -> bool {
        {
            let Ok(mut state) = self.0.state.lock() else {
                return false;
            };
            if state.stopping || !(target.current)() {
                return false;
            }
            state.desired = Some(Arc::new(target));
        }
        self.wake();
        true
    }
    fn remove(&self, remove: impl FnOnce(&Owner) -> bool) {
        if let Ok(mut state) = self.0.state.lock() {
            if state.desired.as_ref().is_some_and(|t| remove(&t.owner)) {
                state.desired = None;
            }
        }
        self.0.changed.notify_all();
    }
    pub(crate) fn claimed(&self, generation: u64) {
        self.remove(|o| o.generation < generation);
    }
    pub(crate) fn stopped(&self, host: &str, session: &str, generation: u64, lease: &str) {
        self.remove(|o| {
            o.host == host
                && o.session_id == session
                && o.generation == generation
                && o.lease_id.as_deref() == Some(lease)
        });
    }
    pub(crate) fn released(&self, session: &str, generation: u64) {
        self.remove(|o| o.session_id == session && o.generation == generation);
    }
    pub(crate) fn closed(&self, host: &str) {
        self.remove(|o| o.host == host);
    }
    pub(crate) fn shutdown(&self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.stopping = true;
            state.desired = None;
        }
        self.0.changed.notify_all();
    }
    fn wake(&self) {
        self.0.changed.notify_all();
        let Ok(mut state) = self.0.state.lock() else {
            return;
        };
        // No worker, socket, audio source or listener at startup/no real feed.
        if state.stopping
            || state.running
            || state.desired.is_none()
            || self.0.view.current().is_none()
        {
            return;
        }
        state.running = true;
        let shared = self.0.clone();
        if std::thread::Builder::new()
            .name("sonante-analyzer-events".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.run()));
                let failed = if let Ok(mut state) = shared.state.lock() {
                    state.running = false;
                    if result.is_err() && !state.stopping {
                        state.desired.clone()
                    } else {
                        None
                    }
                } else {
                    None
                };
                // A failed optional observer must revoke its presentation
                // capability, never stop or poison the accepted audio route.
                if let Some(target) = failed {
                    if (target.current)() {
                        (target.status)(false);
                    }
                }
            })
            .is_err()
        {
            state.running = false;
        }
    }
}
impl Shared {
    fn wait(&self, delay: Duration) {
        if let Ok(state) = self.state.lock() {
            if !state.stopping {
                drop(self.changed.wait_timeout(state, delay));
            }
        }
    }
    fn run(&self) {
        static EPOCH: AtomicU64 = AtomicU64::new(1);
        let mut active: Option<Active> = None;
        let mut unavailable: Option<Arc<Target>> = None;
        loop {
            let desired = {
                let Ok(mut state) = self.state.lock() else {
                    break;
                };
                while !state.stopping && state.desired.is_none() {
                    unavailable = None;
                    if let Some(old) = active.take() {
                        drop(state);
                        old.end();
                        state = match self.state.lock() {
                            Ok(s) => s,
                            Err(_) => return,
                        };
                        continue;
                    }
                    state = match self.changed.wait(state) {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                }
                if state.stopping {
                    None
                } else {
                    state.desired.clone()
                }
            };
            let Some(target) = desired else {
                break;
            };
            let source = if (target.current)() {
                self.view.current()
            } else {
                None
            };
            let same = active.as_ref().is_some_and(|old| {
                Arc::ptr_eq(&old.target, &target)
                    && source.as_ref().is_some_and(|b| Arc::ptr_eq(&old.source, b))
            });
            if !same {
                if let Some(old) = active.take() {
                    old.end();
                }
                if let Some(source) = source {
                    unavailable = None;
                    let Ok(epoch) = EPOCH
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
                    else {
                        return;
                    };
                    // Epoch + opaque presentation lease; no path, credential or media identity.
                    let token = format!("win-observation-{epoch}");
                    active = Some(Active {
                        target: target.clone(),
                        source,
                        epoch,
                        token,
                        receive: Receive::default(),
                        started: false,
                        status: None,
                        retry: Instant::now(),
                    });
                } else {
                    if unavailable
                        .as_ref()
                        .is_none_or(|old| !Arc::ptr_eq(old, &target))
                        && (target.current)()
                    {
                        (target.status)(false);
                        unavailable = Some(target.clone());
                    }
                    self.wait(Duration::from_millis(100));
                    continue;
                }
            }
            let Some(observation) = active.as_mut() else {
                continue;
            };
            if !observation.current(self) {
                self.wait(Duration::from_millis(66));
                continue;
            }
            if Instant::now() < observation.retry {
                self.wait(Duration::from_millis(66));
                continue;
            }
            let began = Instant::now();
            let action = if observation.started {
                Action::Read
            } else {
                Action::Start
            };
            let reply = observation
                .source
                .query(action, observation.epoch, &observation.token)
                .and_then(|wire| {
                    observation.receive.accept(
                        wire,
                        observation.epoch,
                        &observation.token,
                        began.elapsed(),
                    )
                });
            if !observation.current(self) {
                continue;
            }
            match reply {
                Ok(Sample::Fresh(frame)) => {
                    observation.started = true;
                    observation.status(self, true);
                    if observation.current(self) {
                        (observation.target.frame)(frame);
                    }
                }
                Ok(Sample::Duplicate) => {
                    observation.started = true;
                }
                Ok(Sample::Unavailable) => {
                    observation.started = true;
                    observation.status(self, false);
                }
                Err(_) => {
                    observation.started = false;
                    observation.retry = Instant::now() + Duration::from_secs(1);
                    observation.status(self, false);
                }
            }
            // No catch-up burst and no task queue when control is slow or unavailable.
            self.wait(Duration::from_millis(66));
        }
        if let Some(old) = active {
            old.end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows_shared::analyzer::{Check, Reader};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    struct Failed;
    impl Reader for Failed {
        fn identity(&self) -> [u64; 3] {
            [9, 8, 7]
        }
        fn current(&self) -> bool {
            true
        }
        fn query(&self, _: Action, _: u64, _: &str) -> Result<Value, &'static str> {
            panic!("own synthetic observer failure")
        }
    }
    #[test]
    fn observer_failure_revokes_availability_without_touching_audio_or_leases() {
        let view = Arc::new(View::default());
        view.publish(1, 0, Some(Arc::new(Failed)), Arc::new(|| true));
        let collector = Collector::new(view);
        let live = Arc::new(AtomicBool::new(true));
        let (t, events) = target(owner(1, "own"), live.clone());
        assert!(collector.start(t));
        wait(|| events.status.lock().unwrap().last() == Some(&false));
        wait(|| !collector.0.state.lock().unwrap().running);
        assert!(events.frames.lock().unwrap().is_empty());
        assert!(live.load(Ordering::Acquire));
        assert!(collector.0.state.lock().unwrap().desired.is_some());
        collector.shutdown();
    }
    struct Fixture {
        queries: AtomicUsize,
        physical: AtomicUsize,
        maximum: AtomicUsize,
        sequence: AtomicU64,
        live: AtomicBool,
        hold: Mutex<bool>,
        release: Condvar,
        stale: AtomicBool,
    }
    impl Fixture {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                queries: AtomicUsize::new(0),
                physical: AtomicUsize::new(0),
                maximum: AtomicUsize::new(0),
                sequence: AtomicU64::new(1),
                live: AtomicBool::new(true),
                hold: Mutex::new(false),
                release: Condvar::new(),
                stale: AtomicBool::new(false),
            })
        }
    }
    impl Reader for Fixture {
        fn identity(&self) -> [u64; 3] {
            [1, 2, 3]
        }
        fn current(&self) -> bool {
            self.live.load(Ordering::Acquire)
        }
        fn query(&self, action: Action, epoch: u64, token: &str) -> Result<Value, &'static str> {
            self.queries.fetch_add(1, Ordering::AcqRel);
            let count = self.physical.fetch_add(1, Ordering::AcqRel) + 1;
            self.maximum.fetch_max(count, Ordering::AcqRel);
            let mut held = self.hold.lock().unwrap();
            while *held {
                held = self.release.wait(held).unwrap();
            }
            drop(held);
            let stop = matches!(action, Action::Stop);
            let frame=(!stop).then(||json!({"leftRms":0.5,"rightRms":0.4,"leftPeak":0.8,"rightPeak":0.7,"spectrum":vec![0.5;48]}));
            let reply = json!({"SonanteAnalyzer":{"result":"Ok","value":{"version":1,"available":!stop,"epoch":epoch,"token":token,"sequence":if stop{0}else{self.sequence.load(Ordering::Acquire)},"age_ms":if self.stale.load(Ordering::Acquire){151}else{0},"frame":frame}}});
            self.physical.fetch_sub(1, Ordering::AcqRel);
            Ok(reply)
        }
    }
    fn wait(check: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(3);
        while !check() {
            assert!(
                Instant::now() < end,
                "collector did not reach expected state"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    struct Events {
        frames: Mutex<Vec<Frame>>,
        status: Mutex<Vec<bool>>,
    }
    fn target(owner: Owner, live: Arc<AtomicBool>) -> (Target, Arc<Events>) {
        let events = Arc::new(Events {
            frames: Mutex::new(Vec::new()),
            status: Mutex::new(Vec::new()),
        });
        let frame = events.clone();
        let status = events.clone();
        (
            Target {
                owner,
                current: Arc::new(move || live.load(Ordering::Acquire)),
                status: Arc::new(move |v| status.status.lock().unwrap().push(v)),
                frame: Arc::new(move |f| frame.frames.lock().unwrap().push(f)),
            },
            events,
        )
    }
    fn owner(generation: u64, lease: &str) -> Owner {
        Owner {
            host: "main".into(),
            session_id: "fixture".into(),
            generation,
            lease_id: Some(lease.into()),
        }
    }
    fn publish(view: &View, reader: Arc<Fixture>, generation: u64) {
        let scope: Check = Arc::new(|| true);
        view.publish(generation, 0, Some(reader), scope);
    }
    #[test]
    fn no_source_starts_no_worker_and_only_fresh_real_frames_enable_the_original_events() {
        let view = Arc::new(View::default());
        let collector = Collector::new(view.clone());
        let live = Arc::new(AtomicBool::new(true));
        let (t, events) = target(owner(1, "own"), live);
        assert!(collector.start(t));
        assert!(!collector.0.state.lock().unwrap().running);
        let fixture = Fixture::new();
        publish(&view, fixture.clone(), 1);
        wait(|| events.frames.lock().unwrap().len() == 1);
        std::thread::sleep(Duration::from_millis(180));
        assert_eq!(
            events.frames.lock().unwrap().len(),
            1,
            "duplicate is not a new frame"
        );
        fixture.sequence.store(2, Ordering::Release);
        wait(|| events.frames.lock().unwrap().len() == 2);
        fixture.stale.store(true, Ordering::Release);
        wait(|| events.status.lock().unwrap().last() == Some(&false));
        assert_eq!(fixture.maximum.load(Ordering::Acquire), 1);
        collector.shutdown();
        wait(|| !collector.0.state.lock().unwrap().running);
    }
    #[test]
    fn a_late_physical_reply_keeps_capacity_and_cannot_reach_a_new_lease_or_shutdown() {
        let view = Arc::new(View::default());
        let collector = Collector::new(view.clone());
        let fixture = Fixture::new();
        *fixture.hold.lock().unwrap() = true;
        publish(&view, fixture.clone(), 1);
        let old_live = Arc::new(AtomicBool::new(true));
        let (t, old) = target(owner(1, "old"), old_live.clone());
        assert!(collector.start(t));
        wait(|| fixture.physical.load(Ordering::Acquire) == 1);
        old_live.store(false, Ordering::Release);
        let new_live = Arc::new(AtomicBool::new(true));
        let (t, new) = target(owner(2, "new"), new_live);
        assert!(collector.start(t));
        collector.stopped("main", "fixture", 1, "old");
        assert_eq!(fixture.maximum.load(Ordering::Acquire), 1);
        assert!(new.frames.lock().unwrap().is_empty());
        *fixture.hold.lock().unwrap() = false;
        fixture.release.notify_all();
        wait(|| new.frames.lock().unwrap().len() == 1);
        assert!(old.frames.lock().unwrap().is_empty());
        collector.shutdown();
        wait(|| !collector.0.state.lock().unwrap().running);
        let frames = new.frames.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(new.frames.lock().unwrap().len(), frames);
        assert_eq!(fixture.maximum.load(Ordering::Acquire), 1);
    }
    #[test]
    fn pause_and_mode_source_invalidation_do_not_reuse_old_samples_and_old_close_does_not_cancel_handoff(
    ) {
        let view = Arc::new(View::default());
        let collector = Collector::new(view.clone());
        let fixture = Fixture::new();
        publish(&view, fixture.clone(), 1);
        let (t, events) = target(owner(2, "own"), Arc::new(AtomicBool::new(true)));
        assert!(collector.start(t));
        wait(|| events.frames.lock().unwrap().len() == 1);
        view.clear();
        wait(|| events.status.lock().unwrap().last() == Some(&false));
        fixture.sequence.store(3, Ordering::Release);
        let n = events.frames.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(160));
        assert_eq!(events.frames.lock().unwrap().len(), n);
        collector.claimed(1);
        collector.released("old-session", 1);
        collector.closed("now-playing");
        publish(&view, fixture, 2);
        wait(|| events.frames.lock().unwrap().len() > n);
        collector.shutdown();
        wait(|| !collector.0.state.lock().unwrap().running);
    }
}
