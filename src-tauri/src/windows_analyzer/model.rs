//! Pure source/epoch and metric validation for the original Now Playing events.
//! No actor, filesystem, audio reader, TCP address or playback mutation here.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
pub(crate) type Check = Arc<dyn Fn() -> bool + Send + Sync>;
#[path = "linux_policy.rs"]
mod linux_policy;
pub(crate) fn source_allowed(audio: &str, file: Option<&str>) -> bool {
    let status = [format!("audio: {audio}")];
    let song: Vec<String> = file
        .map(|value| format!("file: {value}"))
        .into_iter()
        .collect();
    linux_policy::dsd_or_dop_reason(&status, &song).is_none()
}
pub(crate) fn eligible(
    full: bool,
    running: bool,
    state: &str,
    verified: bool,
    in_path: Option<bool>,
) -> bool {
    full && running && state == "play" && verified && in_path == Some(true)
}
#[derive(Clone, Copy)]
pub(crate) enum Action {
    Start,
    Read,
    Stop,
}
impl Action {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Read => "read",
            Self::Stop => "stop",
        }
    }
}
/// Only native source binding creates this reader. `current` must be pure and
/// nonblocking; query belongs to the single owned observation worker.
pub(crate) trait Reader: Send + Sync {
    fn identity(&self) -> [u64; 3];
    fn current(&self) -> bool;
    fn query(&self, action: Action, epoch: u64, token: &str) -> Result<Value, &'static str>;
}
pub(crate) struct Binding {
    pub generation: u64,
    revision: u64,
    identity: [u64; 3],
    active: AtomicBool,
    reader: Arc<dyn Reader>,
    scope: Check,
}
impl Binding {
    pub(crate) fn end(&self, epoch: u64, token: &str) {
        if self.reader.current() {
            let _ = self.reader.query(Action::Stop, epoch, token);
        }
    }
    pub(crate) fn current(&self) -> bool {
        self.active.load(Ordering::Acquire) && (self.scope)() && self.reader.current()
    }
    pub(crate) fn query(
        &self,
        action: Action,
        epoch: u64,
        token: &str,
    ) -> Result<Value, &'static str> {
        if !self.current() {
            return Err("WIN_ANALYZER_SOURCE_CHANGED");
        }
        let reply = self.reader.query(action, epoch, token)?;
        if !self.current() {
            return Err("WIN_ANALYZER_SOURCE_CHANGED");
        }
        Ok(reply)
    }
}
#[derive(Default)]
pub(crate) struct View {
    selected: Mutex<Option<Arc<Binding>>>,
    changed: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
impl View {
    pub(crate) fn listen(&self, changed: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.changed.lock() {
            *slot = Some(changed);
        }
    }
    pub(crate) fn publish(
        &self,
        generation: u64,
        revision: u64,
        reader: Option<Arc<dyn Reader>>,
        scope: Check,
    ) {
        let Ok(mut selected) = self.selected.lock() else {
            return;
        };
        let next = reader.filter(|r| (scope)() && r.current());
        if let (Some(old), Some(new)) = (selected.as_ref(), next.as_ref()) {
            if old.generation == generation
                && old.revision == revision
                && old.identity == new.identity()
                && old.current()
            {
                return;
            }
        }
        if selected.is_none() && next.is_none() {
            return;
        }
        if let Some(old) = selected.take() {
            old.active.store(false, Ordering::Release);
        }
        *selected = next.map(|reader| {
            Arc::new(Binding {
                generation,
                revision,
                identity: reader.identity(),
                active: AtomicBool::new(true),
                reader,
                scope,
            })
        });
        drop(selected);
        let notify = self.changed.lock().ok().and_then(|n| n.clone());
        if let Some(notify) = notify {
            notify();
        }
    }
    pub(crate) fn current(&self) -> Option<Arc<Binding>> {
        self.selected
            .try_lock()
            .ok()?
            .as_ref()
            .filter(|b| b.current())
            .cloned()
    }
    pub(crate) fn clear(&self) {
        self.publish(0, 0, None, Arc::new(|| false));
    }
}
pub(crate) struct ClearOnDrop(pub Arc<View>);
impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        self.0.clear();
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Frame {
    pub left_rms: f32,
    pub right_rms: f32,
    pub left_peak: f32,
    pub right_peak: f32,
    pub spectrum: Vec<f32>,
}
impl Frame {
    fn valid(&self) -> bool {
        self.spectrum.len() == 48
            && [
                self.left_rms,
                self.right_rms,
                self.left_peak,
                self.right_peak,
            ]
            .into_iter()
            .chain(self.spectrum.iter().copied())
            .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    #[serde(rename = "SonanteAnalyzer")]
    packet: Packet,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Packet {
    result: Value,
    value: Option<Reply>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    version: u8,
    available: bool,
    epoch: u64,
    token: String,
    sequence: u64,
    age_ms: u64,
    frame: Option<Frame>,
}
#[derive(Debug, PartialEq)]
pub(crate) enum Sample {
    Fresh(Frame),
    Duplicate,
    Unavailable,
}
#[derive(Default)]
pub(crate) struct Receive {
    sequence: u64,
}
impl Receive {
    pub(crate) fn accept(
        &mut self,
        raw: Value,
        epoch: u64,
        token: &str,
        roundtrip: Duration,
    ) -> Result<Sample, &'static str> {
        let envelope: Envelope =
            serde_json::from_value(raw).map_err(|_| "WIN_ANALYZER_REPLY_INVALID")?;
        if envelope.packet.result != serde_json::json!("Ok") {
            return Err("WIN_ANALYZER_CONTROL_UNAVAILABLE");
        }
        let reply = envelope.packet.value.ok_or("WIN_ANALYZER_REPLY_INVALID")?;
        if reply.version != 1 || reply.epoch != epoch || reply.token != token {
            return Err("WIN_ANALYZER_REPLY_STALE");
        }
        if !reply.available {
            if reply.frame.is_some() || reply.sequence != 0 {
                return Err("WIN_ANALYZER_REPLY_INVALID");
            }
            return Ok(Sample::Unavailable);
        }
        let frame = reply
            .frame
            .filter(Frame::valid)
            .ok_or("WIN_ANALYZER_REPLY_INVALID")?;
        if reply.sequence == 0 || reply.sequence < self.sequence {
            return Err("WIN_ANALYZER_REPLY_STALE");
        }
        let elapsed = roundtrip.as_millis().min(u64::MAX as u128) as u64;
        if reply
            .age_ms
            .checked_add(elapsed)
            .is_none_or(|age| age > 150)
        {
            return Ok(Sample::Unavailable);
        }
        if reply.sequence == self.sequence {
            return Ok(Sample::Duplicate);
        }
        self.sequence = reply.sequence;
        Ok(Sample::Fresh(frame))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    #[test]
    fn original_dsd_dop_and_file_policy_is_preserved_even_when_eq_supplies_pcm() {
        for audio in ["dsd64:2", "2822400:dsd:2", "DoP:2", "DSD128:2"] {
            assert!(!source_allowed(audio, Some("fixture.flac")));
        }
        for file in [
            "folder/fixture.DSF?token=OWN",
            "fixture.dff#OWN",
            "fixture.dsd",
        ] {
            assert!(!source_allowed("48000:32:2", Some(file)));
        }
        for file in [
            None,
            Some("fixture.flac"),
            Some("folder/dsd-title.flac"),
            Some("fixture.wav?name=.dsf"),
        ] {
            assert!(source_allowed("48000:24:2", file));
        }
    }
    #[test]
    fn only_full_verified_playing_eq_can_publish_a_source() {
        for full in [false, true] {
            for running in [false, true] {
                for state in ["play", "pause", "stop", "unknown"] {
                    for verified in [false, true] {
                        for path in [None, Some(false), Some(true)] {
                            assert_eq!(
                                eligible(full, running, state, verified, path),
                                full && running
                                    && state == "play"
                                    && verified
                                    && path == Some(true)
                            );
                        }
                    }
                }
            }
        }
    }
    struct Fixture {
        live: Arc<AtomicBool>,
    }
    impl Reader for Fixture {
        fn identity(&self) -> [u64; 3] {
            [1, 2, 3]
        }
        fn current(&self) -> bool {
            self.live.load(Ordering::Acquire)
        }
        fn query(&self, _: Action, epoch: u64, token: &str) -> Result<Value, &'static str> {
            Ok(reply(epoch, token, 1))
        }
    }
    fn reply(epoch: u64, token: &str, sequence: u64) -> Value {
        serde_json::json!({"SonanteAnalyzer":{"result":"Ok","value":{"version":1,"available":true,"epoch":epoch,"token":token,"sequence":sequence,"age_ms":0,"frame":{"leftRms":0.5,"rightRms":0.4,"leftPeak":0.8,"rightPeak":0.7,"spectrum":vec![0.5;48]}}}})
    }
    #[test]
    fn schema_matches_original_events_and_duplicate_or_stale_frames_are_not_pushed() {
        let mut receive = Receive::default();
        let first = reply(2, "own", 1);
        let Sample::Fresh(frame) = receive
            .accept(first.clone(), 2, "own", Duration::ZERO)
            .unwrap()
        else {
            panic!()
        };
        let wire = serde_json::to_value(frame).unwrap();
        assert!(wire.get("leftRms").is_some() && wire.get("left_rms").is_none());
        assert_eq!(
            receive.accept(first, 2, "own", Duration::ZERO).unwrap(),
            Sample::Duplicate
        );
        assert!(receive
            .accept(reply(2, "old", 2), 2, "own", Duration::ZERO)
            .is_err());
        assert!(receive
            .accept(reply(1, "own", 2), 2, "own", Duration::ZERO)
            .is_err());
        assert_eq!(
            receive
                .accept(reply(2, "own", 2), 2, "own", Duration::from_millis(151))
                .unwrap(),
            Sample::Unavailable
        );
    }
    #[test]
    fn malformed_or_non_normalized_metrics_never_become_an_audio_capability() {
        for field in ["leftRms", "rightRms", "leftPeak", "rightPeak"] {
            for value in [
                serde_json::json!(-0.1),
                serde_json::json!(1.1),
                serde_json::Value::Null,
            ] {
                let mut raw = reply(2, "own", 1);
                raw["SonanteAnalyzer"]["value"]["frame"][field] = value;
                assert!(Receive::default()
                    .accept(raw, 2, "own", Duration::ZERO)
                    .is_err());
            }
        }
        for bands in [0, 1, 47, 49, 1000] {
            let mut raw = reply(2, "own", 1);
            raw["SonanteAnalyzer"]["value"]["frame"]["spectrum"] =
                serde_json::json!(vec![0.5; bands]);
            assert!(Receive::default()
                .accept(raw, 2, "own", Duration::ZERO)
                .is_err());
        }
        let mut raw = reply(2, "own", 1);
        raw["SonanteAnalyzer"]["value"]["available"] = serde_json::json!(false);
        assert!(Receive::default()
            .accept(raw, 2, "own", Duration::ZERO)
            .is_err());
    }
    #[test]
    fn same_source_observation_is_stable_but_pause_config_or_shutdown_cannot_revive_old_binding() {
        let view = View::default();
        let live = Arc::new(AtomicBool::new(true));
        let scope = Arc::new(AtomicU64::new(0));
        let revision = scope.clone();
        let current: Check = Arc::new(move || revision.load(Ordering::Acquire) == 0);
        view.publish(
            1,
            0,
            Some(Arc::new(Fixture { live: live.clone() })),
            current.clone(),
        );
        let old = view.current().unwrap();
        view.publish(
            1,
            0,
            Some(Arc::new(Fixture { live: live.clone() })),
            current.clone(),
        );
        assert!(Arc::ptr_eq(&old, &view.current().unwrap()));
        scope.store(2, Ordering::Release);
        assert!(!old.current());
        view.clear();
        scope.store(0, Ordering::Release);
        assert!(!old.current());
        view.publish(
            1,
            0,
            Some(Arc::new(Fixture { live: live.clone() })),
            current,
        );
        let new = view.current().unwrap();
        assert!(!Arc::ptr_eq(&old, &new));
        live.store(false, Ordering::Release);
        assert!(view.current().is_none());
    }
    #[test]
    fn view_never_waits_for_publication_and_invalidated_query_cannot_publish() {
        let view = View::default();
        let _held = view.selected.lock().unwrap();
        let at = std::time::Instant::now();
        assert!(view.current().is_none());
        assert!(at.elapsed() < Duration::from_millis(20));
    }
}
