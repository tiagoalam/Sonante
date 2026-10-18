//! Shared-only DSP: an owned MPD PCM pipe, never a system capture or Direct route.
mod binary;
mod continuity;
pub(crate) mod comparison;
mod io;
mod peq;
mod runtime;
pub(crate) mod safety;
use crate::equalizer::{validate_preset, EqPreset};
use io::{Environment, Node};
use runtime::{
    CamillaRuntimeController, LocalWebSocketTransport, RuntimeEndpoint, RuntimeHealth,
    RuntimeShared,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::PathBuf,
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        mpsc::{self, Sender, SyncSender},
        Arc, Mutex, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Error {
    Busy,
    RuntimeMissing,
    RuntimeUntrusted,
    RuntimeStartFailed,
    RuntimeExited,
    ControlFailed,
    ApplyFailed,
    RollbackFailed,
    PipeWireUnavailable,
    SinkUnavailable,
    NodeMissing,
    GraphInvalid,
    Cancelled,
    CleanupFailed,
    ContinuityLost,
    ComparisonStale,
    ComparisonInterrupted,
    ComparisonUnavailable,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EQ_{self:?}")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Off,
    Preparing,
    Ready,
    Active,
    Updating,
    Stopping,
    Error,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Snapshot {
    pub comparison: Option<comparison::Comparison>,
    pub protection: Option<safety::Protection>,
    pub transition: Option<String>,
    pub in_path: Option<bool>,
    pub revision: u64,
    pub phase: Phase,
    pub applied: Option<EqPreset>,
    pub error: Option<Error>,
    pub pcm_rate: u32,
    pub pcm_channels: u8,
    pub verified: bool,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            comparison: None,
            protection: None,
            transition: None,
            in_path: None,
            revision: 0,
            phase: Phase::Off,
            applied: None,
            error: None,
            pcm_rate: 48000,
            pcm_channels: 2,
            verified: false,
        }
    }
}
#[derive(Clone)]
pub(crate) struct Prepared {
    pub pipe: PathBuf,
    pub generation: u64,
}
type Reply<T> = Sender<Result<T, Error>>;
enum Command {
    Prepare(EqPreset, Reply<Prepared>),
    Commit(Reply<()>),
    Apply(EqPreset, Reply<()>),
    Compare(comparison::Action,Option<String>,Option<EqPreset>,Option<crate::audio::EqTransitionWatch>,Reply<Snapshot>),
    Stop(Reply<()>),
    Shutdown,
}
struct Actor {
    sender: SyncSender<Command>,
    worker: JoinHandle<()>,
}
pub(crate) struct State {
    actor: Mutex<Option<Actor>>,
    snapshot: Arc<Mutex<Snapshot>>,
    generation: Arc<AtomicU64>,
    transport: Arc<AtomicU8>,
    shutdown: Arc<AtomicBool>,
    start_gate: Arc<RwLock<()>>,
}
impl State {
    pub fn new() -> Self {
        Self {
            actor: Mutex::new(None),
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            generation: Arc::new(AtomicU64::new(0)),
            transport: Arc::new(AtomicU8::new(0)),
            shutdown: Arc::new(AtomicBool::new(false)),
            start_gate: Arc::new(RwLock::new(())),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or(Snapshot {
            phase: Phase::Error,
            error: Some(Error::ControlFailed),
            ..Snapshot::default()
        })
    }
    pub fn record_transition(&self, value: &str) {
        let mut s = self.snapshot();
        s.transition = Some(value.into());
        set(&self.snapshot, s);
    }
    pub fn record_failure(&self, error: Error) {
        let mut current = self.snapshot();
        current.error = Some(error);
        if current.applied.is_none() {
            current.phase = Phase::Off;
            current.verified = false;
        }
        set(&self.snapshot, current);
    }
    pub fn verify_prepared(&self, prepared: &Prepared) -> bool {
        self.generation.load(Ordering::Acquire) == prepared.generation
            && !self.shutdown.load(Ordering::Acquire)
    }
    pub fn observe_playing(&self, playing: bool) {
        self.transport.store(u8::from(playing), Ordering::Release);
    }
    fn sender(&self, failure: Arc<dyn Fn() + Send + Sync>) -> Result<SyncSender<Command>, Error> {
        let mut actor = self.actor.lock().map_err(|_| Error::ControlFailed)?;
        if self.shutdown.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if let Some(a) = &*actor {
            return Ok(a.sender.clone());
        }
        let (tx, rx) = mpsc::sync_channel(4);
        let status = self.snapshot.clone();
        let generation = self.generation.clone();
        let transport = self.transport.clone();
        let shutdown = self.shutdown.clone();
        let start_gate = self.start_gate.clone();
        let worker = thread::Builder::new()
            .name("sonante-shared-eq".into())
            .spawn(move || {
                let mut session: Option<Session> = None;
                let mut committed = false;
                let mut next_health = Instant::now();
                while !shutdown.load(Ordering::Acquire) {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(Command::Prepare(preset, reply)) => {
                            if session.is_some() {
                                let _ = reply.send(Err(Error::Busy));
                                continue;
                            }
                            set(
                                &status,
                                Snapshot {
                                    phase: Phase::Preparing,
                                    ..Snapshot::default()
                                },
                            );
                            match Session::start(
                                preset.clone(),
                                generation.clone(),
                                shutdown.clone(),
                                start_gate.clone(),
                                Environment::default(),
                            ) {
                                Ok(s) => {
                                    let prepared = Prepared {
                                        pipe: s.pipe.clone(),
                                        generation: s.id,
                                    };
                                    set(
                                        &status,
                                        Snapshot {
                                            phase: Phase::Ready,
                                            applied: None,
                                            verified: false,
                                            ..Snapshot::default()
                                        },
                                    );
                                    session = Some(s);
                                    committed = false;
                                    let _ = reply.send(Ok(prepared));
                                }
                                Err(error) => {
                                    set(
                                        &status,
                                        Snapshot {
                                            phase: Phase::Error,
                                            error: Some(error),
                                            ..Snapshot::default()
                                        },
                                    );
                                    let _ = reply.send(Err(error));
                                }
                            }
                        }
                        Ok(Command::Commit(reply)) => {
                            let result = session
                                .as_mut()
                                .ok_or(Error::ControlFailed)
                                .and_then(|s| s.health().map(|_| ()));
                            committed = result.is_ok();
                            if committed {
                                set(
                                    &status,
                                    Snapshot {
                                        phase: Phase::Ready,
                                        protection: session
                                            .as_ref()
                                            .and_then(|s| safety::assess(&s.preset, 48000).ok()),
                                        applied: session.as_ref().map(|s| s.preset.clone()),
                                        verified: true,
                                        ..Snapshot::default()
                                    },
                                );
                            }
                            let _ = reply.send(result);
                        }
                        Ok(Command::Apply(preset, reply)) => {
                            let result = if let Some(s) = session.as_mut() {
                                let old = snapshot_of(&status);
                                set(
                                    &status,
                                    Snapshot {
                                        phase: Phase::Updating,
                                        ..old.clone()
                                    },
                                );
                                match s.controller.apply_preset(s.id, &preset, pcm()) {
                                    Ok(()) => {
                                        s.preset = preset.clone();
                                        s.reference=false;
                                        s.comparison=None;
                                        set(
                                            &status,
                                            Snapshot {
                                                phase: if transport.load(Ordering::Acquire) != 0 {
                                                    Phase::Active
                                                } else {
                                                    Phase::Ready
                                                },
                                                protection: safety::assess(&preset, 48000).ok(),
                                                applied: Some(preset),
                                                error: None,
                                                verified: true,
                                                ..Snapshot::default()
                                            },
                                        );
                                        Ok(())
                                    }
                                    Err(_) => {
                                        if s.controller.health() == RuntimeHealth::Poisoned {
                                            set(
                                                &status,
                                                Snapshot {
                                                    phase: Phase::Error,
                                                    error: Some(Error::RollbackFailed),
                                                    verified: false,
                                                    ..old
                                                },
                                            );
                                            Err(Error::RollbackFailed)
                                        } else {
                                            set(
                                                &status,
                                                Snapshot {
                                                    error: Some(Error::ApplyFailed),
                                                    ..old
                                                },
                                            );
                                            Err(Error::ApplyFailed)
                                        }
                                    }
                                }
                            } else {
                                Err(Error::ControlFailed)
                            };
                            if result == Err(Error::RollbackFailed) {
                                stop_owned(&mut session);
                                committed = false;
                                failure();
                            }
                            let _ = reply.send(result);
                        }
                        Ok(Command::Compare(action,token,expected,watch,reply)) => {
                            let old=snapshot_of(&status);
                            if session.is_some(){set(&status,Snapshot{phase:Phase::Updating,verified:false,..old.clone()});}
                            let result=if let Some(s)=session.as_mut(){s.compare(&old,action,token,expected,watch)}else{Err(Error::ComparisonUnavailable)};
                            match result {
                                Ok(mut value)=>{value.phase=if transport.load(Ordering::Acquire)!=0{Phase::Active}else{Phase::Ready};set(&status,value);let _=reply.send(Ok(snapshot_of(&status)));}
                                Err(error)=>{
                                    if !matches!(error,Error::ComparisonStale|Error::ComparisonInterrupted|Error::ComparisonUnavailable|Error::ApplyFailed|Error::Cancelled|Error::Busy) {
                                        stop_owned(&mut session);committed=false;
                                        set(&status,Snapshot{phase:Phase::Error,error:Some(error),..Snapshot::default()});failure();
                                    }else{
                                        let mut previous=old;
                                        if error==Error::Cancelled {previous.verified=false;previous.phase=Phase::Stopping;}
                                        previous.comparison=session.as_ref().and_then(|s|s.comparison_view());
                                        set(&status,previous);
                                    }
                                    let _=reply.send(Err(error));
                                }
                            }
                        }
                        Ok(Command::Stop(reply)) => {
                            committed = false;
                            set(
                                &status,
                                Snapshot {
                                    phase: Phase::Stopping,
                                    ..snapshot_of(&status)
                                },
                            );
                            let result = session.as_mut().map_or(Ok(()), |s| s.stop());
                            let process_gone = session.as_ref().is_none_or(|s| s.child.is_none());
                            if process_gone {
                                session.take();
                            }
                            set(
                                &status,
                                Snapshot {
                                    phase: if process_gone {
                                        Phase::Off
                                    } else {
                                        Phase::Error
                                    },
                                    error: result.err(),
                                    ..Snapshot::default()
                                },
                            );
                            // Artifact cleanup uncertainty is visible, but does not
                            // pretend the confirmed-dead processor is still on.
                            let _ = reply.send(if process_gone { Ok(()) } else { result });
                        }
                        Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if committed && generation.load(Ordering::Acquire) != 0 {
                        if let Some(s) = session.as_mut() {
                            let result = if Instant::now() >= next_health {
                                next_health = Instant::now() + Duration::from_secs(2);
                                s.health()
                            } else {
                                s.live()
                                    .map(|_| snapshot_of(&status).phase == Phase::Active)
                            };
                            match result {
                                Ok(active) => {
                                    let mut current = snapshot_of(&status);
                                    current.comparison=s.comparison_view();
                                    if current.phase != Phase::Updating {
                                        current.phase =
                                            if active && transport.load(Ordering::Acquire) != 0 {
                                                Phase::Active
                                            } else {
                                                Phase::Ready
                                            };
                                        current.verified = true;
                                        set(&status, current);
                                    }
                                }
                                Err(error) => {
                                    stop_owned(&mut session);
                                    committed = false;
                                    set(
                                        &status,
                                        Snapshot {
                                            phase: Phase::Error,
                                            error: Some(error),
                                            ..Snapshot::default()
                                        },
                                    );
                                    failure();
                                }
                            }
                        }
                    }
                }
                stop_owned(&mut session);
                set(&status, Snapshot::default());
            })
            .map_err(|_| Error::RuntimeStartFailed)?;
        *actor = Some(Actor {
            sender: tx.clone(),
            worker,
        });
        Ok(tx)
    }
    fn request<T>(
        &self,
        build: impl FnOnce(Reply<T>) -> Command,
        failure: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<T, Error> {
        let tx = self.sender(failure)?;
        let (reply, rx) = mpsc::channel();
        tx.try_send(build(reply)).map_err(|_| Error::Busy)?;
        rx.recv_timeout(Duration::from_secs(12)).map_err(|_| {
            self.generation.store(0, Ordering::Release);
            let (discard, _) = mpsc::channel();
            let _ = tx.try_send(Command::Stop(discard));
            Error::ControlFailed
        })?
    }
    pub fn prepare(
        &self,
        preset: EqPreset,
        failure: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Prepared, Error> {
        validate_preset(&preset).map_err(|_| Error::ApplyFailed)?;
        self.request(|r| Command::Prepare(preset, r), failure)
    }
    pub fn commit(&self) -> Result<(), Error> {
        self.request(Command::Commit, Arc::new(|| {}))
    }
    pub fn apply(&self, preset: EqPreset) -> Result<(), Error> {
        validate_preset(&preset).map_err(|_| Error::ApplyFailed)?;
        self.request(|r| Command::Apply(preset, r), Arc::new(|| {}))
    }
    pub fn compare(&self,action:comparison::Action,token:Option<String>,expected:Option<EqPreset>,watch:Option<crate::audio::EqTransitionWatch>)->Result<Snapshot,Error>{
        if self.shutdown.load(Ordering::Acquire){return Err(Error::Cancelled)}
        // Cleanup must not create an actor with an empty failure callback before
        // the first real Prepare installs its lifecycle callback.
        if self.actor.lock().map_err(|_|Error::ControlFailed)?.is_none(){return Err(Error::ComparisonUnavailable)}
        self.request(|r|Command::Compare(action,token,expected,watch,r),Arc::new(||{}))
    }
    pub fn stop(&self) -> Result<(), Error> {
        self.generation.store(0, Ordering::Release);
        if self
            .actor
            .lock()
            .map_err(|_| Error::ControlFailed)?
            .is_none()
        {
            return Ok(());
        }
        self.request(Command::Stop, Arc::new(|| {}))
    }
    pub fn begin_shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.generation.store(0, Ordering::Release);
        let _spawn_fence = self
            .start_gate
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(a) = self.actor.lock() {
            if let Some(a) = &*a {
                let _ = a.sender.try_send(Command::Shutdown);
            }
        }
    }
    pub fn join(&self) {
        let actor = self.actor.lock().ok().and_then(|mut a| a.take());
        if let Some(a) = actor {
            let _ = a.worker.join();
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.begin_shutdown();
        self.join();
    }
}
// Keep the owned handle if termination cannot be confirmed. A later Stop can retry.
fn stop_owned(session: &mut Option<Session>) {
    if let Some(s) = session.as_mut() {
        let _ = s.stop();
    }
    if session.as_ref().is_some_and(|s| s.child.is_none()) {
        session.take();
    }
}
fn set(status: &Mutex<Snapshot>, mut value: Snapshot) {
    if let Ok(mut s) = status.lock() {
        value.revision = s.revision.saturating_add(1);
        *s = value;
    }
}
fn snapshot_of(status: &Mutex<Snapshot>) -> Snapshot {
    status.lock().map(|s| s.clone()).unwrap_or_default()
}
fn pcm() -> peq::PcmFormat {
    peq::PcmFormat {
        sample_rate_hz: 48000,
        channels: 2,
    }
}

pub(crate) fn binary_path() -> Result<PathBuf, Error> {
    if !cfg!(debug_assertions) {
        return Err(Error::RuntimeMissing);
    }
    let path = std::env::var_os("SONANTE_EQ_CAMILLA_PATH")
        .map(PathBuf::from)
        .ok_or(Error::RuntimeMissing)?;
    let m = fs::symlink_metadata(&path).map_err(|_| Error::RuntimeMissing)?;
    if !path.is_absolute()
        || !m.is_file()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::getuid() }
        || m.mode() & 0o111 == 0
        || m.len() > 128 * 1024 * 1024
    {
        return Err(Error::RuntimeUntrusted);
    }
    let mut file = File::open(&path).map_err(|_| Error::RuntimeUntrusted)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|_| Error::RuntimeUntrusted)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    if format!("{:x}", hash.finalize()) != binary::CAMILLA_SHA256 {
        return Err(Error::RuntimeUntrusted);
    }
    Ok(path)
}
struct Session {
    reference: bool,
    comparison: Option<comparison::Active>,
    id: u64,
    current: Arc<AtomicU64>,
    child: Option<Child>,
    controller: CamillaRuntimeController,
    root: PathBuf,
    marker: continuity::Marker,
    pipe: PathBuf,
    anchor: Option<File>,
    root_identity: (u64, u64),
    pipe_identity: (u64, u64),
    stream: Node,
    sink: Node,
    env: Environment,
    preset: EqPreset,
}
struct SetupFiles {
    root: PathBuf,
    root_id: (u64, u64),
    pipe: Option<(u64, u64)>,
    config: Option<(u64, u64)>,
    armed: bool,
}
impl Drop for SetupFiles {
    fn drop(&mut self) {
        if !self.armed
            || !fs::symlink_metadata(&self.root).is_ok_and(|m| {
                m.is_dir() && !m.file_type().is_symlink() && (m.dev(), m.ino()) == self.root_id
            })
        {
            return;
        }
        for (name, expected) in [("pcm.s32le", self.pipe), ("camilla.json", self.config)] {
            if let Some(identity) = expected {
                let path = self.root.join(name);
                if fs::symlink_metadata(&path).is_ok_and(|m| (m.dev(), m.ino()) == identity) {
                    let _ = fs::remove_file(path);
                }
            }
        }
        let _ = fs::remove_dir(&self.root);
    }
}
static NEXT: AtomicU64 = AtomicU64::new(1);
impl Session {
    fn start(
        preset: EqPreset,
        generation: Arc<AtomicU64>,
        shutdown: Arc<AtomicBool>,
        gate: Arc<RwLock<()>>,
        env: Environment,
    ) -> Result<Self, Error> {
        if shutdown.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let binary = binary_path()?;
        let sink = env.graph()?.default_sink()?;
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        generation.store(id, Ordering::Release);
        let mut random = [0u8; 16];
        File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .map_err(|_| Error::RuntimeStartFailed)?;
        let token = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let parent = crate::supervisor::MpdSupervisor::runtime_dir();
        let root = parent.join(format!("shared-eq-{token}"));
        fs::create_dir_all(&parent).map_err(|_| Error::RuntimeStartFailed)?;
        fs::create_dir(&root).map_err(|_| Error::RuntimeStartFailed)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::RuntimeStartFailed)?;
        let meta = fs::symlink_metadata(&root).map_err(|_| Error::RuntimeStartFailed)?;
        let root_identity = (meta.dev(), meta.ino());
        let mut setup = SetupFiles {
            root: root.clone(),
            root_id: root_identity,
            pipe: None,
            config: None,
            armed: true,
        };
        let marker = continuity::Marker::create(&root)?;
        let pipe = root.join("pcm.s32le");
        let c = std::ffi::CString::new(pipe.as_os_str().as_bytes())
            .map_err(|_| Error::RuntimeStartFailed)?;
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
            return Err(Error::RuntimeStartFailed);
        }
        let created_pipe = fs::symlink_metadata(&pipe).map_err(|_| Error::RuntimeStartFailed)?;
        setup.pipe = Some((created_pipe.dev(), created_pipe.ino()));
        let anchor = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&pipe)
            .map_err(|_| Error::RuntimeStartFailed)?;
        unsafe { libc::fcntl(anchor.as_raw_fd(), libc::F_SETPIPE_SZ, 65536) }; // best effort, this private pipe only
        let m = anchor.metadata().map_err(|_| Error::RuntimeStartFailed)?;
        if !m.file_type().is_fifo() {
            return Err(Error::RuntimeStartFailed);
        }
        let pipe_identity = (m.dev(), m.ino());
        let stream_name = format!("sonante_eq_{token}");
        let endpoint =
            RuntimeEndpoint::reserve(id, pipe.to_string_lossy().into_owned(), stream_name.clone())
                .map_err(|_| Error::RuntimeStartFailed)?;
        let filters = peq::convert_preset(&preset, pcm()).map_err(|_| Error::ApplyFailed)?;
        let config = json!({"title":endpoint.title,"devices":{"samplerate":48000,"chunksize":1024,"target_level":4096,"volume_ramp_time":runtime::VOLUME_RAMP_MS,"capture":{"type":"RawFile","channels":2,"filename":pipe,"format":"S32_LE"},"playback":{"type":"PipeWire","channels":2,"node_name":stream_name,"node_description":"Sonante Equalizer · Shared","node_group_name":format!("sonante_eq_{token}"),"autoconnect_to":null},"enable_rate_adjust":true,"resampler":{"type":"AsyncSinc","profile":"Balanced"}},"filters":filters.filters,"pipeline":filters.pipeline});
        let config_path = root.join("camilla.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config_path)
            .map_err(|_| Error::RuntimeStartFailed)?;
        let config_meta = file.metadata().map_err(|_| Error::RuntimeStartFailed)?;
        setup.config = Some((config_meta.dev(), config_meta.ino()));
        file.write_all(config.to_string().as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::RuntimeStartFailed)?;
        let spawn_fence = gate.read().map_err(|_| Error::Cancelled)?;
        if shutdown.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != id {
            return Err(Error::Cancelled);
        }
        let mut command = env.command(binary.to_str().ok_or(Error::RuntimeUntrusted)?);
        command
            .args(["--address", "127.0.0.1", "--port"])
            .arg(endpoint.port.to_string())
            .arg(&config_path)
            .env("PIPEWIRE_AUTOCONNECT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(test)]
        if let Some(test_root) = std::env::var_os("SONANTE_EQ_TEST_ROOT").map(PathBuf::from) {
            if test_root.parent() == Some(std::path::Path::new("/tmp"))
                && test_root
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("seq-"))
            {
                let _ = fs::copy(&config_path, test_root.join(format!("camilla-{id}.json")));
                if let Ok(log) = File::create(test_root.join(format!("camilla-{id}.log"))) {
                    command.stderr(Stdio::from(log));
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            let parent = std::process::id();
            unsafe {
                command.pre_exec(move || {
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent as libc::pid_t {
                        libc::_exit(125);
                    }
                    Ok(())
                });
            }
        }
        let child = command.spawn().map_err(|_| Error::RuntimeStartFailed)?;
        drop(spawn_fence);
        let controller = CamillaRuntimeController::new(
            endpoint,
            generation.clone(),
            Arc::new(RuntimeShared::default()),
            LocalWebSocketTransport,
        );
        // After spawn, Session Drop owns cleanup on every error path.
        let dummy = Node {
            id: 0,
            serial: 0,
            name: stream_name.clone(),
            class: "Stream/Output/Audio".into(),
            pid: Some(child.id()),
        };
        let mut s = Self {
            reference:false,comparison:None,
            id,
            current: generation,
            child: Some(child),
            controller,
            root,
            marker,
            pipe,
            anchor: Some(anchor),
            root_identity,
            pipe_identity,
            stream: dummy,
            sink,
            env,
            preset,
        };
        setup.armed = false;
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            s.live()?;
            if s.controller.probe_owned().is_ok()
                && s.controller.confirm_initial(&s.preset, pcm()).is_ok()
            {
                if let Ok(g) = s.env.graph() {
                    if let Ok(node) = g.node(&stream_name, "Stream/Output/Audio") {
                        if node.pid != s.child.as_ref().map(Child::id) {
                            return Err(Error::GraphInvalid);
                        }
                        if g.stereo(node, true).is_ok() {
                            s.stream = node.clone();
                            g.connect(&s.env, &s.stream, &s.sink)?;
                            break;
                        }
                        #[cfg(test)]
                        if std::env::var("PIPEWIRE_REMOTE").ok().as_deref()
                            == Some("sonante-eq-isolated")
                            && std::env::var_os("SONANTE_EQ_TEST_ROOT").is_some()
                        {
                            s.env.run("pw-cli",&["set-param",&node.id.to_string(),"PortConfig","{ direction = Output mode = dsp format = { mediaType = audio mediaSubtype = raw format = F32P rate = 48000 channels = 2 position = [ FL FR ] } }"])?;
                        }
                    }
                }
            }
            if Instant::now() >= deadline {
                #[cfg(test)]
                {
                    eprintln!(
                        "private_probe_control={:?} preset={:?}",
                        s.controller.probe_owned(),
                        s.controller.confirm_initial(&s.preset, pcm())
                    );
                    if let Ok(g) = s.env.graph() {
                        eprintln!("private_probe_nodes={:?} ports={}", g.nodes, g.ports.len());
                    }
                }
                return Err(Error::RuntimeStartFailed);
            }
            thread::sleep(Duration::from_millis(40));
        }
        let linked_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            s.live()?;
            if s.health().is_ok() {
                break;
            }
            if Instant::now() >= linked_deadline {
                return Err(Error::GraphInvalid);
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(s)
    }
    fn live(&mut self) -> Result<(), Error> {
        self.marker.verify()?;
        if self.current.load(Ordering::Acquire) != self.id {
            return Err(Error::Cancelled);
        }
        match self.child.as_mut().ok_or(Error::RuntimeExited)?.try_wait() {
            Ok(None) => Ok(()),
            _ => Err(Error::RuntimeExited),
        }
    }
    fn health(&mut self) -> Result<bool, Error> {
        self.live()?;
        self.controller
            .confirm_rendered(&self.preset, pcm(),self.reference)
            .map_err(|_| Error::ControlFailed)?;
        self.env.graph()?.validate(&self.stream, &self.sink)
    }
    fn stop(&mut self) -> Result<(), Error> {
        let mut failed = false;
        if let Some(child) = self.child.as_mut() {
            if child
                .try_wait()
                .map_err(|_| Error::CleanupFailed)?
                .is_none()
            {
                if child.kill().is_err()
                    && child
                        .try_wait()
                        .map_err(|_| Error::CleanupFailed)?
                        .is_none()
                {
                    return Err(Error::CleanupFailed);
                }
            }
            child.wait().map_err(|_| Error::CleanupFailed)?;
        }
        self.child.take();
        self.anchor.take();
        let valid = fs::symlink_metadata(&self.root).is_ok_and(|m| {
            m.is_dir() && !m.file_type().is_symlink() && (m.dev(), m.ino()) == self.root_identity
        });
        if valid {
            match fs::symlink_metadata(&self.pipe) {
                Ok(m) if m.file_type().is_fifo() && (m.dev(), m.ino()) == self.pipe_identity => {
                    failed |= fs::remove_file(&self.pipe).is_err()
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => failed = true,
            }
            let config = self.root.join("camilla.json");
            if config.exists() {
                failed |= fs::remove_file(config).is_err();
            }
            failed |= self.marker.remove().is_err();
            failed |= fs::remove_dir(&self.root).is_err();
        } else {
            failed = true
        }
        if failed {
            Err(Error::CleanupFailed)
        } else {
            Ok(())
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.stop();
        }
    }
}

#[cfg(test)]
mod probe_tests;

#[cfg(test)]
#[test]
fn comparison_cleanup_cannot_initialize_an_actor_or_lose_future_failure_callback() {
 let state=State::new();assert_eq!(state.compare(comparison::Action::Finish,Some("stale".into()),None,None).unwrap_err(),Error::ComparisonUnavailable);assert!(state.actor.lock().unwrap().is_none());state.begin_shutdown();assert_eq!(state.compare(comparison::Action::Finish,Some("stale".into()),None,None).unwrap_err(),Error::Cancelled);assert!(state.actor.lock().unwrap().is_none());
}
