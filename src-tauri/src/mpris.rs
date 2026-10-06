use crate::audio::{AudioEngine, AudioState, CurrentMedia, PlaybackStatus as AudioPlaybackStatus};
use crate::{is_shared_output, ConfigState};
use mpris_server::{
    zbus::{self, fdo},
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Time, TrackId, Volume,
};
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

const BUS_NAME_SUFFIX: &str = "sonante";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BasicMediaAction {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetadataSnapshot {
    track_id: String,
    length_micros: Option<i64>,
    title: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MprisSnapshot {
    playback_status: PlaybackStatus,
    metadata: MetadataSnapshot,
    position_micros: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangedProperty {
    PlaybackStatus,
    Metadata,
}

fn playback_status_from_mpd(state: &str) -> Result<PlaybackStatus, String> {
    match state {
        "play" => Ok(PlaybackStatus::Playing),
        "pause" => Ok(PlaybackStatus::Paused),
        "stop" => Ok(PlaybackStatus::Stopped),
        value => Err(format!("Estado MPD inválido para MPRIS: {value}")),
    }
}

fn seconds_to_micros(seconds: Option<f64>) -> Option<i64> {
    let seconds = seconds?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    let micros = seconds * 1_000_000.0;
    if !micros.is_finite() || micros >= i64::MAX as f64 {
        return None;
    }
    Some(micros.round() as i64)
}

fn stable_identity_hash(parts: &[&str]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for part in parts {
        for byte in part.as_bytes().iter().copied().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn track_id_for(media: &CurrentMedia) -> Result<TrackId, String> {
    let (kind, queue_index, hash) = match media {
        CurrentMedia::Local { uri, queue_index } => {
            ("local", *queue_index, stable_identity_hash(&[uri]))
        }
        CurrentMedia::Plex {
            server_id,
            part_key,
            queue_index,
        } => (
            "plex",
            *queue_index,
            stable_identity_hash(&[server_id, part_key]),
        ),
    };
    TrackId::try_from(format!(
        "/com/sonante/player/track/{kind}_{queue_index}_{hash:016x}"
    ))
    .map_err(|error| format!("Falha ao gerar TrackId MPRIS: {error}"))
}

impl MetadataSnapshot {
    fn no_track() -> Self {
        Self {
            track_id: TrackId::NO_TRACK.to_string(),
            length_micros: None,
            title: None,
            artists: Vec::new(),
            album: None,
        }
    }

    fn to_metadata(&self) -> Result<Metadata, String> {
        let track_id = TrackId::try_from(self.track_id.clone())
            .map_err(|error| format!("TrackId MPRIS inválido: {error}"))?;
        let mut metadata = Metadata::new();
        metadata.set_trackid(Some(track_id));
        metadata.set_length(self.length_micros.map(Time::from_micros));
        metadata.set_title(self.title.clone());
        metadata.set_artist((!self.artists.is_empty()).then(|| self.artists.clone()));
        metadata.set_album(self.album.clone());
        Ok(metadata)
    }
}

impl MprisSnapshot {
    fn stopped() -> Self {
        Self {
            playback_status: PlaybackStatus::Stopped,
            metadata: MetadataSnapshot::no_track(),
            position_micros: 0,
        }
    }

    fn from_audio(status: AudioPlaybackStatus) -> Result<Self, String> {
        let metadata = match status.current_media.as_ref() {
            Some(media) => MetadataSnapshot {
                track_id: track_id_for(media)?.to_string(),
                length_micros: seconds_to_micros(Some(status.duration))
                    .filter(|micros| *micros > 0),
                title: (!status.title.is_empty()).then_some(status.title),
                artists: (!status.artist.is_empty())
                    .then_some(vec![status.artist])
                    .unwrap_or_default(),
                album: (!status.album.is_empty()).then_some(status.album),
            },
            None => MetadataSnapshot::no_track(),
        };
        Ok(Self {
            playback_status: playback_status_from_mpd(&status.state)?,
            metadata,
            position_micros: seconds_to_micros(Some(status.elapsed)).unwrap_or(0),
        })
    }
}

fn changed_properties(
    previous: Option<&MprisSnapshot>,
    current: &MprisSnapshot,
) -> Vec<ChangedProperty> {
    let mut changed = Vec::new();
    if previous.is_none_or(|snapshot| snapshot.playback_status != current.playback_status) {
        changed.push(ChangedProperty::PlaybackStatus);
    }
    if previous.is_none_or(|snapshot| snapshot.metadata != current.metadata) {
        changed.push(ChangedProperty::Metadata);
    }
    changed
}

trait BasicMediaControls {
    fn play(&mut self) -> Result<(), String>;
    fn pause(&mut self) -> Result<(), String>;
    fn play_pause(&mut self) -> Result<(), String>;
    fn next(&mut self, is_shared: bool) -> Result<(), String>;
    fn previous(&mut self, is_shared: bool) -> Result<(), String>;
}

impl BasicMediaControls for AudioEngine {
    fn play(&mut self) -> Result<(), String> {
        AudioEngine::play(self)
    }

    fn pause(&mut self) -> Result<(), String> {
        AudioEngine::pause(self)
    }

    fn play_pause(&mut self) -> Result<(), String> {
        self.toggle_play_pause()
    }

    fn next(&mut self, is_shared: bool) -> Result<(), String> {
        AudioEngine::next(self, is_shared)
    }

    fn previous(&mut self, is_shared: bool) -> Result<(), String> {
        AudioEngine::previous(self, is_shared)
    }
}

fn apply_basic_action(
    controls: &mut impl BasicMediaControls,
    action: BasicMediaAction,
    is_shared: bool,
) -> Result<(), String> {
    match action {
        BasicMediaAction::Play => controls.play(),
        BasicMediaAction::Pause => controls.pause(),
        BasicMediaAction::PlayPause => controls.play_pause(),
        BasicMediaAction::Next => controls.next(is_shared),
        BasicMediaAction::Previous => controls.previous(is_shared),
    }
}

fn query_audio_snapshot(app: &AppHandle) -> Result<MprisSnapshot, String> {
    let state = app
        .try_state::<AudioState>()
        .ok_or_else(|| "Estado de áudio indisponível para o MPRIS.".to_string())?;
    let audio = state
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar o estado de áudio via MPRIS: {error}"))?;
    MprisSnapshot::from_audio(audio.get_mpris_status()?)
}

async fn query_audio_snapshot_async(app: AppHandle) -> Result<MprisSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || query_audio_snapshot(&app))
        .await
        .map_err(|error| format!("Falha na tarefa MPRIS: {error}"))?
}

async fn publish_snapshot(app: AppHandle, snapshot: MprisSnapshot) -> Result<(), String> {
    let (server, changed) = {
        let state = app
            .try_state::<MprisState>()
            .ok_or_else(|| "Estado do serviço MPRIS indisponível.".to_string())?;
        let mut lifecycle = state
            .0
            .lock()
            .map_err(|error| format!("Falha ao acessar o lifecycle MPRIS: {error}"))?;
        if lifecycle.phase != LifecyclePhase::Running {
            return Ok(());
        }
        let changed = changed_properties(lifecycle.snapshot.as_ref(), &snapshot);
        lifecycle.snapshot = Some(snapshot.clone());
        (lifecycle.server.clone(), changed)
    };

    let Some(server) = server else {
        return Ok(());
    };
    let mut properties = Vec::new();
    for property in changed {
        match property {
            ChangedProperty::PlaybackStatus => {
                properties.push(Property::PlaybackStatus(snapshot.playback_status.clone()));
            }
            ChangedProperty::Metadata => {
                properties.push(Property::Metadata(snapshot.metadata.to_metadata()?));
            }
        }
    }
    if properties.is_empty() {
        return Ok(());
    }
    server
        .properties_changed(properties)
        .await
        .map_err(|error| format!("Falha ao emitir PropertiesChanged MPRIS: {error}"))
}

fn refresh_properties_blocking(app: AppHandle) -> Result<(), String> {
    let snapshot = query_audio_snapshot(&app)?;
    tauri::async_runtime::block_on(publish_snapshot(app, snapshot))
}

#[derive(Debug)]
struct SonanteMpris {
    app: AppHandle,
}

impl SonanteMpris {
    async fn control(&self, action: BasicMediaAction) -> fdo::Result<()> {
        let app = self.app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let is_shared =
                if matches!(action, BasicMediaAction::Next | BasicMediaAction::Previous) {
                    let config_state = app.try_state::<ConfigState>().ok_or_else(|| {
                        "Configuração indisponível para o controle MPRIS.".to_string()
                    })?;
                    let config = config_state.0.lock().map_err(|error| {
                        format!("Falha ao acessar configuração via MPRIS: {error}")
                    })?;
                    is_shared_output(&config)
                } else {
                    false
                };
            let state = app
                .try_state::<AudioState>()
                .ok_or_else(|| "Estado de áudio indisponível para o controle MPRIS.".to_string())?;
            let mut audio = state.0.lock().map_err(|error| {
                format!("Falha ao acessar o estado de áudio via MPRIS: {error}")
            })?;
            apply_basic_action(&mut *audio, action, is_shared)
        })
        .await
        .map_err(|error| fdo::Error::Failed(format!("Falha na tarefa MPRIS: {error}")))?
        .map_err(fdo::Error::Failed)?;

        let refresh_app = self.app.clone();
        let refresh =
            tauri::async_runtime::spawn_blocking(move || refresh_properties_blocking(refresh_app))
                .await
                .map_err(|error| fdo::Error::Failed(format!("Falha na tarefa MPRIS: {error}")))?;
        if let Err(error) = refresh {
            eprintln!("[MPRIS] Controle aceito, mas atualização de propriedades falhou: {error}");
        }
        Ok(())
    }

    async fn current_snapshot(&self) -> MprisSnapshot {
        match query_audio_snapshot_async(self.app.clone()).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!("[MPRIS] Estado MPD temporariamente indisponível: {error}");
                MprisSnapshot::stopped()
            }
        }
    }
}

