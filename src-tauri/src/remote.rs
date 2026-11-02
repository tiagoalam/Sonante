//! Explicit, paired LAN remote. No player polling or listening socket when off.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    net::{Ipv4Addr, TcpListener},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub(crate) type Guard = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) const DEFAULT_PORT: u16 = 8787;
pub(crate) const CLIENT_LIFETIME: u64 = 30 * 24 * 3600;
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preferences {
    pub enabled: bool,
    pub port: u16,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
        }
    }
}
#[derive(Clone, Default, Serialize)]
pub(crate) struct Snapshot {
    pub schema: u8,
    pub available: bool,
    pub state: String,
    pub elapsed: f64,
    pub duration: f64,
    pub media: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source: String,
    pub audio_format: String,
    pub index: Option<usize>,
    pub volume: u32,
    pub muted: bool,
    pub volume_writable: bool,
}
#[derive(Serialize)]
pub(crate) struct QueueTrack {
    pub index: usize,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<f64>,
}
#[derive(Serialize)]
pub(crate) struct QueuePage {
    pub revision: String,
    pub total: usize,
    pub offset: usize,
    pub tracks: Vec<QueueTrack>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Command {
    Play { media: String },
    Pause { media: String },
    Next { media: String },
    Previous { media: String },
    Seek { media: String, seconds: f64 },
    Volume { value: u32 },
    QueuePlay { index: usize, revision: String },
}
impl Command {
    pub(crate) fn valid(&self) -> bool {
        let media = |v: &str| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit());
        match self {
            Self::Play { media: m }
            | Self::Pause { media: m }
            | Self::Next { media: m }
            | Self::Previous { media: m } => media(m),
            Self::Seek { media: m, seconds } => {
                media(m) && seconds.is_finite() && (0.0..=86400.0).contains(seconds)
            }
            Self::Volume { value } => *value <= 100,
            Self::QueuePlay { index, revision } => {
                *index < 100_000
                    && revision.len() == 67
                    && revision.starts_with("q1:")
                    && revision[3..].bytes().all(|b| b.is_ascii_hexdigit())
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    pub request_id: String,
    pub command: Command,
}
impl Control {
    pub(crate) fn valid(&self) -> bool {
        (8..=64).contains(&self.request_id.len())
            && self
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && self.command.valid()
    }
}
pub(crate) trait Backend: Send + Sync + 'static {
    fn active(&self) -> bool;
    fn credential_path(&self) -> Option<PathBuf> {
        None
    }
    fn queue(&self, offset: usize) -> Result<QueuePage, String>;
    fn control(&self, command: Command, current: Guard) -> Result<(), String>;
    fn artwork(&self, key: &str, current: Guard) -> Result<Option<(String, Vec<u8>)>, String>;
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "REMOTE_RANDOM")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn constant(a: &str, b: &str) -> bool {
    let x = Sha256::digest(a.as_bytes());
    let y = Sha256::digest(b.as_bytes());
    x.iter().zip(y).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
pub(crate) fn public_error(error: &str) -> String {
    if error.contains("QUEUE_EDIT_CHANGED") || error.contains("REMOTE_CHANGED") {
        "changed"
    } else if error.contains("DIRECT")
        || error.contains("VOLUME_POLICY")
        || error.contains("VolumeEquipment")
    {
        "desktop_required"
    } else if error.contains("SUPERSEDED")
        || error.contains("INDEXING")
        || error.contains("BUSY")
        || error.contains("TRANSACTION")
    {
        "busy"
    } else {
        "unavailable"
    }
    .into()
}
#[derive(Clone)]
struct Pairing {
    code: String,
    until: Instant,
    attempts: u8,
}
#[derive(Clone)]
struct Remembered {
    id: String,
    digest: String,
    result: Option<Result<(), String>>,
}
#[derive(Clone)]
struct Client {
    name: String,
    until: u64,
    requests: VecDeque<Remembered>,
}
#[derive(Clone, Default)]
pub(crate) struct Auth {
    pair: Option<Pairing>,
    clients: BTreeMap<String, Client>,
}
impl Auth {
    fn prune(&mut self, now: Instant) {
        self.clients.retain(|_, v| v.until > unix_now());
        if self.pair.as_ref().is_some_and(|p| p.until <= now) {
            self.pair = None;
        }
    }
    pub(crate) fn begin(&mut self) -> Result<(), String> {
        let bytes = secret()?;
        let value = u64::from_str_radix(&bytes[..12], 16).map_err(|_| "REMOTE_RANDOM")? % 1_000_000;
        self.pair = Some(Pairing {
            code: format!("{value:06}"),
            until: Instant::now() + Duration::from_secs(120),
            attempts: 0,
        });
        Ok(())
    }
    pub(crate) fn pair(&mut self, code: &str, name: &str) -> Result<String, String> {
        let now = Instant::now();
        self.prune(now);
        let pair = self.pair.as_mut().ok_or("pairing_closed")?;
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid".into());
        }
        pair.attempts += 1;
        let matches = constant(&pair.code, code);
        if !matches {
            if pair.attempts >= 5 {
                self.pair = None;
            }
            return Err("pairing_failed".into());
        }
        if self.clients.len() >= 8 {
            return Err("device_limit".into());
        }
        let token = secret()?;
        let name = name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(48)
            .collect::<String>();
        self.clients.insert(
            digest(token.as_bytes()),
            Client {
                name: if name.is_empty() {
                    "Browser".into()
                } else {
                    name
                },
                until: unix_now() + CLIENT_LIFETIME,
                requests: VecDeque::new(),
            },
        );
        self.pair = None;
        Ok(token)
    }
    pub(crate) fn allowed(&self, token: &str) -> bool {
        token.len() == 64
            && token.bytes().all(|b| b.is_ascii_hexdigit())
            && self
                .clients
                .get(&digest(token.as_bytes()))
                .is_some_and(|v| v.until > unix_now())
    }
    pub(crate) fn claim(
        &mut self,
        token: &str,
        control: &Control,
    ) -> Result<Option<Result<(), String>>, String> {
        if !self.allowed(token) {
            return Err("unauthorized".into());
        }
        let signature = digest(&serde_json::to_vec(&control.command).map_err(|_| "invalid")?);
        let client = self
            .clients
            .get_mut(&digest(token.as_bytes()))
            .ok_or("unauthorized")?;
        if let Some(previous) = client.requests.iter().find(|p| p.id == control.request_id) {
            if previous.digest != signature {
                return Err("invalid".into());
            }
            return previous.result.clone().map(Some).ok_or("busy".into());
        }
        while client.requests.len() >= 32 {
            client.requests.pop_front();
        }
        client.requests.push_back(Remembered {
            id: control.request_id.clone(),
            digest: signature,
            result: None,
        });
        Ok(None)
    }
    pub(crate) fn finish(&mut self, token: &str, id: &str, result: Result<(), String>) {
        if let Some(client) = self.clients.get_mut(&digest(token.as_bytes())) {
            if let Some(request) = client.requests.iter_mut().find(|r| r.id == id) {
                request.result = Some(result);
            }
        }
    }
}
pub(crate) struct Stamped {
    pub snapshot: Snapshot,
    pub at: Instant,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedClient {
    token_hash: String,
    name: String,
    expires_at: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedClients {
    schema: u8,
    clients: Vec<SavedClient>,
}
impl Auth {
    fn saved(&self) -> SavedClients {
        SavedClients {
            schema: 1,
            clients: self
                .clients
                .iter()
                .filter(|(_, v)| v.until > unix_now())
                .map(|(hash, v)| SavedClient {
                    token_hash: hash.clone(),
                    name: v.name.clone(),
                    expires_at: v.until,
                })
                .collect(),
        }
    }
    fn restore(saved: SavedClients) -> Result<Self, String> {
        let now = unix_now();
        if saved.schema != 1 || saved.clients.len() > 8 {
            return Err("REMOTE_PREFERENCES".into());
        }
        let mut auth = Self::default();
        for record in saved.clients {
            if record.token_hash.len() != 64
                || !record
                    .token_hash
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                || record.name.chars().count() > 48
                || record.name.chars().any(|c| c.is_control())
                || record.expires_at > now.saturating_add(CLIENT_LIFETIME)
            {
                return Err("REMOTE_PREFERENCES".into());
            }
            if record.expires_at <= now {
                continue;
            }
            if auth
                .clients
                .insert(
                    record.token_hash,
                    Client {
                        name: record.name,
                        until: record.expires_at,
                        requests: VecDeque::new(),
                    },
                )
                .is_some()
            {
                return Err("REMOTE_PREFERENCES".into());
            }
        }
        Ok(auth)
    }
}
pub(crate) struct Shared {
    pub snapshot: Mutex<Option<Stamped>>,
    pub auth: Mutex<Auth>,
    auth_write: Mutex<()>,
    credential_path: Option<PathBuf>,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(None),
            auth: Mutex::new(Auth::default()),
            auth_write: Mutex::new(()),
            credential_path: None,
        }
    }
}
impl Shared {
    fn with_store(path: Option<PathBuf>) -> Result<Self, String> {
        let mut value = Self {
            credential_path: path,
            ..Self::default()
        };
        if let Some(path) = &value.credential_path {
            use std::io::Read;
            use std::os::unix::fs::OpenOptionsExt;
            if path.try_exists().map_err(|_| "REMOTE_PREFERENCES")? {
                crate::persistence::prepare_private_file_for_load(path, "remote-clients.json")
                    .map_err(|_| "REMOTE_PREFERENCES")?;
                let mut data = Vec::new();
                std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(path)
                    .map_err(|_| "REMOTE_PREFERENCES")?
                    .take(16385)
                    .read_to_end(&mut data)
                    .map_err(|_| "REMOTE_PREFERENCES")?;
                if data.len() > 16384 {
                    return Err("REMOTE_PREFERENCES".into());
                }
                value.auth = Mutex::new(Auth::restore(
                    serde_json::from_slice(&data).map_err(|_| "REMOTE_PREFERENCES")?,
                )?);
            }
        }
        Ok(value)
    }
    fn save(&self, saved: &SavedClients) -> Result<(), String> {
        if let Some(path) = &self.credential_path {
            crate::persistence::atomic_write_private(
                path,
                &serde_json::to_vec(saved).map_err(|_| "REMOTE_PREFERENCES")?,
                "remote-clients.json",
            )
            .map_err(|_| "REMOTE_PREFERENCES")?;
        }
        Ok(())
    }
    pub(crate) fn pair(&self, code: &str, name: &str) -> Result<String, String> {
        let _serial = self.auth_write.lock().map_err(|_| "busy")?;
        let (token, hash, client, saved) = {
            let mut auth = self.auth.lock().map_err(|_| "busy")?;
            let mut candidate = auth.clone();
            let result = candidate.pair(code, name);
            // Consume the code/attempt even if persistence fails, but never expose
            // a new credential until its private durable write is confirmed.
            auth.pair = candidate.pair.take();
            let token = result?;
            let hash = digest(token.as_bytes());
            let saved = candidate.saved();
            let client = candidate.clients.remove(&hash).ok_or("unavailable")?;
            (token, hash, client, saved)
        };
        self.save(&saved).map_err(|_| "unavailable")?;
        self.auth
            .lock()
            .map_err(|_| "busy")?
            .clients
            .insert(hash, client);
        Ok(token)
    }
    pub(crate) fn revoke(&self) -> Result<(), String> {
        let _serial = self.auth_write.lock().map_err(|_| "REMOTE_BUSY")?;
        *self.auth.lock().map_err(|_| "REMOTE_BUSY")? = Auth::default();
        self.save(&SavedClients {
            schema: 1,
            clients: Vec::new(),
        })
    }
    pub(crate) fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .ok()
            .and_then(|s| {
                s.as_ref().map(|s| {
                    let mut v = s.snapshot.clone();
                    if s.at.elapsed() > Duration::from_secs(6) {
                        v.available = false;
                        v.volume_writable = false;
                    }
                    v
                })
            })
            .unwrap_or(Snapshot {
                schema: 1,
                ..Snapshot::default()
            })
    }
}
struct Worker {
    stop: Arc<AtomicBool>,
    join: thread::JoinHandle<()>,
    port: u16,
    shared: Arc<Shared>,
}
impl Worker {
    fn shutdown(self) {
        self.stop.store(true, Ordering::Release);
        if self.join.join().is_err() {
            eprintln!("[Remote] worker=interrupted");
        }
    }
}
#[derive(Default)]
struct Runtime {
    preferences: Preferences,
    worker: Option<Worker>,
}
#[derive(Default)]
pub(crate) struct State {
    pub operation: Mutex<()>,
    runtime: Mutex<Runtime>,
    pub enabled: AtomicBool,
}
#[derive(Serialize)]
pub(crate) struct Status {
    pub enabled: bool,
    pub requested_enabled: bool,
    pub port: u16,
    pub urls: Vec<String>,
    pub code: Option<String>,
    pub expires_in: u64,
    pub devices: Vec<String>,
}
impl State {
    pub(crate) fn configure(
        &self,
        prefs: Preferences,
        backend: Arc<dyn Backend>,
    ) -> Result<(), String> {
        if prefs.port < 1024 {
            return Err("REMOTE_INVALID_PORT".into());
        }
        let previous = {
            let mut runtime = self.runtime.lock().map_err(|_| "REMOTE_BUSY")?;
            if runtime.preferences.enabled == prefs.enabled
                && runtime.preferences.port == prefs.port
                && runtime
                    .worker
                    .as_ref()
                    .is_some_and(|w| !w.stop.load(Ordering::Acquire) && !w.join.is_finished())
            {
                return Ok(());
            }
            self.enabled.store(false, Ordering::Release);
            runtime.preferences = prefs.clone();
            runtime.worker.take()
        };
        let previous_shared = previous.as_ref().map(|w| w.shared.clone());
        if let Some(worker) = previous {
            worker.shutdown();
        }
        if !prefs.enabled {
            let shared = previous_shared.unwrap_or_else(|| {
                Arc::new(Shared {
                    credential_path: backend.credential_path(),
                    ..Shared::default()
                })
            });
            shared.revoke()?;
            return Ok(());
        }
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, prefs.port))
            .map_err(|_| "REMOTE_PORT_UNAVAILABLE")?;
        listener
            .set_nonblocking(true)
            .map_err(|_| "REMOTE_NETWORK")?;
        let port = listener.local_addr().map_err(|_| "REMOTE_NETWORK")?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::with_store(backend.credential_path())?);
        let shutdown = stop.clone();
        let data = shared.clone();
        let join = thread::Builder::new()
            .name("sonante-remote".into())
            .spawn(move || crate::remote_http::serve(listener, shutdown, data, backend))
            .map_err(|_| "REMOTE_NETWORK")?;
        self.runtime.lock().map_err(|_| "REMOTE_BUSY")?.worker = Some(Worker {
            stop,
            join,
            port,
            shared,
        });
        self.enabled.store(true, Ordering::Release);
        Ok(())
    }
    pub(crate) fn preferences(&self) -> Result<Preferences, String> {
        Ok(self
            .runtime
            .lock()
            .map_err(|_| "REMOTE_BUSY")?
            .preferences
            .clone())
    }
    pub(crate) fn shared(&self) -> Option<Arc<Shared>> {
        self.runtime
            .lock()
            .ok()?
            .worker
            .as_ref()
            .map(|w| w.shared.clone())
    }
    pub(crate) fn publish(&self, snapshot: Snapshot) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        if let Some(shared) = self.shared() {
            if let Ok(mut value) = shared.snapshot.lock() {
                *value = Some(Stamped {
                    snapshot,
                    at: Instant::now(),
                });
            }
        }
    }
    pub(crate) fn status(&self, addresses: Vec<Ipv4Addr>) -> Result<Status, String> {
        let runtime = self.runtime.lock().map_err(|_| "REMOTE_BUSY")?;
        let running = runtime
            .worker
            .as_ref()
            .filter(|w| !w.stop.load(Ordering::Acquire) && !w.join.is_finished());
        let port = running.map(|w| w.port).unwrap_or(runtime.preferences.port);
        let mut status = Status {
            enabled: running.is_some(),
            requested_enabled: runtime.preferences.enabled,
            port,
            urls: addresses
                .iter()
                .map(|ip| format!("http://{ip}:{port}"))
                .collect(),
            code: None,
            expires_in: 0,
            devices: Vec::new(),
        };
        if let Some(worker) = running {
            let mut auth = worker.shared.auth.lock().map_err(|_| "REMOTE_BUSY")?;
            let now = Instant::now();
            auth.prune(now);
            status.devices = auth.clients.values().map(|v| v.name.clone()).collect();
            if let Some(pair) = &auth.pair {
                status.code = Some(pair.code.clone());
                status.expires_in = pair.until.saturating_duration_since(now).as_secs();
            }
        }
        Ok(status)
    }
    pub(crate) fn shutdown(&self) {
        let Ok(_serial) = self.operation.lock() else {
            return;
        };
        self.enabled.store(false, Ordering::Release);
        let worker = self
            .runtime
            .lock()
            .ok()
            .and_then(|mut runtime| runtime.worker.take());
        if let Some(worker) = worker {
            worker.shutdown();
        }
    }
}

