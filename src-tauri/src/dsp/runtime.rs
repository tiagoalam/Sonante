//! Session-bound CamillaDSP WebSocket control. No app command calls this yet.
use super::peq::{
    convert_preset, CamillaFilter, CamillaPipelineStep, PcmFormat, PeqConversionError,
};
use crate::equalizer::EqPreset;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::{client, Message};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(not(test))]
const VERIFY_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(test)]
const VERIFY_TIMEOUT: Duration = Duration::from_millis(120);
const VERIFY_INTERVAL: Duration = Duration::from_millis(20);
const MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;
// Configured volume_ramp_time is 100 ms; one extra 1024-frame chunk and margin.
const TRANSITION_SETTLE: Duration = Duration::from_millis(150);
const TRANSITION_ATTENUATION_DB: f64 = -60.0;

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    Conversion(PeqConversionError),
    RuntimeUnavailable,
    WebSocketConnectFailed,
    RequestTimeout,
    UnexpectedReply,
    UnexpectedConfig,
    ApplyRejected,
    VerificationFailed,
    RollbackFailed,
    SessionStale,
    ConcurrentApply,
    CamillaNotRunning,
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeEndpoint {
    pub port: u16,
    pub title: String,
    pub generation: u64,
    pub capture_name: String,
    pub playback_name: String,
}

impl RuntimeEndpoint {
    pub fn reserve(
        generation: u64,
        capture_name: String,
        playback_name: String,
    ) -> Result<Self, RuntimeError> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| RuntimeError::RuntimeUnavailable)?;
        let port = listener
            .local_addr()
            .map_err(|_| RuntimeError::RuntimeUnavailable)?
            .port();
        let mut secret = [0_u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut secret))
            .map_err(|_| RuntimeError::RuntimeUnavailable)?;
        let title = format!(
            "sonante_dsp_{generation}_{}",
            secret
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        drop(listener); // CamillaDSP must bind its own socket; title proves the endpoint after spawn.
        Ok(Self {
            port,
            title,
            generation,
            capture_name,
            playback_name,
        })
    }
}

#[derive(Debug, Clone)]
enum Command {
    GetState,
    GetConfigJson,
    GetVolume,
    SetVolume(f64),
    PatchConfig(Value),
}

pub trait RuntimeTransport: Send + Sync {
    fn request(&self, port: u16, command: Value) -> Result<Value, RuntimeError>;
}

pub struct LocalWebSocketTransport;

impl RuntimeTransport for LocalWebSocketTransport {
    fn request(&self, port: u16, command: Value) -> Result<Value, RuntimeError> {
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let stream = TcpStream::connect_timeout(&address.into(), CONNECT_TIMEOUT)
            .map_err(|_| RuntimeError::WebSocketConnectFailed)?;
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|_| RuntimeError::WebSocketConnectFailed)?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .map_err(|_| RuntimeError::WebSocketConnectFailed)?;
        let url = format!("ws://127.0.0.1:{port}/");
        let (mut socket, _) =
            client(url, stream).map_err(|_| RuntimeError::WebSocketConnectFailed)?;
        socket
            .send(Message::Text(command.to_string().into()))
            .map_err(|_| RuntimeError::RequestTimeout)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RuntimeError::RequestTimeout);
            }
            socket
                .get_mut()
                .set_read_timeout(Some(remaining))
                .map_err(|_| RuntimeError::RequestTimeout)?;
            match socket.read() {
                Ok(Message::Text(text)) => {
                    if text.len() > MAX_REPLY_BYTES {
                        return Err(RuntimeError::UnexpectedReply);
                    }
                    return serde_json::from_str(&text).map_err(|_| RuntimeError::UnexpectedReply);
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                Ok(_) => return Err(RuntimeError::UnexpectedReply),
                Err(_) => return Err(RuntimeError::RequestTimeout),
            }
        }
    }
}

pub struct CamillaRuntimeController<T: RuntimeTransport = LocalWebSocketTransport> {
    endpoint: RuntimeEndpoint,
    current_generation: Arc<AtomicU64>,
    shared: Arc<RuntimeShared>,
    transport: T,
}

#[derive(Default)]
pub(super) struct RuntimeShared {
    applying: AtomicBool,
    published_preset: Mutex<Option<String>>,
}