impl RootInterface for SonanteMpris {
    async fn raise(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Raise não é suportado.".into()))
    }

    async fn quit(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Quit não é suportado.".into()))
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _fullscreen: bool) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("Sonante".into())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok(String::new())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for SonanteMpris {
    async fn next(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Next).await
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Previous).await
    }

    async fn pause(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Pause).await
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::PlayPause).await
    }

    async fn stop(&self) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Stop não é suportado.".into()))
    }

    async fn play(&self) -> fdo::Result<()> {
        self.control(BasicMediaAction::Play).await
    }

    async fn seek(&self, _offset: Time) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("Seek não é suportado.".into()))
    }

    async fn set_position(&self, _track_id: TrackId, _position: Time) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "SetPosition não é suportado.".into(),
        ))
    }

    async fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported("OpenUri não é suportado.".into()))
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(self.current_snapshot().await.playback_status)
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(LoopStatus::None)
    }

    async fn set_loop_status(&self, _loop_status: LoopStatus) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _rate: PlaybackRate) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_shuffle(&self, _shuffle: bool) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        self.current_snapshot()
            .await
            .metadata
            .to_metadata()
            .map_err(fdo::Error::Failed)
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(1.0)
    }

    async fn set_volume(&self, _volume: Volume) -> zbus::Result<()> {
        Err(zbus::Error::Unsupported)
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(Time::from_micros(
            self.current_snapshot().await.position_micros,
        ))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(true)
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

#[derive(Debug)]
struct ObserverHandle {
    control: Arc<ObserverControl>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ObserverHandle {
    fn start(app: AppHandle, socket_path: String) -> Result<Self, String> {
        let control = Arc::new(ObserverControl::new());
        let thread_control = Arc::clone(&control);
        let thread = thread::Builder::new()
            .name("sonante-mpris-mpd".into())
            .spawn(move || {
                if let Err(error) = run_mpd_observer(&app, &socket_path, &thread_control) {
                    if !thread_control.is_cancelled() {
                        eprintln!("[MPRIS] Observer MPD encerrado: {error}");
                    }
                }
            })
            .map_err(|error| format!("Falha ao iniciar thread do observer MPD: {error}"))?;
        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    fn stop(mut self) {
        self.control.cancel();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                eprintln!("[MPRIS] Observer MPD terminou com panic.");
            }
        }
    }
}

#[derive(Debug)]
struct ObserverControl {
    cancelled: AtomicBool,
    socket: Mutex<Option<UnixStream>>,
    wait_lock: Mutex<()>,
    wait_wakeup: Condvar,
}

impl ObserverControl {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            socket: Mutex::new(None),
            wait_lock: Mutex::new(()),
            wait_wakeup: Condvar::new(),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn install_socket(&self, stream: &UnixStream) -> Result<bool, String> {
        let mut socket = self
            .socket
            .lock()
            .map_err(|error| format!("Falha ao guardar socket do observer: {error}"))?;
        if self.is_cancelled() {
            return Ok(false);
        }
        *socket = Some(
            stream
                .try_clone()
                .map_err(|error| format!("Falha ao guardar cleanup do observer: {error}"))?,
        );
        Ok(true)
    }

    fn clear_socket(&self) -> Result<(), String> {
        self.socket
            .lock()
            .map_err(|error| format!("Falha ao liberar socket do observer: {error}"))?
            .take();
        Ok(())
    }

    fn wait_for_retry(&self, duration: Duration) -> Result<bool, String> {
        let guard = self
            .wait_lock
            .lock()
            .map_err(|error| format!("Falha ao preparar backoff do observer: {error}"))?;
        if self.is_cancelled() {
            return Ok(false);
        }
        let _guard = self
            .wait_wakeup
            .wait_timeout_while(guard, duration, |_| !self.is_cancelled())
            .map_err(|error| format!("Falha durante backoff do observer: {error}"))?;
        Ok(!self.is_cancelled())
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        match self.socket.lock() {
            Ok(mut socket) => {
                if let Some(stream) = socket.take() {
                    let _ = stream.shutdown(Shutdown::Both);
                }
            }
            Err(error) => eprintln!("[MPRIS] Falha ao cancelar socket do observer: {error}"),
        }
        self.wait_wakeup.notify_all();
    }
}

#[derive(Debug)]
struct ReconnectBackoff {
    initial: Duration,
    maximum: Duration,
    next: Duration,
}

impl ReconnectBackoff {
    fn new(initial: Duration, maximum: Duration) -> Self {
        Self {
            initial,
            maximum,
            next: initial,
        }
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.maximum);
        delay
    }

    fn reset(&mut self) {
        self.next = self.initial;
    }
}

