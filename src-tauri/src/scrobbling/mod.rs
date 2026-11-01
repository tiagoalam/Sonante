//! Opt-in scrobbling actor: existing playback snapshots in, HTTPS metadata out.
//! No MPD command, audio polling, artwork access or general-config save.
mod http;
mod model;
use http::{Api, Failure};
use model::{NowPlaying, Observation, Pending, Provider, Target, Track, Tracker};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};
const STORAGE: &str = "SCROBBLING_STORAGE_UNAVAILABLE";
const UNAVAILABLE: &str = "SCROBBLING_UNAVAILABLE";
const EVENT: &str = "scrobbling-changed";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PENDING: usize = 500;
fn utc() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Lastfm {
    enabled: bool,
    username: String,
    api_key: String,
    api_secret: String,
    session: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Listenbrainz {
    enabled: bool,
    username: String,
    token: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    next_id: u64,
    lastfm: Option<Lastfm>,
    listenbrainz: Option<Listenbrainz>,
    pending: Vec<Pending>,
}
impl Default for File {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            lastfm: None,
            listenbrainz: None,
            pending: vec![],
        }
    }
}
impl File {
    fn account(&self, p: Provider) -> Option<&str> {
        match p {
            Provider::Lastfm => self.lastfm.as_ref().map(|v| v.username.as_str()),
            Provider::Listenbrainz => self.listenbrainz.as_ref().map(|v| v.username.as_str()),
        }
    }
    fn enabled(&self, p: Provider) -> bool {
        match p {
            Provider::Lastfm => self.lastfm.as_ref().is_some_and(|v| v.enabled),
            Provider::Listenbrainz => self.listenbrainz.as_ref().is_some_and(|v| v.enabled),
        }
    }
    fn targets(&self, since: &[Instant; 2]) -> [Option<Target>; 2] {
        Provider::all().map(|p| {
            self.account(p).filter(|_| self.enabled(p)).map(|a| Target {
                account: a.to_string(),
                since: since[p.index()],
            })
        })
    }
    fn validate(&self) -> bool {
        if self.version != 1 || self.next_id == 0 || self.pending.len() > MAX_PENDING {
            return false;
        }
        if self.lastfm.as_ref().is_some_and(|v| {
            !http::user(&v.username)
                || !http::hex_key(&v.api_key)
                || !http::hex_key(&v.api_secret)
                || !http::lastfm_credential(&v.session)
        }) {
            return false;
        }
        if self
            .listenbrainz
            .as_ref()
            .is_some_and(|v| !http::user(&v.username) || !http::lb_token(&v.token))
        {
            return false;
        }
        let mut ids = std::collections::HashSet::new();
        self.pending.iter().all(|p| {
            p.id > 0
                && p.id < self.next_id
                && ids.insert(p.id)
                && p.track.valid()
                && p.started_at > 0
                && p.started_at <= 253_402_300_799
                && self.account(p.provider) == Some(p.account.as_str())
        })
    }
}
fn load(path: &Path) -> Result<File, String> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(File::default()),
        Err(_) => return Err(STORAGE.into()),
    };
    let m = file.metadata().map_err(|_| STORAGE)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.len() > MAX_BYTES
    {
        return Err(STORAGE.into());
    }
    let mut bytes = vec![];
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| STORAGE)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(STORAGE.into());
    }
    let data: File = serde_json::from_slice(&bytes).map_err(|_| STORAGE)?;
    if !data.validate() {
        return Err(STORAGE.into());
    }
    Ok(data)
}
#[derive(Default, Clone, Serialize)]
pub struct ServiceStatus {
    connected: bool,
    enabled: bool,
    username: Option<String>,
    queued: usize,
    last_sent: Option<u64>,
    error: Option<String>,
}
#[derive(Default, Clone, Serialize)]
pub struct Status {
    revision: u64,
    ready: bool,
    storage_error: bool,
    lastfm_authorizing: bool,
    lastfm: ServiceStatus,
    listenbrainz: ServiceStatus,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    LastfmBegin { api_key: String, api_secret: String },
    LastfmFinish,
    LastfmCancel,
    ListenbrainzConnect { token: String },
    SetEnabled { provider: Provider, enabled: bool },
    Disconnect { provider: Provider },
}
#[derive(Serialize)]
pub struct ActionResult {
    status: Status,
    authorization_url: Option<String>,
}
enum Command {
    Observe(Observation),
    Action(Action, oneshot::Sender<Result<ActionResult, String>>),
}
#[derive(Default)]
pub struct State {
    sender: Mutex<Option<mpsc::Sender<Command>>>,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    view: Arc<Mutex<Status>>,
    stopped: AtomicBool,
    active: AtomicBool,
    revision: AtomicU64,
}
impl State {
    fn snapshot(&self) -> Status {
        self.view
            .lock()
            .map(|v| v.clone())
            .unwrap_or_else(|_| Status {
                ready: true,
                storage_error: true,
                ..Status::default()
            })
    }
}
pub(crate) fn start(app: &AppHandle) {
    let state = app.state::<State>();
    let Ok(mut sender) = state.sender.lock() else {
        return;
    };
    if sender.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel(128);
    *sender = Some(tx);
    let view = state.view.clone();
    let handle = app.clone();
    let task = tauri::async_runtime::spawn(async move { run(handle, view, rx).await });
    if let Ok(mut slot) = state.task.lock() {
        *slot = Some(task);
    };
}
pub(crate) fn shutdown(app: &AppHandle) {
    let state = app.state::<State>();
    state.stopped.store(true, Ordering::Release);
    if let Ok(mut slot) = state.task.lock() {
        if let Some(task) = slot.take() {
            task.abort();
        }
    }
    if let Ok(mut sender) = state.sender.lock() {
        sender.take();
    };
}
#[tauri::command]
pub fn get_scrobbling(state: tauri::State<'_, State>) -> Status {
    state.snapshot()
}
#[tauri::command]
pub async fn set_scrobbling(app: AppHandle, action: Action) -> Result<ActionResult, String> {
    match &action {
        Action::LastfmBegin {
            api_key,
            api_secret,
        } if api_key.len() > 128 || api_secret.len() > 128 => {
            return Err("SCROBBLING_INVALID".into())
        }
        Action::ListenbrainzConnect { token } if token.len() > 128 => {
            return Err("SCROBBLING_INVALID".into())
        }
        _ => {}
    }
    let state = app.state::<State>();
    if state.stopped.load(Ordering::Acquire) {
        return Err(UNAVAILABLE.into());
    }
    let tx = state
        .sender
        .lock()
        .map_err(|_| UNAVAILABLE)?
        .clone()
        .ok_or(UNAVAILABLE)?;
    let (reply, read) = oneshot::channel();
    tx.try_send(Command::Action(action, reply))
        .map_err(|_| UNAVAILABLE)?;
    read.await.map_err(|_| UNAVAILABLE.to_string())?
}
pub(crate) fn is_enabled(app: &AppHandle) -> bool {
    app.try_state::<State>()
        .is_some_and(|s| s.active.load(Ordering::Acquire))
}
fn media_key(media:&crate::audio::CurrentMedia,sources:crate::config::EnabledSources)->Option<String>{
    use crate::audio::CurrentMedia;
    let allowed=match media{CurrentMedia::Local{..}=>sources.local,CurrentMedia::Plex{..}=>sources.plex,CurrentMedia::Navidrome{..}=>sources.navidrome,_=>false};
    allowed.then(||serde_json::to_string(media).ok()).flatten()
}
pub(crate) fn observe(
    app: &AppHandle,
    status: &crate::audio::PlaybackStatus,
    track: Option<crate::audio::TrackMetadata>,
) {
    let Some(state) = app.try_state::<State>() else {
        return;
    };
    if state.stopped.load(Ordering::Acquire) || !state.active.load(Ordering::Acquire) {
        return;
    }
    // This call is bounded and nonblocking; disk/network belong to the actor.
    let metadata = track.and_then(|t| Track::new(&t.artist, &t.title, &t.album, status.duration));
    let key = if metadata.is_some() && status.snapshot_consistent && status.playback_error.is_none() {
        status.current_media.as_ref().and_then(|media|media_key(media,crate::config::EnabledSources{local:crate::music_sources::local(),plex:crate::music_sources::plex(),navidrome:crate::music_sources::navidrome()}))
    } else {None};
    let observation = Observation {
        at: Instant::now(),
        utc: utc(),
        key,
        playing: status.state == "play",
        stopped: status.state == "stop",
        gap: !status.snapshot_consistent
            || status.playback_error.is_some()
            || matches!(
                status.current_media,
                Some(
                    crate::audio::CurrentMedia::Local { .. }
                        | crate::audio::CurrentMedia::Plex { .. } | crate::audio::CurrentMedia::Navidrome { .. }
                )
            ) && metadata.is_none(),
        elapsed: status.elapsed,
        track: metadata,
    };
    if let Ok(sender) = state.sender.try_lock() {
        if let Some(sender) = sender.as_ref() {
            let _ = sender.try_send(Command::Observe(observation));
        }
    };
}
struct Authorization {
    key: String,
    secret: String,
    token: String,
    created: Instant,
}
struct Worker {
    app: AppHandle,
    view: Arc<Mutex<Status>>,
    path: PathBuf,
    data: File,
    tracker: Tracker,
    since: [Instant; 2],
    errors: [Option<&'static str>; 2],
    last_sent: [Option<u64>; 2],
    blocked: [bool; 2],
    np: [Option<NowPlaying>; 2],
    auth: Option<Authorization>,
    storage_error: bool,
}
impl Worker {
    fn status(&self) -> Status {
        let service = |p: Provider| ServiceStatus {
            connected: self.data.account(p).is_some(),
            enabled: self.data.enabled(p),
            username: self.data.account(p).map(str::to_string),
            queued: self.data.pending.iter().filter(|v| v.provider == p).count(),
            last_sent: self.last_sent[p.index()],
            error: self.errors[p.index()].map(str::to_string),
        };
        Status {
            revision: self.app.state::<State>().revision.load(Ordering::Acquire),
            ready: true,
            storage_error: self.storage_error,
            lastfm_authorizing: self
                .auth
                .as_ref()
                .is_some_and(|a| a.created.elapsed() < Duration::from_secs(3600)),
            lastfm: service(Provider::Lastfm),
            listenbrainz: service(Provider::Listenbrainz),
        }
    }
    fn publish(&self) {
        self.app
            .state::<State>()
            .revision
            .fetch_add(1, Ordering::AcqRel);
        self.app.state::<State>().active.store(
            !self.storage_error && Provider::all().iter().any(|p| self.data.enabled(*p)),
            Ordering::Release,
        );
        let status = self.status();
        if let Ok(mut view) = self.view.lock() {
            *view = status.clone();
        }
        let _ = self.app.emit(EVENT, status);
    }
    fn commit(&mut self, data: File) -> Result<(), String> {
        let result = (|| {
            if !data.validate() {
                return Err(STORAGE.to_string());
            }
            let owner = self.app.state::<crate::profile_lease::State>();
            let lease = owner.lease().map_err(|_| STORAGE)?;
            lease.validate().map_err(|_| STORAGE)?;
            let bytes = serde_json::to_vec(&data).map_err(|_| STORAGE)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(STORAGE.into());
            }
            crate::persistence::atomic_write_private_synced(&self.path, &bytes, "scrobbling.json")
                .map_err(|_| STORAGE.to_string())
        })();
        if result.is_ok() {
            self.data = data;
        } else {
            self.storage_error = true;
            self.publish();
        }
        result
    }
    fn observation(&mut self, o: Observation) {
        let (np, listens) = self.tracker.observe(o, &self.data.targets(&self.since));
        if self.storage_error {
            return;
        }
        for job in np {
            let i = job.provider.index();
            eprintln!(
                "[Scrobbling] provider={} operation=now_playing outcome=queued",
                http::provider_label(job.provider)
            );
            self.np[i] = Some(job);
        }
        if listens.is_empty() {
            return;
        }
        let mut data = self.data.clone();
        for mut listen in listens {
            if data.pending.len() >= MAX_PENDING {
                self.errors[listen.provider.index()] = Some("queue_full");
                continue;
            }
            let Some(next) = data.next_id.checked_add(1) else {
                self.errors[listen.provider.index()] = Some("queue_full");
                continue;
            };
            listen.id = data.next_id;
            data.next_id = next;
            eprintln!(
                "[Scrobbling] provider={} operation=submit_listen outcome=queued",
                http::provider_label(listen.provider)
            );
            data.pending.push(listen);
        }
        let _ = self.commit(data);
        self.publish();
    }
    async fn action(&mut self, api: &mut Api, action: Action) -> Result<ActionResult, String> {
        if self.storage_error {
            return Err(STORAGE.into());
        }
        let mut url = None;
        let mut changed = None;
        let mut data = self.data.clone();
        let network = |e: Failure| format!("SCROBBLING_{}", e.code.to_ascii_uppercase());
        match action {
            Action::LastfmBegin {
                api_key,
                api_secret,
            } => {
                let (key, secret) = if api_key.trim().is_empty() && api_secret.trim().is_empty() {
                    let old = data.lastfm.as_ref().ok_or("SCROBBLING_INVALID")?;
                    (old.api_key.clone(), old.api_secret.clone())
                } else {
                    (api_key.trim().to_string(), api_secret.trim().to_string())
                };
                if !http::hex_key(&key) || !http::hex_key(&secret) {
                    return Err("SCROBBLING_INVALID".into());
                }
                let result = api
                    .lastfm(&key, &secret, None, "auth.getToken", vec![])
                    .await
                    .map_err(network)?;
                let token = result["token"]
                    .as_str()
                    .ok_or("SCROBBLING_PROTOCOL")?
                    .to_string();
                url = Some(http::lastfm_authorization_url(&key, &token).map_err(network)?);
                self.auth = Some(Authorization {
                    key,
                    secret,
                    token,
                    created: Instant::now(),
                });
            }
            Action::LastfmFinish => {
                let a = self
                    .auth
                    .as_ref()
                    .filter(|a| a.created.elapsed() < Duration::from_secs(3600))
                    .ok_or("SCROBBLING_EXPIRED")?;
                let result = api
                    .lastfm(
                        &a.key,
                        &a.secret,
                        None,
                        "auth.getSession",
                        vec![("token", a.token.clone())],
                    )
                    .await
                    .map_err(network)?;
                let username = result["session"]["name"]
                    .as_str()
                    .ok_or("SCROBBLING_PROTOCOL")?
                    .to_string();
                let session = result["session"]["key"]
                    .as_str()
                    .ok_or("SCROBBLING_PROTOCOL")?
                    .to_string();
                data.lastfm = Some(Lastfm {
                    enabled: true,
                    username: username.clone(),
                    api_key: a.key.clone(),
                    api_secret: a.secret.clone(),
                    session,
                });
                data.pending
                    .retain(|p| p.provider != Provider::Lastfm || p.account == username);
                self.commit(data)?;
                self.auth = None;
                changed = Some(Provider::Lastfm);
            }
            Action::LastfmCancel => self.auth = None,
            Action::ListenbrainzConnect { token } => {
                let token = token.trim().to_string();
                if !http::lb_token(&token) {
                    return Err("SCROBBLING_INVALID".into());
                }
                let username = api.validate_lb(&token).await.map_err(network)?;
                data.listenbrainz = Some(Listenbrainz {
                    enabled: true,
                    username: username.clone(),
                    token,
                });
                data.pending
                    .retain(|p| p.provider != Provider::Listenbrainz || p.account == username);
                self.commit(data)?;
                changed = Some(Provider::Listenbrainz);
            }
            Action::SetEnabled { provider, enabled } => {
                match provider {
                    Provider::Lastfm => {
                        data.lastfm.as_mut().ok_or("SCROBBLING_INVALID")?.enabled = enabled
                    }
                    Provider::Listenbrainz => {
                        data.listenbrainz
                            .as_mut()
                            .ok_or("SCROBBLING_INVALID")?
                            .enabled = enabled
                    }
                }
                if !enabled {
                    data.pending.retain(|p| p.provider != provider);
                }
                self.commit(data)?;
                if !enabled {
                    self.tracker.suspend(provider);
                }
                self.since[provider.index()] = Instant::now();
                self.np[provider.index()] = None;
            }
            Action::Disconnect { provider } => {
                match provider {
                    Provider::Lastfm => {
                        data.lastfm = None;
                        self.auth = None;
                    }
                    Provider::Listenbrainz => data.listenbrainz = None,
                }
                data.pending.retain(|p| p.provider != provider);
                self.commit(data)?;
                self.tracker.suspend(provider);
                changed = Some(provider);
            }
        }
        if let Some(p) = changed {
            let i = p.index();
            self.since[i] = Instant::now();
            self.errors[i] = None;
            self.blocked[i] = false;
            self.np[i] = None;
        }
        self.publish();
        Ok(ActionResult {
            status: self.status(),
            authorization_url: url,
        })
    }
    async fn send(
        &self,
        api: &mut Api,
        p: Provider,
        track: &Track,
        timestamp: Option<u64>,
    ) -> Result<(), Failure> {
        match p {
            Provider::Lastfm => {
                let v = self.data.lastfm.as_ref().ok_or(Failure::new("auth"))?;
                let mut args = vec![
                    ("artist", track.artist.clone()),
                    ("track", track.title.clone()),
                    ("duration", track.duration.to_string()),
                ];
                if !track.album.is_empty() {
                    args.push(("album", track.album.clone()));
                }
                if let Some(ts) = timestamp {
                    args.push(("timestamp", ts.to_string()));
                }
                api.lastfm(
                    &v.api_key,
                    &v.api_secret,
                    Some(&v.session),
                    if timestamp.is_some() {
                        "track.scrobble"
                    } else {
                        "track.updateNowPlaying"
                    },
                    args,
                )
                .await
                .map(|_| ())
            }
            Provider::Listenbrainz => {
                let v = self
                    .data
                    .listenbrainz
                    .as_ref()
                    .ok_or(Failure::new("auth"))?;
                api.submit_lb(&v.token, track, timestamp).await
            }
        }
    }
    fn due(&self) -> Option<u64> {
        self.data
            .pending
            .iter()
            .filter(|p| self.data.enabled(p.provider) && !self.blocked[p.provider.index()])
            .map(|p| p.next_attempt)
            .min()
    }
    async fn deliver(&mut self, api: &mut Api) {
        if self.storage_error {
            return;
        }
        let valid = self
            .app
            .state::<crate::profile_lease::State>()
            .lease()
            .and_then(|lease| lease.validate())
            .is_ok();
        if !valid {
            self.storage_error = true;
            self.publish();
            return;
        }
        for p in Provider::all() {
            let i = p.index();
            if let Some(job) = self.np[i].take() {
                if self.data.enabled(p)
                    && !self.blocked[i]
                    && self.data.account(p) == Some(job.account.as_str())
                    && job.observed.elapsed() <= Duration::from_secs(15)
                {
                    match self.send(api, p, &job.track, None).await {
                        Ok(()) => eprintln!(
                            "[Scrobbling] provider={} operation=now_playing outcome=accepted",
                            http::provider_label(p)
                        ),
                        Err(e) => {
                            eprintln!(
                                "[Scrobbling] provider={} operation=now_playing outcome={}",
                                http::provider_label(p),
                                e.code
                            );
                            self.errors[i] = Some(e.code);
                            if e.code == "auth" {
                                self.blocked[i] = true;
                            }
                            self.publish();
                        }
                    }
                    return;
                }
            }
        }
        let now = utc();
        let Some(job) = self
            .data
            .pending
            .iter()
            .find(|p| {
                p.next_attempt <= now
                    && self.data.enabled(p.provider)
                    && !self.blocked[p.provider.index()]
            })
            .cloned()
        else {
            return;
        };
        let result = self
            .send(api, job.provider, &job.track, Some(job.started_at))
            .await;
        eprintln!(
            "[Scrobbling] provider={} operation=submit_listen outcome={}",
            http::provider_label(job.provider),
            result.as_ref().err().map_or("accepted", |e| e.code)
        );
        let i = job.provider.index();
        let mut next = self.data.clone();
        match result {
            Ok(()) => {
                next.pending.retain(|p| p.id != job.id);
                self.last_sent[i] = Some(now);
                self.errors[i] = None;
            }
            Err(e) => {
                self.errors[i] = Some(e.code);
                if e.code == "auth" {
                    self.blocked[i] = true;
                }
                if e.terminal() && e.code != "auth" {
                    next.pending.retain(|p| p.id != job.id);
                } else if let Some(p) = next.pending.iter_mut().find(|p| p.id == job.id) {
                    p.attempts = p.attempts.saturating_add(1);
                    let delay = (30u64.saturating_mul(1u64 << p.attempts.min(7)))
                        .min(3600)
                        .max(e.retry);
                    p.next_attempt = now.saturating_add(delay);
                }
            }
        }
        let _ = self.commit(next);
        self.publish();
    }
}
async fn run(app: AppHandle, view: Arc<Mutex<Status>>, mut rx: mpsc::Receiver<Command>) {
    let path = crate::persistence::sonante_config_dir().join("scrobbling.json");
    let initial = app
        .state::<crate::profile_lease::State>()
        .lease()
        .and_then(|lease| {
            lease.validate()?;
            load(&path)
        });
    let storage_error = initial.is_err();
    let data = initial.unwrap_or_default();
    let mut worker = Worker {
        app,
        view,
        path,
        data,
        tracker: Tracker::default(),
        since: [Instant::now(); 2],
        errors: [None; 2],
        last_sent: [None; 2],
        blocked: [false; 2],
        np: [None, None],
        auth: None,
        storage_error,
    };
    worker.publish();
    let Ok(mut api) = Api::new() else {
        return;
    };
    loop {
        let delay = if worker.storage_error {
            Duration::from_secs(3600)
        } else if worker.np.iter().any(Option::is_some) {
            Duration::from_millis(1)
        } else {
            Duration::from_secs(
                worker
                    .due()
                    .map(|due| due.saturating_sub(utc()).min(3600))
                    .unwrap_or(3600),
            )
        };
        tokio::select! {biased;
            command=rx.recv()=>match command{
                Some(Command::Observe(o))=>worker.observation(o),
                Some(Command::Action(action,reply))=>{let result=worker.action(&mut api,action).await;let _=reply.send(result);},
                None=>break,
            },
            _=tokio::time::sleep(delay)=>worker.deliver(&mut api).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_hex_lastfm_session_survives_private_store_round_trip_without_affecting_listenbrainz() {
        let mut data = File::default();
        data.lastfm = Some(Lastfm {
            enabled: true,
            username: "fixture".into(),
            api_key: "a".repeat(32),
            api_secret: "b".repeat(32),
            session: "g+/&?#=%".repeat(4),
        });
        data.listenbrainz = Some(Listenbrainz {
            enabled: true,
            username: "fixture".into(),
            token: "SYNTHETIC-TOKEN".into(),
        });
        let bytes = serde_json::to_vec(&data).unwrap();
        let restored: File = serde_json::from_slice(&bytes).unwrap();
        assert!(restored.validate());
        assert_eq!(restored.lastfm.unwrap().session, "g+/&?#=%".repeat(4));
        data.lastfm.as_mut().unwrap().session = "a".repeat(31) + "\n";
        assert!(!data.validate());
        data.lastfm.as_mut().unwrap().session = "a".repeat(32);
        data.lastfm.as_mut().unwrap().api_key = "G".repeat(32);
        assert!(!data.validate());
    }
    #[test]
    fn private_store_validates_account_bound_queue_and_never_serializes_secrets_in_status() {
        let mut data = File::default();
        data.listenbrainz = Some(Listenbrainz {
            enabled: true,
            username: "fixture".into(),
            token: "SYNTHETIC-TOKEN".into(),
        });
        data.pending.push(Pending {
            id: 1,
            provider: Provider::Listenbrainz,
            account: "fixture".into(),
            track: Track::new("Artist", "Song", "", 60.).unwrap(),
            started_at: 123,
            next_attempt: 123,
            attempts: 0,
        });
        data.next_id = 2;
        assert!(data.validate());
        data.pending[0].account = "other".into();
        assert!(!data.validate());
        data.pending[0].account = "fixture".into();
        data.pending.push(data.pending[0].clone());
        assert!(!data.validate());
        let view = serde_json::to_string(&Status {
            ready: true,
            ..Status::default()
        })
        .unwrap();
        for s in [
            "SYNTHETIC-TOKEN",
            "api_secret",
            "session",
            "authorization_url",
        ] {
            assert!(!view.contains(s));
        }
    }
    #[test]
    fn private_store_refuses_corruption_symlinks_and_overbroad_permissions_without_writing() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let path =
            std::env::temp_dir().join(format!("sonante-scrobbling-owned-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        let file = path.join("scrobbling.json");
        assert!(load(&file).unwrap().validate());
        std::fs::write(&file, b"invalid").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"invalid");
        std::fs::remove_file(&file).unwrap();
        let real = path.join("target");
        std::fs::write(&real, serde_json::to_vec(&File::default()).unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &file).unwrap();
        assert!(load(&file).is_err());
        std::fs::remove_file(&file).unwrap();
        std::fs::rename(&real, &file).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&file).is_err());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&file).unwrap().validate());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(path).unwrap();
    }
}