struct ApplyClaim<'a>(&'a AtomicBool);
impl Drop for ApplyClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl<T: RuntimeTransport> CamillaRuntimeController<T> {
    pub(super) fn new(
        endpoint: RuntimeEndpoint,
        current_generation: Arc<AtomicU64>,
        shared: Arc<RuntimeShared>,
        transport: T,
    ) -> Self {
        Self {
            endpoint,
            current_generation,
            shared,
            transport,
        }
    }

    fn current(&self, generation: u64) -> Result<(), RuntimeError> {
        if generation != self.endpoint.generation
            || self.current_generation.load(Ordering::SeqCst) != generation
        {
            Err(RuntimeError::SessionStale)
        } else {
            Ok(())
        }
    }

    fn request(&self, generation: u64, command: Command) -> Result<Value, RuntimeError> {
        self.current(generation)?;
        let (name, payload) = match command {
            Command::GetState => ("GetState", json!("GetState")),
            Command::GetConfigJson => ("GetConfigJson", json!("GetConfigJson")),
            Command::GetVolume => ("GetVolume", json!("GetVolume")),
            Command::SetVolume(db) => ("SetVolume", json!({"SetVolume":db})),
            Command::PatchConfig(patch) => ("PatchConfig", json!({"PatchConfig":patch})),
        };
        let reply = self.transport.request(self.endpoint.port, payload)?;
        self.current(generation)?;
        let value = reply.get(name).ok_or(RuntimeError::UnexpectedReply)?;
        if value.get("result") != Some(&json!("Ok")) {
            return Err(RuntimeError::ApplyRejected);
        }
        Ok(value.clone())
    }

    fn config(&self, generation: u64) -> Result<Value, RuntimeError> {
        let reply = self.request(generation, Command::GetConfigJson)?;
        let encoded = reply
            .get("value")
            .and_then(Value::as_str)
            .ok_or(RuntimeError::UnexpectedReply)?;
        let config: Value =
            serde_json::from_str(encoded).map_err(|_| RuntimeError::UnexpectedConfig)?;
        let devices = config
            .get("devices")
            .ok_or(RuntimeError::UnexpectedConfig)?;
        if config.get("title").and_then(Value::as_str) != Some(self.endpoint.title.as_str())
            || devices
                .pointer("/capture/node_name")
                .and_then(Value::as_str)
                != Some(self.endpoint.capture_name.as_str())
            || devices
                .pointer("/playback/node_name")
                .and_then(Value::as_str)
                != Some(self.endpoint.playback_name.as_str())
        {
            return Err(RuntimeError::UnexpectedConfig);
        }
        Ok(config)
    }

    pub(super) fn probe_owned(&self) -> Result<(), RuntimeError> {
        let generation = self.endpoint.generation;
        self.config(generation)?;
        // With no MPD stream yet, CamillaDSP may report Inactive despite owning its nodes.
        Ok(())
    }

    fn running(&self, generation: u64) -> Result<(), RuntimeError> {
        let reply = self.request(generation, Command::GetState)?;
        match reply.get("value").and_then(Value::as_str) {
            Some("Running" | "Paused" | "Stalled") => Ok(()),
            _ => Err(RuntimeError::CamillaNotRunning),
        }
    }

    pub fn published_preset(&self) -> Result<Option<String>, RuntimeError> {
        self.shared
            .published_preset
            .lock()
            .map(|value| value.clone())
            .map_err(|_| RuntimeError::RuntimeUnavailable)
    }

    fn volume_target(&self, generation: u64) -> Result<f64, RuntimeError> {
        self.request(generation, Command::GetVolume)?
            .get("value")
            .and_then(Value::as_f64)
            .ok_or(RuntimeError::UnexpectedReply)
    }

    fn set_transition_gain(&self, generation: u64, db: f64) -> Result<(), RuntimeError> {
        self.request(generation, Command::SetVolume(db))?;
        if (self.volume_target(generation)? - db).abs() > 0.001 {
            return Err(RuntimeError::VerificationFailed);
        }
        Ok(())
    }