fn read_mpd_line(reader: &mut impl BufRead, context: &str) -> Result<String, String> {
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .map_err(|error| format!("Falha ao {context}: {error}"))?;
    if bytes == 0 {
        return Err(format!("EOF ao {context}."));
    }
    if !line.ends_with('\n') {
        return Err(format!("Resposta parcial ao {context}."));
    }
    Ok(line)
}

fn read_idle_player_response(reader: &mut impl BufRead) -> Result<(), String> {
    let mut changed = false;
    loop {
        let line = read_mpd_line(reader, "aguardar evento player do MPD")?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line == "changed: player" {
            changed = true;
        } else if line == "OK" {
            return changed
                .then_some(())
                .ok_or_else(|| "Resposta idle do MPD sem evento player.".to_string());
        } else if line.starts_with("ACK ") {
            return Err(format!("MPD recusou idle player: {line}"));
        } else {
            return Err(format!("Resposta idle inesperada do MPD: {line}"));
        }
    }
}

fn arm_idle_player(stream: &mut UnixStream) -> Result<(), String> {
    stream
        .write_all(b"idle player\n")
        .map_err(|error| format!("Falha ao iniciar idle player: {error}"))?;
    stream
        .flush()
        .map_err(|error| format!("Falha ao enviar idle player: {error}"))
}

trait PlayerObserverConnection {
    fn wait_for_player_change(&mut self) -> Result<(), String>;
}

trait PlayerObserverRuntime {
    type Connection: PlayerObserverConnection;

    fn connect(&mut self, control: &ObserverControl) -> Result<Self::Connection, String>;
    fn refresh_snapshot(&mut self) -> Result<(), String>;
}

struct MpdObserverConnection {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl PlayerObserverConnection for MpdObserverConnection {
    fn wait_for_player_change(&mut self) -> Result<(), String> {
        read_idle_player_response(&mut self.reader)?;
        arm_idle_player(&mut self.stream)
    }
}

struct AppObserverRuntime<'a> {
    app: &'a AppHandle,
    socket_path: &'a str,
}