#[cfg(test)]
mod navidrome_tests {
 use super::*;
 use crate::{audio::CurrentMedia,config::EnabledSources};
 #[test]
 fn enabled_source_policy_accepts_navidrome_and_keeps_radio_upnp_excluded(){
  let nav=CurrentMedia::Navidrome{connection_id:"fixture-session".into(),song_id:"song-a".into(),queue_index:0};
  for mask in 0..8 {let sources=EnabledSources{local:mask&1!=0,plex:mask&2!=0,navidrome:mask&4!=0};assert_eq!(media_key(&nav,sources).is_some(),sources.navidrome);
   assert!(media_key(&CurrentMedia::Radio{station_id:"radio".into(),queue_index:0},sources).is_none());assert!(media_key(&CurrentMedia::Upnp{session_id:"remote".into(),queue_index:0},sources).is_none());}
 }
 #[test]
 fn navidrome_uses_the_existing_listening_threshold_once_per_provider_and_sends_no_source_references(){
  use model::{Tracker,Observation,Target,Track};
  let origin=Instant::now();let targets=[Some(Target{account:"lastfm-fixture".into(),since:origin}),Some(Target{account:"brainz-fixture".into(),since:origin})];
  let media=CurrentMedia::Navidrome{connection_id:"PRIVATE_SESSION".into(),song_id:"PRIVATE_SONG_ID".into(),queue_index:2};
  let key=media_key(&media,EnabledSources{local:true,plex:true,navidrome:true}).unwrap();let mut tracker=Tracker::default();let mut sent=Vec::new();let mut np=0;
  for second in 0..=70 {
   let (now,pending)=tracker.observe(Observation{at:origin+Duration::from_secs(second),utc:1_000+second,key:Some(key.clone()),playing:true,stopped:false,gap:false,elapsed:second as f64,track:Track::new("Artist","Track","Album",120.)},&targets);
   if second<60 {assert!(pending.is_empty());}np+=now.len();sent.extend(pending);
  }
  assert_eq!(np,2);assert_eq!(sent.len(),2);assert_eq!(sent[0].track.title,"Track");
  let payload=serde_json::to_string(&sent).unwrap();for private in ["PRIVATE_SESSION","PRIVATE_SONG_ID","connection_id","song_id","127.0.0.1"]{assert!(!payload.contains(private));}
 }
}