#[cfg(test)]
impl Auth {
    pub(crate) fn test_token(&mut self) -> String {
        self.begin().unwrap();
        let code = self.pair.as_ref().unwrap().code.clone();
        self.pair(&code, "Fixture").unwrap()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_defaults_never_enable_a_listener() {
        let state = State::default();
        assert!(!state.enabled.load(Ordering::Acquire));
        let status = state.status(Vec::new()).unwrap();
        assert!(!status.enabled);
        assert!(!status.requested_enabled);
        assert_eq!(status.port, 8787);
        assert!(state.shared().is_none());
    }
    #[test]
    fn remote_pairing_is_short_lived_single_use_and_stops_after_five_failures() {
        let mut auth = Auth::default();
        auth.begin().unwrap();
        let code = auth.pair.as_ref().unwrap().code.clone();
        let wrong = if code == "000000" { "999999" } else { "000000" };
        for _ in 0..5 {
            assert!(auth.pair(wrong, "Fixture").is_err());
        }
        assert!(auth.pair(&code, "Fixture").is_err());
        auth.begin().unwrap();
        auth.pair.as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
        assert!(auth.pair("123456", "Fixture").is_err());
        let token = auth.test_token();
        assert!(auth.allowed(&token));
        assert!(auth.pair("123456", "Fixture").is_err());
        assert!(!auth.clients.contains_key(&token));
    }
    #[test]
    fn remote_expiration_revocation_and_device_limits_are_enforced() {
        let mut auth = Auth::default();
        let mut tokens = Vec::new();
        for _ in 0..8 {
            tokens.push(auth.test_token());
        }
        auth.begin().unwrap();
        let code = auth.pair.as_ref().unwrap().code.clone();
        assert!(auth.pair(&code, "Ninth").is_err());
        auth.clients
            .get_mut(&digest(tokens[0].as_bytes()))
            .unwrap()
            .until = unix_now() - 1;
        assert!(!auth.allowed(&tokens[0]));
        assert!(auth.allowed(&tokens[1]));
        auth = Auth::default();
        assert!(!auth.allowed(&tokens[1]));
    }
    #[test]
    fn remote_request_replay_returns_the_original_ack_without_reapplying_a_command() {
        let mut auth = Auth::default();
        let token = auth.test_token();
        let control = Control {
            request_id: "fixture-command".into(),
            command: Command::Next {
                media: "a".repeat(64),
            },
        };
        assert!(auth.claim(&token, &control).unwrap().is_none());
        assert_eq!(auth.claim(&token, &control).unwrap_err(), "busy");
        auth.finish(&token, &control.request_id, Ok(()));
        assert!(matches!(
            auth.claim(&token, &control).unwrap(),
            Some(Ok(()))
        ));
        let other = Control {
            request_id: control.request_id,
            command: Command::Previous {
                media: "a".repeat(64),
            },
        };
        assert_eq!(auth.claim(&token, &other).unwrap_err(), "invalid");
    }
    #[test]
    fn remote_command_schema_rejects_paths_unknown_actions_invalid_ranges_and_extra_fields() {
        for json in [
            r#"{"action":"seek","media":"x","seconds":1}"#,
            r#"{"action":"volume","value":101}"#,
            r#"{"action":"volume","value":10,"uri":"private/path"}"#,
            r#"{"action":"exec","command":"anything"}"#,
            r#"{"action":"queue_play","index":0,"revision":"obsolete"}"#,
        ] {
            assert!(!serde_json::from_str::<Command>(json).is_ok_and(|v| v.valid()));
        }
        assert!(!Command::Seek {
            media: "a".repeat(64),
            seconds: f64::NAN
        }
        .valid());
        assert!(!Command::Seek {
            media: "a".repeat(64),
            seconds: -1.
        }
        .valid());
        assert!(Command::QueuePlay {
            index: 0,
            revision: format!("q1:{}", "f".repeat(64))
        }
        .valid());
        assert_eq!(public_error("/private/path?token=secret"), "unavailable");
    }
    #[test]
    fn remote_cached_telemetry_becomes_unavailable_without_background_player_reads() {
        let shared = Shared::default();
        assert!(!shared.snapshot().available);
        *shared.snapshot.lock().unwrap() = Some(Stamped {
            snapshot: Snapshot {
                schema: 1,
                available: true,
                volume_writable: true,
                ..Snapshot::default()
            },
            at: Instant::now() - Duration::from_secs(7),
        });
        assert!(!shared.snapshot().available);
        assert!(!shared.snapshot().volume_writable);
    }
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "sonante-remote-auth-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn paired(shared: &Shared) -> String {
        let code = {
            let mut auth = shared.auth.lock().unwrap();
            auth.begin().unwrap();
            auth.pair.as_ref().unwrap().code.clone()
        };
        shared.pair(&code, "Firefox on Linux").unwrap()
    }
    #[test]
    fn remote_saved_grant_survives_restart_without_storing_bearer_and_revocation_survives_restart()
    {
        let dir = Scratch::new();
        let file = dir.0.join("clients.json");
        let first = Shared::with_store(Some(file.clone())).unwrap();
        let token = paired(&first);
        let bytes = std::fs::read_to_string(&file).unwrap();
        assert!(!bytes.contains(&token));
        assert!(bytes.contains(&digest(token.as_bytes())));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(first);
        let restored = Shared::with_store(Some(file.clone())).unwrap();
        assert!(restored.auth.lock().unwrap().allowed(&token));
        assert!(restored.auth.lock().unwrap().pair.is_none());
        restored.revoke().unwrap();
        assert!(!restored.auth.lock().unwrap().allowed(&token));
        drop(restored);
        assert!(!Shared::with_store(Some(file))
            .unwrap()
            .auth
            .lock()
            .unwrap()
            .allowed(&token));
    }
    #[test]
    fn remote_expired_or_malformed_saved_grants_never_authorize_a_device() {
        let dir = Scratch::new();
        let file = dir.0.join("clients.json");
        let shared = Shared::with_store(Some(file.clone())).unwrap();
        let token = paired(&shared);
        let mut records = shared.auth.lock().unwrap().saved();
        records.clients[0].expires_at = unix_now() - 1;
        shared.save(&records).unwrap();
        assert!(!Shared::with_store(Some(file.clone()))
            .unwrap()
            .auth
            .lock()
            .unwrap()
            .allowed(&token));
        for bad in [
            serde_json::json!({"schema":2,"clients":[]}),
            serde_json::json!({"schema":1,"clients":[{"token_hash":"not-a-hash","name":"Fixture","expires_at":unix_now()+10}]}),
            serde_json::json!({"schema":1,"clients":[{"token_hash":"a".repeat(64),"name":"Fixture","expires_at":unix_now()+CLIENT_LIFETIME+100}]}),
        ] {
            std::fs::write(&file, serde_json::to_vec(&bad).unwrap()).unwrap();
            assert!(Shared::with_store(Some(file.clone())).is_err());
        }
    }
    #[test]
    fn remote_failed_credential_write_never_authorizes_or_returns_a_new_grant() {
        let dir = Scratch::new();
        let parent = dir.0.join("not-a-directory");
        std::fs::write(&parent, b"fixture").unwrap();
        let shared = Shared {
            credential_path: Some(parent.join("clients.json")),
            ..Shared::default()
        };
        let code = {
            let mut auth = shared.auth.lock().unwrap();
            auth.begin().unwrap();
            auth.pair.as_ref().unwrap().code.clone()
        };
        assert!(shared.pair(&code, "Fixture").is_err());
        assert!(shared.auth.lock().unwrap().clients.is_empty());
    }
    #[test]
    fn remote_disabling_an_inactive_listener_also_revokes_saved_devices() {
        struct SavedBackend(std::path::PathBuf);
        impl Backend for SavedBackend {
            fn active(&self) -> bool {
                false
            }
            fn credential_path(&self) -> Option<std::path::PathBuf> {
                Some(self.0.clone())
            }
            fn queue(&self, _: usize) -> Result<QueuePage, String> {
                panic!("no player work")
            }
            fn control(&self, _: Command, _: Guard) -> Result<(), String> {
                panic!("no player work")
            }
            fn artwork(&self, _: &str, _: Guard) -> Result<Option<(String, Vec<u8>)>, String> {
                panic!("no player work")
            }
        }
        let dir = Scratch::new();
        let file = dir.0.join("clients.json");
        let shared = Shared::with_store(Some(file.clone())).unwrap();
        let token = paired(&shared);
        drop(shared);
        let state = State::default();
        state
            .configure(
                Preferences {
                    enabled: false,
                    port: DEFAULT_PORT,
                },
                Arc::new(SavedBackend(file.clone())),
            )
            .unwrap();
        assert!(state.shared().is_none());
        assert!(!state.enabled.load(Ordering::Acquire));
        assert!(!Shared::with_store(Some(file))
            .unwrap()
            .auth
            .lock()
            .unwrap()
            .allowed(&token));
    }
}