impl PlayerObserverRuntime for AppObserverRuntime<'_> {
    type Connection = MpdObserverConnection;

    fn connect(&mut self, control: &ObserverControl) -> Result<Self::Connection, String> {
        let mut stream = UnixStream::connect(self.socket_path)
            .map_err(|error| format!("Falha ao conectar observer ao MPD: {error}"))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|error| format!("Falha ao configurar handshake do observer: {error}"))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .map_err(|error| format!("Falha ao configurar escrita do observer: {error}"))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|error| format!("Falha ao preparar observer MPD: {error}"))?,
        );
        if !control.install_socket(&stream)? {
            return Err("Observer MPD cancelado durante conexão.".to_string());
        }

        let greeting = read_mpd_line(&mut reader, "ler handshake do observer MPD")?;
        if !greeting.starts_with("OK MPD ") {
            return Err("Handshake inválido no observer MPD.".to_string());
        }
        reader
            .get_ref()
            .set_read_timeout(None)
            .map_err(|error| format!("Falha ao preparar espera idle do MPD: {error}"))?;
        arm_idle_player(&mut stream)?;
        Ok(MpdObserverConnection { stream, reader })
    }

    fn refresh_snapshot(&mut self) -> Result<(), String> {
        refresh_properties_blocking(self.app.clone())
    }
}

fn run_observer_state_machine<R, W>(
    runtime: &mut R,
    control: &ObserverControl,
    mut wait_for_retry: W,
) -> Result<(), String>
where
    R: PlayerObserverRuntime,
    W: FnMut(&ObserverControl, Duration) -> Result<bool, String>,
{
    let mut backoff = ReconnectBackoff::new(Duration::from_millis(250), Duration::from_secs(5));
    let mut connection_lost_logged = false;

    loop {
        if control.is_cancelled() {
            return Ok(());
        }

        let session_result = match runtime.connect(control) {
            Ok(mut connection) => {
                if connection_lost_logged {
                    eprintln!("[MPRIS] Conexão do observer MPD recuperada.");
                    connection_lost_logged = false;
                }
                backoff.reset();
                if control.is_cancelled() {
                    Ok(())
                } else {
                    if let Err(error) = runtime.refresh_snapshot() {
                        eprintln!("[MPRIS] Snapshot do observer indisponível: {error}");
                    }
                    loop {
                        match connection.wait_for_player_change() {
                            Ok(()) if control.is_cancelled() => break Ok(()),
                            Ok(()) => {
                                if let Err(error) = runtime.refresh_snapshot() {
                                    eprintln!("[MPRIS] Falha ao atualizar evento player: {error}");
                                }
                            }
                            Err(error) => break Err(error),
                        }
                    }
                }
            }
            Err(error) => Err(error),
        };

        control.clear_socket()?;
        if control.is_cancelled() {
            return Ok(());
        }

        if let Err(error) = session_result {
            if !connection_lost_logged {
                eprintln!("[MPRIS] Conexão do observer MPD indisponível: {error}");
                connection_lost_logged = true;
            }
        }

        if !wait_for_retry(control, backoff.next_delay())? {
            return Ok(());
        }
    }
}

fn run_mpd_observer(
    app: &AppHandle,
    socket_path: &str,
    control: &ObserverControl,
) -> Result<(), String> {
    let mut runtime = AppObserverRuntime { app, socket_path };
    run_observer_state_machine(&mut runtime, control, |control, delay| {
        control.wait_for_retry(delay)
    })
}

type SonanteMprisServer = Server<SonanteMpris>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecyclePhase {
    NotStarted,
    Starting,
    Running,
    Failed,
    Stopping,
}

#[derive(Debug)]
struct ServiceLifecycle {
    phase: LifecyclePhase,
    server: Option<Arc<SonanteMprisServer>>,
    startup_task: Option<tauri::async_runtime::JoinHandle<()>>,
    observer: Option<ObserverHandle>,
    snapshot: Option<MprisSnapshot>,
}

impl ServiceLifecycle {
    fn new() -> Self {
        Self {
            phase: LifecyclePhase::NotStarted,
            server: None,
            startup_task: None,
            observer: None,
            snapshot: None,
        }
    }

    fn begin_start(&mut self) -> bool {
        if self.phase != LifecyclePhase::NotStarted {
            return false;
        }
        self.phase = LifecyclePhase::Starting;
        true
    }

    fn finish_start(&mut self, server: Arc<SonanteMprisServer>) -> Option<Arc<SonanteMprisServer>> {
        if self.phase == LifecyclePhase::Starting {
            self.startup_task = None;
            self.server = Some(server);
            self.phase = LifecyclePhase::Running;
            None
        } else {
            Some(server)
        }
    }

    fn start_failed(&mut self) {
        if self.phase == LifecyclePhase::Starting {
            self.startup_task = None;
            self.phase = LifecyclePhase::Failed;
        }
    }

    fn set_startup_task(&mut self, task: tauri::async_runtime::JoinHandle<()>) {
        if self.phase == LifecyclePhase::Starting {
            self.startup_task = Some(task);
        } else {
            task.abort();
        }
    }

    fn set_observer(&mut self, observer: ObserverHandle) -> Option<ObserverHandle> {
        if self.phase == LifecyclePhase::Running {
            self.observer = Some(observer);
            None
        } else {
            Some(observer)
        }
    }