    pub fn apply_preset(
        &self,
        generation: u64,
        preset: &EqPreset,
        format: PcmFormat,
    ) -> Result<(), RuntimeError> {
        let desired = convert_preset(preset, format).map_err(RuntimeError::Conversion)?;
        self.current(generation)?;
        self.shared
            .applying
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| RuntimeError::ConcurrentApply)?;
        let _claim = ApplyClaim(&self.shared.applying);
        self.running(generation)?;
        let previous = self.config(generation)?;
        if previous
            .pointer("/devices/samplerate")
            .and_then(Value::as_u64)
            != Some(u64::from(format.sample_rate_hz))
        {
            return Err(RuntimeError::UnexpectedConfig);
        }
        if previous
            .pointer("/devices/volume_ramp_time")
            .and_then(Value::as_f64)
            != Some(100.0)
            || self.volume_target(generation)?.abs() > 0.001
        {
            return Err(RuntimeError::UnexpectedConfig);
        }
        let desired_value =
            serde_json::to_value(&desired).map_err(|_| RuntimeError::UnexpectedConfig)?;
        let patch = dsp_patch(&previous, &desired_value)?;
        // Main fader is temporary transition gain. Preset preamp remains in filters.
        if let Err(error) = self.set_transition_gain(generation, TRANSITION_ATTENUATION_DB) {
            if error == RuntimeError::SessionStale {
                return Err(error);
            }
            self.set_transition_gain(generation, 0.0)
                .map_err(|_| RuntimeError::RollbackFailed)?;
            return Err(error);
        }
        std::thread::sleep(TRANSITION_SETTLE);
        let update = self
            .request(generation, Command::PatchConfig(patch))
            .and_then(|_| self.verify(generation, &desired_value, &previous))
            .and_then(|_| self.set_transition_gain(generation, 0.0));
        if let Err(error) = update {
            if error == RuntimeError::SessionStale {
                return Err(error);
            }
            let rollback = (|| {
                let observed = self.config(generation)?;
                let restore = dsp_patch(&observed, &previous)?;
                self.request(generation, Command::PatchConfig(restore))?;
                self.verify(generation, &previous, &previous)?;
                self.set_transition_gain(generation, 0.0)
            })();
            if rollback.is_err() {
                return Err(RuntimeError::RollbackFailed);
            }
            return Err(error);
        }
        self.current(generation)?;
        *self
            .shared
            .published_preset
            .lock()
            .map_err(|_| RuntimeError::RuntimeUnavailable)? = Some(preset.id.clone());
        Ok(())
    }

    fn verify(
        &self,
        generation: u64,
        expected: &Value,
        baseline: &Value,
    ) -> Result<(), RuntimeError> {
        let deadline = Instant::now() + VERIFY_TIMEOUT;
        loop {
            let current = self.config(generation)?;
            if current.get("devices") != baseline.get("devices") {
                return Err(RuntimeError::UnexpectedConfig);
            }
            if same_dsp(&current, expected)? {
                self.running(generation)?;
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(RuntimeError::VerificationFailed);
            }
            std::thread::sleep(VERIFY_INTERVAL);
        }
    }
}

fn dsp_patch(from: &Value, to: &Value) -> Result<Value, RuntimeError> {
    let previous = from.get("filters").and_then(Value::as_object);
    let next = to.get("filters").and_then(Value::as_object);
    let pipeline = to.get("pipeline").ok_or(RuntimeError::UnexpectedConfig)?;
    let mut filters = Map::new();
    if let Some(previous) = previous {
        for key in previous.keys() {
            if !key.starts_with("peq_") {
                return Err(RuntimeError::UnexpectedConfig);
            }
            filters.insert(key.clone(), Value::Null);
        }
    }
    if let Some(next) = next {
        for (key, value) in next {
            filters.insert(key.clone(), value.clone());
        }
    }
    Ok(json!({"filters":filters,"pipeline":pipeline}))
}

#[derive(Deserialize, serde::Serialize)]
struct DspView {
    #[serde(default)]
    filters: Option<BTreeMap<String, CamillaFilter>>,
    #[serde(default)]
    pipeline: Option<Vec<CamillaPipelineStep>>,
}

fn same_dsp(left: &Value, right: &Value) -> Result<bool, RuntimeError> {
    let left: DspView =
        serde_json::from_value(left.clone()).map_err(|_| RuntimeError::UnexpectedConfig)?;
    let right: DspView =
        serde_json::from_value(right.clone()).map_err(|_| RuntimeError::UnexpectedConfig)?;
    let normalized_left = json!({"filters":left.filters.unwrap_or_default(),"pipeline":left.pipeline.unwrap_or_default()});
    let normalized_right = json!({"filters":right.filters.unwrap_or_default(),"pipeline":right.pipeline.unwrap_or_default()});
    Ok(numeric_eq(&normalized_left, &normalized_right))
}