    fn begin_shutdown(
        &mut self,
    ) -> (
        Option<Arc<SonanteMprisServer>>,
        Option<tauri::async_runtime::JoinHandle<()>>,
        Option<ObserverHandle>,
    ) {
        self.phase = LifecyclePhase::Stopping;
        self.snapshot = None;
        (
            self.server.take(),
            self.startup_task.take(),
            self.observer.take(),
        )
    }
}

#[derive(Debug)]
pub struct MprisState(Mutex<ServiceLifecycle>);

impl Default for MprisState {
    fn default() -> Self {
        Self(Mutex::new(ServiceLifecycle::new()))
    }
}

pub fn start(app: AppHandle) {
    let Some(state) = app.try_state::<MprisState>() else {
        eprintln!("[MPRIS] Estado do serviço indisponível.");
        return;
    };
    let should_start = match state.0.lock() {
        Ok(mut lifecycle) => lifecycle.begin_start(),
        Err(_) => {
            eprintln!("[MPRIS] Falha ao acessar o lifecycle do serviço.");
            false
        }
    };
    if !should_start {
        return;
    }

    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let server = match Server::new(
            BUS_NAME_SUFFIX,
            SonanteMpris {
                app: task_app.clone(),
            },
        )
        .await
        {
            Ok(server) => server,
            Err(error) => {
                eprintln!("[MPRIS] Session D-Bus indisponível: {error}");
                if let Some(state) = task_app.try_state::<MprisState>() {
                    if let Ok(mut lifecycle) = state.0.lock() {
                        lifecycle.start_failed();
                    }
                }
                return;
            }
        };

        let server = Arc::new(server);
        let unused_server = task_app
            .try_state::<MprisState>()
            .and_then(|state| state.0.lock().ok()?.finish_start(server));
        if let Some(server) = unused_server {
            if let Err(error) = server.release_bus_name().await {
                eprintln!("[MPRIS] Falha ao liberar o nome D-Bus: {error}");
            }
            return;
        }

        let socket_path = task_app.try_state::<AudioState>().and_then(|state| {
            state
                .0
                .lock()
                .ok()
                .map(|audio| audio.local_album_socket_path())
        });
        let Some(socket_path) = socket_path else {
            eprintln!("[MPRIS] Observer MPD não pôde obter o socket de áudio.");
            return;
        };
        let observer = match ObserverHandle::start(task_app.clone(), socket_path) {
            Ok(observer) => observer,
            Err(error) => {
                eprintln!("[MPRIS] Observer MPD indisponível: {error}");
                return;
            }
        };
        let unused_observer = match task_app.try_state::<MprisState>() {
            Some(state) => match state.0.lock() {
                Ok(mut lifecycle) => lifecycle.set_observer(observer),
                Err(_) => {
                    eprintln!("[MPRIS] Falha ao guardar o observer MPD.");
                    Some(observer)
                }
            },
            None => Some(observer),
        };
        if let Some(observer) = unused_observer {
            observer.stop();
        }
    });
    if let Some(state) = app.try_state::<MprisState>() {
        match state.0.lock() {
            Ok(mut lifecycle) => lifecycle.set_startup_task(task),
            Err(_) => {
                task.abort();
                eprintln!("[MPRIS] Falha ao guardar a tarefa de inicialização.");
            }
        }
    } else {
        task.abort();
    }
}

pub fn shutdown(app: &AppHandle) {
    let (server, startup_task, observer) = match app.try_state::<MprisState>() {
        Some(state) => match state.0.lock() {
            Ok(mut lifecycle) => lifecycle.begin_shutdown(),
            Err(_) => {
                eprintln!("[MPRIS] Falha ao acessar o lifecycle no shutdown.");
                (None, None, None)
            }
        },
        None => (None, None, None),
    };

    if let Some(task) = startup_task {
        task.abort();
    }
    if let Some(observer) = observer {
        observer.stop();
    }

    if let Some(server) = server {
        if let Err(error) = tauri::async_runtime::block_on(server.release_bus_name()) {
            eprintln!("[MPRIS] Falha ao encerrar o serviço: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{VolumeBackend, VolumeStatus};
    use std::collections::VecDeque;
    use std::io::{Cursor, Read};
    use std::sync::{mpsc, Barrier};
    use std::time::Instant;

    #[derive(Default)]
    struct FakeControls {
        actions: Vec<BasicMediaAction>,
        shared_transport: Vec<(BasicMediaAction, bool)>,
        failure: Option<String>,
    }

    impl FakeControls {
        fn record(&mut self, action: BasicMediaAction) -> Result<(), String> {
            self.actions.push(action);
            match &self.failure {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    impl BasicMediaControls for FakeControls {
        fn play(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::Play)
        }

        fn pause(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::Pause)
        }

        fn play_pause(&mut self) -> Result<(), String> {
            self.record(BasicMediaAction::PlayPause)
        }

        fn next(&mut self, is_shared: bool) -> Result<(), String> {
            self.shared_transport
                .push((BasicMediaAction::Next, is_shared));
            self.record(BasicMediaAction::Next)
        }

        fn previous(&mut self, is_shared: bool) -> Result<(), String> {
            self.shared_transport
                .push((BasicMediaAction::Previous, is_shared));
            self.record(BasicMediaAction::Previous)
        }
    }

    #[test]
    fn basic_actions_dispatch_to_the_existing_audio_controls() {
        let mut controls = FakeControls::default();
        let actions = [
            BasicMediaAction::Play,
            BasicMediaAction::Pause,
            BasicMediaAction::PlayPause,
            BasicMediaAction::Next,
            BasicMediaAction::Previous,
        ];

        for action in actions {
            apply_basic_action(&mut controls, action, true).expect("ação simulada deve funcionar");
        }

        assert_eq!(controls.actions, actions);
        assert_eq!(
            controls.shared_transport,
            vec![
                (BasicMediaAction::Next, true),
                (BasicMediaAction::Previous, true),
            ]
        );
    }

    #[test]
    fn basic_action_errors_are_not_reported_as_success() {
        let mut controls = FakeControls {
            actions: Vec::new(),
            shared_transport: Vec::new(),
            failure: Some("ACK simulado".into()),
        };

        assert_eq!(
            apply_basic_action(&mut controls, BasicMediaAction::Next, true),
            Err("ACK simulado".into())
        );
    }

    fn audio_status(state: &str, current_media: Option<CurrentMedia>) -> AudioPlaybackStatus {
        AudioPlaybackStatus {
            state: state.into(),
            elapsed: 12.5,
            duration: 245.25,
            audio_format: String::new(),
            current_media,
            title: "Faixa".into(),
            artist: "Artista".into(),
            album: "Álbum".into(),
            thumb: None,
            plex_image: None,
            volume: VolumeStatus {
                value: 0,
                muted: false,
                writable: false,
                available: false,
                backend: VolumeBackend::Unavailable,
            },
            is_updating: false,
        }
    }

    #[test]
    fn playback_states_map_to_mpris() {
        assert_eq!(
            playback_status_from_mpd("play"),
            Ok(PlaybackStatus::Playing)
        );
        assert_eq!(
            playback_status_from_mpd("pause"),
            Ok(PlaybackStatus::Paused)
        );
        assert_eq!(
            playback_status_from_mpd("stop"),
            Ok(PlaybackStatus::Stopped)
        );
        assert!(playback_status_from_mpd("unknown").is_err());
    }

    #[test]
    fn metadata_exposes_only_supported_non_empty_fields() {
        let snapshot = MprisSnapshot::from_audio(audio_status(
            "play",
            Some(CurrentMedia::Local {
                uri: "Música/Faixa.flac".into(),
                queue_index: 3,
            }),
        ))
        .expect("snapshot válido");
        let metadata = snapshot.metadata.to_metadata().expect("metadata válida");

        assert_eq!(metadata.title(), Some("Faixa"));
        assert_eq!(metadata.artist(), Some(vec!["Artista".to_string()]));
        assert_eq!(metadata.album(), Some("Álbum"));
        assert_eq!(metadata.length(), Some(Time::from_micros(245_250_000)));
        assert!(metadata.art_url().is_none());
    }

    #[test]
    fn empty_metadata_fields_are_omitted() {
        let mut status = audio_status(
            "pause",
            Some(CurrentMedia::Local {
                uri: "faixa.flac".into(),
                queue_index: 0,
            }),
        );
        status.title.clear();
        status.artist.clear();
        status.album.clear();
        let metadata = MprisSnapshot::from_audio(status)
            .expect("snapshot válido")
            .metadata
            .to_metadata()
            .expect("metadata válida");

        assert_eq!(metadata.title(), None);
        assert_eq!(metadata.artist(), None);
        assert_eq!(metadata.album(), None);
    }

    #[test]
    fn duration_conversion_is_checked() {
        assert_eq!(seconds_to_micros(Some(1.25)), Some(1_250_000));
        assert_eq!(seconds_to_micros(Some(0.0)), Some(0));
        assert_eq!(seconds_to_micros(None), None);
        assert_eq!(seconds_to_micros(Some(f64::NAN)), None);
        assert_eq!(seconds_to_micros(Some(f64::INFINITY)), None);
        assert_eq!(seconds_to_micros(Some(-0.1)), None);
        assert_eq!(seconds_to_micros(Some(i64::MAX as f64)), None);
    }

    #[test]
    fn local_track_id_is_valid_and_does_not_leak_source() {
        let source = "/home/user/Música/Faixa.flac?X-Plex-Token=segredo";
        let track_id = track_id_for(&CurrentMedia::Local {
            uri: source.into(),
            queue_index: 7,
        })
        .expect("TrackId local válido");

        assert!(track_id
            .as_str()
            .starts_with("/com/sonante/player/track/local_7_"));
        assert!(!track_id.as_str().contains("home"));
        assert!(!track_id.as_str().contains("Faixa"));
        assert!(!track_id.as_str().contains("X-Plex-Token"));
        assert!(!track_id.as_str().contains("segredo"));
    }

    #[test]
    fn plex_track_id_is_valid_and_does_not_leak_identity() {
        let track_id = track_id_for(&CurrentMedia::Plex {
            server_id: "server-secret".into(),
            part_key: "/library/parts/42/file.flac?X-Plex-Token=segredo".into(),
            queue_index: 2,
        })
        .expect("TrackId Plex válido");

        assert!(track_id
            .as_str()
            .starts_with("/com/sonante/player/track/plex_2_"));
        for forbidden in ["server-secret", "library", "X-Plex-Token", "segredo"] {
            assert!(!track_id.as_str().contains(forbidden));
        }
    }

    #[test]
    fn no_current_track_uses_standard_no_track_id() {
        let snapshot =
            MprisSnapshot::from_audio(audio_status("stop", None)).expect("snapshot parado válido");
        let metadata = snapshot.metadata.to_metadata().expect("metadata válida");

        assert_eq!(metadata.trackid(), Some(TrackId::NO_TRACK));
        assert_eq!(metadata.length(), None);
    }

    #[test]
    fn snapshot_changes_include_only_status_or_metadata_differences() {
        let playing = MprisSnapshot::from_audio(audio_status(
            "play",
            Some(CurrentMedia::Local {
                uri: "a.flac".into(),
                queue_index: 0,
            }),
        ))
        .expect("snapshot válido");
        let mut paused = playing.clone();
        paused.playback_status = PlaybackStatus::Paused;
        assert_eq!(
            changed_properties(Some(&playing), &paused),
            vec![ChangedProperty::PlaybackStatus]
        );

        let next = MprisSnapshot::from_audio(audio_status(
            "play",
            Some(CurrentMedia::Plex {
                server_id: "server".into(),
                part_key: "/part/2".into(),
                queue_index: 1,
            }),
        ))
        .expect("snapshot válido");
        assert_eq!(
            changed_properties(Some(&playing), &next),
            vec![ChangedProperty::Metadata]
        );

        let mut progressed = playing.clone();
        progressed.position_micros += 1_000_000;
        assert!(changed_properties(Some(&playing), &progressed).is_empty());
        assert!(changed_properties(Some(&playing), &playing).is_empty());
        assert_eq!(
            changed_properties(None, &playing),
            vec![ChangedProperty::PlaybackStatus, ChangedProperty::Metadata]
        );
    }

    #[test]
    fn idle_parser_accepts_player_event_and_rejects_ack_or_partial_data() {
        assert_eq!(
            read_idle_player_response(&mut Cursor::new(b"changed: player\nOK\n")),
            Ok(())
        );
        assert!(
            read_idle_player_response(&mut Cursor::new(b"ACK [5@0] {idle} unknown command\n"))
                .is_err()
        );
        assert!(read_idle_player_response(&mut Cursor::new(b"changed: player")).is_err());
    }

    enum FakeConnectionAttempt {
        Fail(&'static str),
        Connect {
            id: usize,
            events: VecDeque<Result<(), String>>,
        },
    }

    struct FakeObserverConnection {
        id: usize,
        events: VecDeque<Result<(), String>>,
        waited: Arc<Mutex<Vec<usize>>>,
        dropped: Arc<Mutex<Vec<usize>>>,
    }

    impl PlayerObserverConnection for FakeObserverConnection {
        fn wait_for_player_change(&mut self) -> Result<(), String> {
            self.waited.lock().unwrap().push(self.id);
            self.events
                .pop_front()
                .unwrap_or_else(|| Err("conexão simulada encerrada".into()))
        }
    }

    impl Drop for FakeObserverConnection {
        fn drop(&mut self) {
            self.dropped.lock().unwrap().push(self.id);
        }
    }

    struct FakeObserverRuntime {
        attempts: VecDeque<FakeConnectionAttempt>,
        connect_count: usize,
        refresh_count: usize,
        snapshots: VecDeque<MprisSnapshot>,
        previous_snapshot: Option<MprisSnapshot>,
        published_changes: Vec<Vec<ChangedProperty>>,
        waited: Arc<Mutex<Vec<usize>>>,
        dropped: Arc<Mutex<Vec<usize>>>,
    }

    impl FakeObserverRuntime {
        fn new(attempts: Vec<FakeConnectionAttempt>) -> Self {
            Self {
                attempts: attempts.into(),
                connect_count: 0,
                refresh_count: 0,
                snapshots: VecDeque::new(),
                previous_snapshot: None,
                published_changes: Vec::new(),
                waited: Arc::new(Mutex::new(Vec::new())),
                dropped: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl PlayerObserverRuntime for FakeObserverRuntime {
        type Connection = FakeObserverConnection;

        fn connect(&mut self, _control: &ObserverControl) -> Result<Self::Connection, String> {
            self.connect_count += 1;
            match self
                .attempts
                .pop_front()
                .unwrap_or(FakeConnectionAttempt::Fail("script esgotado"))
            {
                FakeConnectionAttempt::Fail(error) => Err(error.into()),
                FakeConnectionAttempt::Connect { id, events } => Ok(FakeObserverConnection {
                    id,
                    events,
                    waited: Arc::clone(&self.waited),
                    dropped: Arc::clone(&self.dropped),
                }),
            }
        }

        fn refresh_snapshot(&mut self) -> Result<(), String> {
            self.refresh_count += 1;
            if let Some(snapshot) = self.snapshots.pop_front() {
                let changed = changed_properties(self.previous_snapshot.as_ref(), &snapshot);
                self.previous_snapshot = Some(snapshot);
                self.published_changes.push(changed);
            }
            Ok(())
        }
    }

    fn disconnected_connection(id: usize) -> FakeConnectionAttempt {
        FakeConnectionAttempt::Connect {
            id,
            events: VecDeque::from([Err("EOF simulado".into())]),
        }
    }

    fn run_fake_until_wait_stops(
        runtime: &mut FakeObserverRuntime,
        delays: &mut Vec<Duration>,
        waits_before_stop: usize,
    ) {
        let control = ObserverControl::new();
        let mut waits = 0;
        run_observer_state_machine(runtime, &control, |_, delay| {
            delays.push(delay);
            waits += 1;
            Ok(waits < waits_before_stop)
        })
        .expect("state machine simulada deve encerrar");
    }

    #[test]
    fn initial_connection_failure_keeps_observer_alive_until_reconnect() {
        let mut runtime = FakeObserverRuntime::new(vec![
            FakeConnectionAttempt::Fail("socket ausente"),
            disconnected_connection(1),
        ]);
        let mut delays = Vec::new();

        run_fake_until_wait_stops(&mut runtime, &mut delays, 2);

        assert_eq!(runtime.connect_count, 2);
        assert_eq!(runtime.refresh_count, 1);
        assert_eq!(delays, [Duration::from_millis(250); 2]);
    }

    #[test]
    fn disconnected_session_is_dropped_and_replaced_by_a_new_connection() {
        let mut runtime =
            FakeObserverRuntime::new(vec![disconnected_connection(1), disconnected_connection(2)]);
        let waited = Arc::clone(&runtime.waited);
        let dropped = Arc::clone(&runtime.dropped);
        let mut delays = Vec::new();

        run_fake_until_wait_stops(&mut runtime, &mut delays, 2);

        assert_eq!(*waited.lock().unwrap(), vec![1, 2]);
        assert_eq!(*dropped.lock().unwrap(), vec![1, 2]);
        assert_eq!(runtime.refresh_count, 2);
    }

    #[test]
    fn reconnect_backoff_grows_to_limit_and_resets_after_success() {
        let mut backoff = ReconnectBackoff::new(Duration::from_millis(250), Duration::from_secs(5));
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
        assert_eq!(backoff.next_delay(), Duration::from_secs(4));
        assert_eq!(backoff.next_delay(), Duration::from_secs(5));
        assert_eq!(backoff.next_delay(), Duration::from_secs(5));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));

        let mut runtime = FakeObserverRuntime::new(vec![
            FakeConnectionAttempt::Fail("primeira falha"),
            FakeConnectionAttempt::Fail("segunda falha"),
            disconnected_connection(1),
        ]);
        let mut delays = Vec::new();
        run_fake_until_wait_stops(&mut runtime, &mut delays, 3);
        assert_eq!(
            delays,
            [
                Duration::from_millis(250),
                Duration::from_millis(500),
                Duration::from_millis(250),
            ]
        );
    }

    #[test]
    fn reconnect_refreshes_once_and_publishes_only_snapshot_changes() {
        let playing = MprisSnapshot::from_audio(audio_status(
            "play",
            Some(CurrentMedia::Local {
                uri: "a.flac".into(),
                queue_index: 0,
            }),
        ))
        .unwrap();
        let mut paused = playing.clone();
        paused.playback_status = PlaybackStatus::Paused;
        let mut runtime = FakeObserverRuntime::new(vec![
            disconnected_connection(1),
            disconnected_connection(2),
            disconnected_connection(3),
        ]);
        runtime.previous_snapshot = Some(playing.clone());
        runtime.snapshots = VecDeque::from([playing, paused]);
        let mut delays = Vec::new();

        run_fake_until_wait_stops(&mut runtime, &mut delays, 3);

        assert_eq!(runtime.refresh_count, 3);
        assert_eq!(runtime.published_changes[0], Vec::<ChangedProperty>::new());
        assert_eq!(
            runtime.published_changes[1],
            vec![ChangedProperty::PlaybackStatus]
        );
        assert_eq!(runtime.published_changes.len(), 2);
    }

    #[test]
    fn cancellation_interrupts_backoff_immediately() {
        let control = Arc::new(ObserverControl::new());
        let thread_control = Arc::clone(&control);
        let (ready_tx, ready_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            thread_control.wait_for_retry(Duration::from_secs(30))
        });
        ready_rx.recv().unwrap();
        let started = Instant::now();

        control.cancel();

        assert!(!waiter.join().unwrap().unwrap());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn cancellation_unblocks_idle_socket() {
        let control = Arc::new(ObserverControl::new());
        let (mut observed, _peer) = UnixStream::pair().unwrap();
        control.install_socket(&observed).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let reader_barrier = Arc::clone(&barrier);
        let reader = thread::spawn(move || {
            reader_barrier.wait();
            let mut byte = [0_u8; 1];
            observed.read(&mut byte)
        });
        barrier.wait();
        let started = Instant::now();

        control.cancel();

        assert!(reader.join().unwrap().is_ok());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn shutdown_during_reconnect_does_not_leave_thread_pending() {
        struct AlwaysUnavailable {
            attempted: mpsc::Sender<()>,
        }

        impl PlayerObserverRuntime for AlwaysUnavailable {
            type Connection = FakeObserverConnection;

            fn connect(&mut self, _control: &ObserverControl) -> Result<Self::Connection, String> {
                self.attempted.send(()).unwrap();
                Err("MPD indisponível".into())
            }

            fn refresh_snapshot(&mut self) -> Result<(), String> {
                unreachable!()
            }
        }

        let control = Arc::new(ObserverControl::new());
        let thread_control = Arc::clone(&control);
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let observer = thread::spawn(move || {
            let mut runtime = AlwaysUnavailable {
                attempted: attempted_tx,
            };
            run_observer_state_machine(&mut runtime, &thread_control, |control, delay| {
                control.wait_for_retry(delay)
            })
        });
        attempted_rx.recv().unwrap();
        let started = Instant::now();

        control.cancel();

        observer.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn lifecycle_starts_only_once() {
        let mut lifecycle = ServiceLifecycle::new();

        assert!(lifecycle.begin_start());
        assert!(!lifecycle.begin_start());
    }

    #[test]
    fn shutdown_while_starting_prevents_late_installation() {
        let mut lifecycle = ServiceLifecycle::new();

        assert!(lifecycle.begin_start());
        let (server, task, observer) = lifecycle.begin_shutdown();
        assert!(server.is_none());
        assert!(task.is_none());
        assert!(observer.is_none());
        assert_eq!(lifecycle.phase, LifecyclePhase::Stopping);
    }

    #[test]
    fn failed_start_is_not_retried() {
        let mut lifecycle = ServiceLifecycle::new();

        assert!(lifecycle.begin_start());
        lifecycle.start_failed();
        assert_eq!(lifecycle.phase, LifecyclePhase::Failed);
        assert!(!lifecycle.begin_start());
    }
}