fn numeric_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() <= 0.00001,
            _ => false,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| numeric_eq(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| numeric_eq(value, other)))
        }
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::equalizer::flat_preset;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    struct FakeState {
        config: Mutex<Value>,
        volume: Mutex<f64>,
        patches: AtomicUsize,
        reject_patch: AtomicUsize,
        skip_patch: AtomicUsize,
        timeout_patch: AtomicUsize,
        stale_patch: AtomicBool,
        unavailable: AtomicBool,
        generation: Arc<AtomicU64>,
        entered: Option<Arc<Barrier>>,
        release: Option<Arc<Barrier>>,
    }

    #[derive(Clone)]
    struct FakeTransport(Arc<FakeState>);

    impl RuntimeTransport for FakeTransport {
        fn request(&self, _: u16, command: Value) -> Result<Value, RuntimeError> {
            if self.0.unavailable.load(Ordering::SeqCst) {
                return Err(RuntimeError::WebSocketConnectFailed);
            }
            if command == json!("GetState") {
                return Ok(json!({"GetState":{"result":"Ok","value":"Running"}}));
            }
            if command == json!("GetConfigJson") {
                let config = self.0.config.lock().unwrap().to_string();
                return Ok(json!({"GetConfigJson":{"result":"Ok","value":config}}));
            }
            if command == json!("GetVolume") {
                return Ok(
                    json!({"GetVolume":{"result":"Ok","value":*self.0.volume.lock().unwrap()}}),
                );
            }
            if let Some(volume) = command.get("SetVolume").and_then(Value::as_f64) {
                *self.0.volume.lock().unwrap() = volume;
                return Ok(json!({"SetVolume":{"result":"Ok"}}));
            }
            let patch = command
                .get("PatchConfig")
                .ok_or(RuntimeError::UnexpectedReply)?;
            let number = self.0.patches.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(barrier) = &self.0.entered {
                if number == 1 {
                    barrier.wait();
                }
            }
            if let Some(barrier) = &self.0.release {
                if number == 1 {
                    barrier.wait();
                }
            }
            if self.0.stale_patch.swap(false, Ordering::SeqCst) {
                self.0.generation.store(99, Ordering::SeqCst);
            }
            if self.0.timeout_patch.load(Ordering::SeqCst) == number {
                return Err(RuntimeError::RequestTimeout);
            }
            if self.0.reject_patch.load(Ordering::SeqCst) == number {
                return Ok(json!({"PatchConfig":{"result":{"ConfigValidationError":"rejected"}}}));
            }
            if self.0.skip_patch.load(Ordering::SeqCst) != number {
                let mut config = self.0.config.lock().unwrap();
                if let Some(filters) = patch.get("filters").and_then(Value::as_object) {
                    let destination = config["filters"].as_object_mut();
                    if destination.is_none() {
                        config["filters"] = json!({});
                    }
                    let destination = config["filters"].as_object_mut().unwrap();
                    for (key, value) in filters {
                        if value.is_null() {
                            destination.remove(key);
                        } else {
                            destination.insert(key.clone(), value.clone());
                        }
                    }
                }
                config["pipeline"] = patch["pipeline"].clone();
            }
            Ok(json!({"PatchConfig":{"result":"Ok"}}))
        }
    }

    fn fixture(rate: u32) -> (Arc<CamillaRuntimeController<FakeTransport>>, Arc<FakeState>) {
        let generation = Arc::new(AtomicU64::new(7));
        let state = Arc::new(FakeState {
            config: Mutex::new(
                json!({"title":"private_token","devices":{"samplerate":rate,"volume_ramp_time":100,"capture":{"node_name":"cap"},"playback":{"node_name":"play"}},"filters":null,"pipeline":[]}),
            ),
            volume: Mutex::new(0.0),
            patches: AtomicUsize::new(0),
            reject_patch: AtomicUsize::new(0),
            skip_patch: AtomicUsize::new(0),
            timeout_patch: AtomicUsize::new(0),
            stale_patch: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            generation: Arc::clone(&generation),
            entered: None,
            release: None,
        });
        let endpoint = RuntimeEndpoint {
            port: 1234,
            title: "private_token".into(),
            generation: 7,
            capture_name: "cap".into(),
            playback_name: "play".into(),
        };
        let controller = Arc::new(CamillaRuntimeController::new(
            endpoint,
            generation,
            Arc::new(RuntimeShared::default()),
            FakeTransport(Arc::clone(&state)),
        ));
        (controller, state)
    }

    fn pcm(rate: u32) -> PcmFormat {
        PcmFormat {
            sample_rate_hz: rate,
            channels: 2,
        }
    }

    #[test]
    fn flat_one_band_and_ten_band_apply_only_after_observed_config() {
        for rate in [44_100, 48_000, 96_000] {
            let (controller, state) = fixture(rate);
            let mut flat = flat_preset();
            assert_eq!(controller.published_preset().unwrap(), None);
            controller.apply_preset(7, &flat, pcm(rate)).unwrap();
            assert_eq!(
                controller.published_preset().unwrap(),
                Some(flat.id.clone())
            );
            assert_eq!(
                state.config.lock().unwrap()["pipeline"]
                    .as_array()
                    .unwrap()
                    .len(),
                11
            );
            flat.bands.truncate(1);
            flat.id = "one".into();
            controller.apply_preset(7, &flat, pcm(rate)).unwrap();
            assert_eq!(controller.published_preset().unwrap(), Some("one".into()));
            assert_eq!(
                state.config.lock().unwrap()["pipeline"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert_eq!(state.config.lock().unwrap()["devices"]["samplerate"], rate);
        }
    }

    #[test]
    fn zero_gain_enable_disable_and_frequency_gain_q_update_do_not_leave_old_filters() {
        let (controller, state) = fixture(48_000);
        let mut preset = flat_preset();
        preset.bands.truncate(1);
        controller.apply_preset(7, &preset, pcm(48_000)).unwrap();
        let name = state.config.lock().unwrap()["pipeline"][1]["names"][0]
            .as_str()
            .unwrap()
            .to_owned();
        preset.bands[0].gain_db = 3.0;
        preset.bands[0].frequency_hz = 1100.0;
        preset.bands[0].q = 2.0;
        controller.apply_preset(7, &preset, pcm(48_000)).unwrap();
        assert_eq!(
            state.config.lock().unwrap()["filters"][&name]["parameters"],
            json!({"type":"Peaking","freq":1100.0,"gain":3.0,"q":2.0})
        );
        preset.bands[0].gain_db = 0.0;
        controller.apply_preset(7, &preset, pcm(48_000)).unwrap();
        assert_eq!(
            state.config.lock().unwrap()["filters"][&name]["parameters"]["gain"],
            0.0
        );
        preset.bands[0].enabled = false;
        controller.apply_preset(7, &preset, pcm(48_000)).unwrap();
        assert!(state.config.lock().unwrap()["filters"].get(&name).is_none());
        assert_eq!(
            state.config.lock().unwrap()["pipeline"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        preset.bands[0].enabled = true;
        controller.apply_preset(7, &preset, pcm(48_000)).unwrap();
        assert!(state.config.lock().unwrap()["filters"].get(&name).is_some());
    }

    #[test]
    fn unavailable_wrong_identity_and_format_fail_before_patch() {
        let (controller, state) = fixture(48_000);
        let preset = flat_preset();
        state.unavailable.store(true, Ordering::SeqCst);
        assert_eq!(
            controller.apply_preset(7, &preset, pcm(48_000)),
            Err(RuntimeError::WebSocketConnectFailed)
        );
        state.unavailable.store(false, Ordering::SeqCst);
        state.config.lock().unwrap()["title"] = json!("foreign");
        assert_eq!(
            controller.apply_preset(7, &preset, pcm(48_000)),
            Err(RuntimeError::UnexpectedConfig)
        );
        state.config.lock().unwrap()["title"] = json!("private_token");
        assert_eq!(
            controller.apply_preset(7, &preset, pcm(44_100)),
            Err(RuntimeError::UnexpectedConfig)
        );
        assert_eq!(state.patches.load(Ordering::SeqCst), 0);
        assert_eq!(controller.published_preset().unwrap(), None);
    }

    #[test]
    fn rejection_and_timeout_restore_previous_and_do_not_publish() {
        for timeout in [false, true] {
            let (controller, state) = fixture(48_000);
            let flat = flat_preset();
            controller.apply_preset(7, &flat, pcm(48_000)).unwrap();
            let before = state.config.lock().unwrap().clone();
            let mut changed = flat.clone();
            changed.id = "changed".into();
            changed.bands[0].gain_db = 6.0;
            let next = state.patches.load(Ordering::SeqCst) + 1;
            if timeout {
                state.timeout_patch.store(next, Ordering::SeqCst);
            } else {
                state.reject_patch.store(next, Ordering::SeqCst);
            }
            assert_eq!(
                controller.apply_preset(7, &changed, pcm(48_000)),
                Err(if timeout {
                    RuntimeError::RequestTimeout
                } else {
                    RuntimeError::ApplyRejected
                })
            );
            assert!(same_dsp(&state.config.lock().unwrap(), &before).unwrap());
            assert_eq!(controller.published_preset().unwrap(), Some(flat.id));
        }
    }

    #[test]
    fn verification_mismatch_rolls_back_and_rollback_failure_is_distinct() {
        let (controller, state) = fixture(48_000);
        let preset = flat_preset();
        state.skip_patch.store(1, Ordering::SeqCst);
        assert_eq!(
            controller.apply_preset(7, &preset, pcm(48_000)),
            Err(RuntimeError::VerificationFailed)
        );
        assert_eq!(controller.published_preset().unwrap(), None);
        assert!(state.config.lock().unwrap()["pipeline"]
            .as_array()
            .unwrap()
            .is_empty());
        let (controller, state) = fixture(48_000);
        state.skip_patch.store(1, Ordering::SeqCst);
        state.reject_patch.store(2, Ordering::SeqCst);
        assert_eq!(
            controller.apply_preset(7, &preset, pcm(48_000)),
            Err(RuntimeError::RollbackFailed)
        );
        assert_eq!(controller.published_preset().unwrap(), None);
    }

    #[test]
    fn stale_completion_never_publishes_to_new_session() {
        let (controller, state) = fixture(48_000);
        state.stale_patch.store(true, Ordering::SeqCst);
        assert_eq!(
            controller.apply_preset(7, &flat_preset(), pcm(48_000)),
            Err(RuntimeError::SessionStale)
        );
        assert_eq!(controller.published_preset().unwrap(), None);
        assert_eq!(
            controller.apply_preset(7, &flat_preset(), pcm(48_000)),
            Err(RuntimeError::SessionStale)
        );
    }

    #[test]
    fn concurrent_apply_is_rejected_without_interleaving() {
        let generation = Arc::new(AtomicU64::new(7));
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let state = Arc::new(FakeState {
            config: Mutex::new(
                json!({"title":"private_token","devices":{"samplerate":48000,"volume_ramp_time":100,"capture":{"node_name":"cap"},"playback":{"node_name":"play"}},"filters":null,"pipeline":[]}),
            ),
            volume: Mutex::new(0.0),
            patches: AtomicUsize::new(0),
            reject_patch: AtomicUsize::new(0),
            skip_patch: AtomicUsize::new(0),
            timeout_patch: AtomicUsize::new(0),
            stale_patch: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            generation: Arc::clone(&generation),
            entered: Some(Arc::clone(&entered)),
            release: Some(Arc::clone(&release)),
        });
        let endpoint = RuntimeEndpoint {
            port: 1234,
            title: "private_token".into(),
            generation: 7,
            capture_name: "cap".into(),
            playback_name: "play".into(),
        };
        let shared = Arc::new(RuntimeShared::default());
        let other_controller = CamillaRuntimeController::new(
            endpoint.clone(),
            Arc::clone(&generation),
            Arc::clone(&shared),
            FakeTransport(Arc::clone(&state)),
        );
        let controller = Arc::new(CamillaRuntimeController::new(
            endpoint,
            generation,
            shared,
            FakeTransport(Arc::clone(&state)),
        ));
        let worker = Arc::clone(&controller);
        let handle =
            std::thread::spawn(move || worker.apply_preset(7, &flat_preset(), pcm(48_000)));
        entered.wait();
        assert_eq!(
            other_controller.apply_preset(7, &flat_preset(), pcm(48_000)),
            Err(RuntimeError::ConcurrentApply)
        );
        release.wait();
        assert_eq!(handle.join().unwrap(), Ok(()));
        assert_eq!(
            other_controller.published_preset().unwrap(),
            Some("flat".into())
        );
        assert_eq!(state.patches.load(Ordering::SeqCst), 1);
    }
}
