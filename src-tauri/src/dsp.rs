//! Isolated EQ-1A foundation. No startup, MPD or Tauri command uses this module yet.
mod activation;
#[cfg(test)]
mod harness;
mod pipewire;

use pipewire::{
    exact_node, stereo_ports, validate_paused_topology, validate_topology, Direction, DspRoute,
    PipeWireCommandRunner, PipeWireMonitor, PipeWireRouteManager, RouteStatus, RouteViolation,
    SystemCommandRunner,
};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DspError {
    InvalidTransition,
    InvalidIdentifier,
    CamillaBinaryMissing,
    CamillaStartFailed,
    CamillaExited,
    PipeWireToolMissing(&'static str),
    CommandFailed {
        program: &'static str,
        operation: &'static str,
        status: Option<i32>,
        stderr: String,
    },
    SnapshotFailed(String),
    NodeMissing(String),
    NodeAmbiguous(String),
    PortMissing(String),
    LinkCreateFailed,
    LinkRemoveFailed,
    TopologyInvalid(Vec<RouteViolation>),
    MonitorFailed,
    CleanupFailed(String),
    RuntimeFailed(String),
    FirstStreamTimeout,
    MpdNodeUnsafe,
    ActivationCancelled,
    PlaybackSnapshotFailed,
    PlaybackRestoreFailed,
    MpdCommandFailed(&'static str),
    SessionStale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DspState {
    Inactive,
    Starting,
    Active,
    Failed(DspError),
    Stopping,
}

impl DspState {
    fn transition(&mut self, next: DspState) -> Result<(), DspError> {
        let valid = matches!(
            (&*self, &next),
            (Self::Inactive, Self::Starting)
                | (Self::Starting, Self::Active)
                | (Self::Starting, Self::Failed(_))
                | (Self::Active, Self::Failed(_))
                | (Self::Active, Self::Starting)
                | (Self::Starting, Self::Stopping)
                | (Self::Active, Self::Stopping)
                | (Self::Failed(_), Self::Stopping)
                | (Self::Stopping, Self::Inactive)
                | (Self::Stopping, Self::Failed(_))
        );
        if !valid {
            return Err(DspError::InvalidTransition);
        }
        *self = next;
        Ok(())
    }
}

pub fn resolve_camilla_binary(explicit: Option<&Path>) -> Result<PathBuf, DspError> {
    let path = explicit
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("SONANTE_CAMILLADSP_BIN").map(PathBuf::from))
        .ok_or(DspError::CamillaBinaryMissing)?;
    let metadata = fs::metadata(&path).map_err(|_| DspError::CamillaBinaryMissing)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(DspError::CamillaBinaryMissing);
    }
    Ok(path)
}

fn flat_config(capture: &str, playback: &str) -> String {
    format!("devices:\n  samplerate: 48000\n  chunksize: 1024\n  enable_rate_adjust: false\n  resampler: null\n  capture:\n    type: PipeWire\n    channels: 2\n    node_name: {capture}\n    node_group_name: sonante_dsp\n  playback:\n    type: PipeWire\n    channels: 2\n    node_name: {playback}\n    node_group_name: sonante_dsp\npipeline: []\n")
}

fn camilla_command(binary: &Path, config: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg(config)
        .env("PIPEWIRE_AUTOCONNECT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub struct DspSupervisor<R: PipeWireCommandRunner = SystemCommandRunner> {
    state: DspState,
    generation: u64,
    activation_generation: Arc<AtomicU64>,
    activation_claim: Arc<AtomicU64>,
    process: Option<Child>,
    monitor: Option<PipeWireMonitor>,
    route_manager: PipeWireRouteManager<R>,
    route: Option<DspRoute>,
    runtime_dir: Option<PathBuf>,
    runtime_parent: Option<PathBuf>,
    route_status: RouteStatus,
}

impl DspSupervisor<SystemCommandRunner> {
    pub fn system() -> Self {
        Self::new(SystemCommandRunner)
    }
}

impl<R: PipeWireCommandRunner> DspSupervisor<R> {
    pub fn new(runner: R) -> Self {
        Self {
            state: DspState::Inactive,
            generation: 0,
            activation_generation: Arc::new(AtomicU64::new(0)),
            activation_claim: Arc::new(AtomicU64::new(0)),
            process: None,
            monitor: None,
            route_manager: PipeWireRouteManager::new(runner),
            route: None,
            runtime_dir: None,
            runtime_parent: None,
            route_status: RouteStatus::NotReady,
        }
    }

    pub fn state(&self) -> &DspState {
        &self.state
    }
    pub fn route_status(&self) -> &RouteStatus {
        &self.route_status
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn capture_node_name(&self) -> Option<&str> {
        self.route.as_ref().map(|route| route.capture_name.as_str())
    }

    #[cfg(test)]
    fn with_runtime_parent(mut self, parent: PathBuf) -> Self {
        self.runtime_parent = Some(parent);
        self
    }

    pub fn start(&mut self, binary: Option<&Path>) -> Result<(u64, String), DspError> {
        if self.state != DspState::Inactive {
            return Err(DspError::InvalidTransition);
        }
        let binary = resolve_camilla_binary(binary)?;
        self.state.transition(DspState::Starting)?;
        self.generation = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.activation_generation
            .store(self.generation, Ordering::SeqCst);
        self.activation_claim.store(0, Ordering::SeqCst);
        let sink_name = format!(
            "sonante_dsp_null_{}_{}",
            std::process::id(),
            self.generation
        );
        let capture = format!(
            "sonante_dsp_{}_{}_capture",
            std::process::id(),
            self.generation
        );
        let playback = format!(
            "sonante_dsp_{}_{}_playback",
            std::process::id(),
            self.generation
        );
        self.route = Some(DspRoute {
            mpd_name: None,
            capture_name: capture.clone(),
            playback_name: playback.clone(),
            sink_name: sink_name.clone(),
        });
        let result: Result<(u64, String), DspError> = (|| {
            let parent = self
                .runtime_parent
                .clone()
                .unwrap_or_else(crate::supervisor::MpdSupervisor::runtime_dir);
            fs::create_dir_all(&parent)
                .map_err(|_| DspError::RuntimeFailed("create runtime parent".into()))?;
            let runtime = parent.join(format!("dsp-{}-{}", std::process::id(), self.generation));
            fs::create_dir(&runtime)
                .map_err(|_| DspError::RuntimeFailed("create DSP runtime directory".into()))?;
            self.runtime_dir = Some(runtime.clone());
            fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
                .map_err(|_| DspError::RuntimeFailed("protect DSP runtime directory".into()))?;
            let config_path = runtime.join("flat.yml");
            let mut config = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&config_path)
                .map_err(|_| DspError::RuntimeFailed("create DSP config".into()))?;
            config
                .write_all(flat_config(&capture, &playback).as_bytes())
                .map_err(|_| DspError::RuntimeFailed("write DSP config".into()))?;
            self.process = Some(
                camilla_command(&binary, &config_path)
                    .spawn()
                    .map_err(|_| DspError::CamillaStartFailed)?,
            );
            self.monitor = Some(PipeWireMonitor::start(self.generation)?);
            self.wait_for_camilla(&capture, &playback)?;
            self.route_status = RouteStatus::CamillaReady;
            Ok((self.generation, sink_name))
        })();
        if let Err(error) = result {
            let cleanup = self.stop();
            let reported = cleanup.err().unwrap_or(error);
            self.state = DspState::Failed(reported.clone());
            return Err(reported);
        }
        result
    }

    fn wait_for_camilla(&mut self, capture: &str, playback: &str) -> Result<(), DspError> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            self.check_process()?;
            let graph = self.route_manager.snapshot()?;
            let cap = exact_node(&graph, capture, "Stream/Input/Audio");
            let play = exact_node(&graph, playback, "Stream/Output/Audio");
            if let (Ok(cap), Ok(play)) = (&cap, &play) {
                if cap.autoconnect != Some(false) || play.autoconnect != Some(false) {
                    return Err(DspError::TopologyInvalid(vec![
                        RouteViolation::AutoconnectEnabled(capture.into()),
                    ]));
                }
                let ports_ready = stereo_ports(&graph, cap, Direction::Input).is_ok()
                    && stereo_ports(&graph, play, Direction::Output).is_ok();
                if !ports_ready {
                    if Instant::now() >= deadline {
                        return Err(DspError::PortMissing(capture.into()));
                    }
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
                if graph.links.iter().any(|link| {
                    graph.ports.iter().any(|port| {
                        (port.node_id == cap.id && port.id == link.input_port)
                            || (port.node_id == play.id && port.id == link.output_port)
                    })
                }) {
                    return Err(DspError::TopologyInvalid(vec![RouteViolation::ExtraLink(
                        0,
                    )]));
                }
                return Ok(());
            }
            for node in [cap, play] {
                if let Err(error) = node {
                    if !matches!(error, DspError::NodeMissing(_)) {
                        return Err(error);
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(DspError::NodeMissing(capture.into()));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn check_process(&mut self) -> Result<(), DspError> {
        if let Some(process) = self.process.as_mut() {
            if !matches!(process.try_wait(), Ok(None)) {
                self.state = DspState::Failed(DspError::CamillaExited);
                self.route_status =
                    RouteStatus::Invalid(vec![RouteViolation::MissingNode("CamillaDSP".into())]);
                return Err(DspError::CamillaExited);
            }
        }
        Ok(())
    }

    // EQ-1A intentionally accepts only a virtual sink. MPD stream creation belongs to EQ-1B.
    pub fn activate_null_route(&mut self, mpd_name: String) -> Result<RouteStatus, DspError> {
        let status = self.connect_null_route(mpd_name)?;
        if status == RouteStatus::RouteReady {
            self.state.transition(DspState::Active)?;
        }
        Ok(status)
    }

    fn connect_null_route(&mut self, mpd_name: String) -> Result<RouteStatus, DspError> {
        if self.state != DspState::Starting {
            return Err(DspError::InvalidTransition);
        }
        self.check_process()?;
        match self.monitor.as_mut() {
            Some(monitor) => {
                monitor.event_pending()?;
            }
            None => return Err(DspError::MonitorFailed),
        }
        let graph = self.route_manager.snapshot()?;
        let route = self.route.as_mut().ok_or(DspError::InvalidTransition)?;
        route.mpd_name = Some(mpd_name);
        let sink = exact_node(&graph, &route.sink_name, "Audio/Sink")?;
        if !sink.virtual_sink
            || sink.name
                != format!(
                    "sonante_dsp_null_{}_{}",
                    std::process::id(),
                    self.generation
                )
        {
            return Err(DspError::InvalidIdentifier);
        }
        let mpd = match exact_node(
            &graph,
            route.mpd_name.as_deref().unwrap_or_default(),
            "Stream/Output/Audio",
        ) {
            Ok(node) => node,
            Err(DspError::NodeMissing(_)) => {
                self.route_status = RouteStatus::NotReady;
                return Ok(RouteStatus::NotReady);
            }
            Err(error) => return Err(error),
        };
        let capture = exact_node(&graph, &route.capture_name, "Stream/Input/Audio")?;
        let playback = exact_node(&graph, &route.playback_name, "Stream/Output/Audio")?;
        if [mpd, capture, playback]
            .iter()
            .any(|n| n.autoconnect != Some(false))
        {
            return Err(DspError::TopologyInvalid(vec![
                RouteViolation::AutoconnectEnabled("DSP route".into()),
            ]));
        }
        let mpd_ports = stereo_ports(&graph, mpd, Direction::Output)?;
        let cap_ports = stereo_ports(&graph, capture, Direction::Input)?;
        let play_ports = stereo_ports(&graph, playback, Direction::Output)?;
        let sink_ports = stereo_ports(&graph, sink, Direction::Input)?;
        let route_ports: Vec<u32> = mpd_ports
            .iter()
            .chain(play_ports.iter())
            .chain(cap_ports.iter())
            .chain(sink_ports.iter())
            .map(|p| p.id)
            .collect();
        if graph.links.iter().any(|link| {
            route_ports.contains(&link.output_port) || route_ports.contains(&link.input_port)
        }) {
            return Err(DspError::TopologyInvalid(vec![RouteViolation::ExtraLink(
                0,
            )]));
        }
        let result = (|| {
            self.route_manager
                .link_stereo(playback, play_ports, sink, sink_ports)?;
            self.route_manager
                .link_stereo(mpd, mpd_ports, capture, cap_ports)?;
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let current = self.route_manager.snapshot()?;
                match validate_topology(&current, route, true) {
                    RouteStatus::RouteReady => break Ok(RouteStatus::RouteReady),
                    RouteStatus::Invalid(v)
                        if v.iter()
                            .all(|item| matches!(item, RouteViolation::InactiveLink(_)))
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(20));
                    }
                    RouteStatus::Invalid(v) => break Err(DspError::TopologyInvalid(v)),
                    _ => {
                        break Err(DspError::TopologyInvalid(vec![
                            RouteViolation::MissingMpdStream,
                        ]))
                    }
                }
            }
        })();
        match result {
            Ok(status) => {
                self.route_status = status.clone();
                Ok(status)
            }
            Err(error) => {
                let cleanup = self.route_manager.remove_owned_links();
                let reported = cleanup.err().unwrap_or(error);
                self.state = DspState::Failed(reported.clone());
                Err(reported)
            }
        }
    }

    // pw-link monitors topology; link state changes on pause require confirmed MPD state.
    pub fn on_graph_event(
        &mut self,
        session: u64,
        player_state: activation::PlayerState,
    ) -> Result<(), DspError> {
        if session != self.generation || self.state != DspState::Active {
            return Ok(());
        }
        self.check_process()?;
        let graph = match self.route_manager.snapshot() {
            Ok(graph) => graph,
            Err(error) => {
                self.state = DspState::Failed(error.clone());
                return Err(error);
            }
        };
        let route = self.route.as_ref().ok_or(DspError::InvalidTransition)?;
        if player_state == activation::PlayerState::Stopped
            && route.mpd_name.as_deref().is_some_and(|name| {
                matches!(
                    exact_node(&graph, name, "Stream/Output/Audio"),
                    Err(DspError::NodeMissing(_))
                )
            })
        {
            // MPD may remove its stream on stop. The null destination must still be valid.
            let only_missing_mpd = matches!(
                validate_paused_topology(&graph, route),
                RouteStatus::Invalid(ref violations)
                    if !violations.is_empty()
                        && violations.iter().all(|violation| matches!(
                            violation,
                            RouteViolation::MissingNode(name)
                                if Some(name.as_str()) == route.mpd_name.as_deref()
                        ))
            );
            if let (Ok(sink), Ok(_), Ok(_)) = (
                exact_node(&graph, &route.sink_name, "Audio/Sink"),
                exact_node(&graph, &route.capture_name, "Stream/Input/Audio"),
                exact_node(&graph, &route.playback_name, "Stream/Output/Audio"),
            ) {
                if sink.virtual_sink && only_missing_mpd {
                    if let Err(error) = self.route_manager.remove_owned_links() {
                        self.state = DspState::Failed(error.clone());
                        return Err(error);
                    }
                    self.route_status = RouteStatus::CamillaReady;
                    if let Some(route) = self.route.as_mut() {
                        route.mpd_name = None;
                    }
                    self.state.transition(DspState::Starting)?;
                    return Ok(());
                }
            }
        }
        self.route_status = match player_state {
            activation::PlayerState::Playing => validate_topology(&graph, route, true),
            activation::PlayerState::Paused | activation::PlayerState::Stopped => {
                validate_paused_topology(&graph, route)
            }
        };
        if (self.route_manager.owned_links().len() != 4
            || self
                .route_manager
                .owned_links()
                .iter()
                .any(|id| !graph.links.iter().any(|link| link.id == *id)))
            && self.route_status == RouteStatus::RouteReady
        {
            self.route_status = RouteStatus::Invalid(vec![RouteViolation::MissingMpdStream]);
        }
        match &self.route_status {
            RouteStatus::Invalid(violations) => {
                self.state = DspState::Failed(DspError::TopologyInvalid(violations.clone()));
            }
            RouteStatus::NotReady => {
                self.state = DspState::Failed(DspError::TopologyInvalid(vec![
                    RouteViolation::MissingMpdStream,
                ]));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn drain_monitor(&mut self, player_state: activation::PlayerState) -> Result<(), DspError> {
        let pending = match self.monitor.as_mut() {
            Some(monitor) => (
                monitor.session,
                match monitor.event_pending() {
                    Ok(value) => value,
                    Err(error) => {
                        self.state = DspState::Failed(error.clone());
                        return Err(error);
                    }
                },
            ),
            None => return Ok(()),
        };
        if pending.1 {
            self.on_graph_event(pending.0, player_state)?;
        }
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), DspError> {
        if self.state == DspState::Inactive {
            return Ok(());
        }
        self.state.transition(DspState::Stopping)?;
        self.activation_generation.store(0, Ordering::SeqCst);
        self.activation_claim.store(0, Ordering::SeqCst);
        self.generation = NEXT_SESSION.fetch_add(1, Ordering::Relaxed); // Invalidate old events first.
        let mut problems = Vec::new();
        if let Some(monitor) = self.monitor.take() {
            if monitor.stop().is_err() {
                problems.push("monitor");
            }
        }
        if self.route_manager.remove_owned_links().is_err() {
            problems.push("links");
        }
        if let Some(mut process) = self.process.take() {
            match process.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if process.kill().is_err() {
                        problems.push("CamillaDSP kill");
                    }
                }
                Err(_) => problems.push("CamillaDSP status"),
            }
            if process.wait().is_err() {
                problems.push("CamillaDSP wait");
            }
        }
        if let Some(runtime) = self.runtime_dir.take() {
            if let Err(error) = fs::remove_file(runtime.join("flat.yml")) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    problems.push("DSP config");
                }
            }
            if fs::remove_dir(&runtime).is_err() {
                problems.push("DSP runtime directory");
                self.runtime_dir = Some(runtime);
            }
        }
        self.route = None;
        self.route_status = RouteStatus::NotReady;
        if problems.is_empty() {
            self.state.transition(DspState::Inactive)
        } else {
            let error = DspError::CleanupFailed(problems.join(", "));
            self.state.transition(DspState::Failed(error.clone()))?;
            Err(error)
        }
    }
}

impl<R: PipeWireCommandRunner> Drop for DspSupervisor<R> {
    fn drop(&mut self) {
        if self.stop().is_err() {
            eprintln!("[DSP] cleanup failed; owned resources need inspection");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoPipeWire;
    impl PipeWireCommandRunner for NoPipeWire {
        fn run(
            &mut self,
            _: &'static str,
            _: &'static str,
            _: &[String],
        ) -> Result<Vec<u8>, DspError> {
            Err(DspError::SnapshotFailed("fake runner".into()))
        }
    }

    #[test]
    fn state_machine_rejects_invalid_transitions() {
        let mut state = DspState::Inactive;
        assert_eq!(
            state.transition(DspState::Active),
            Err(DspError::InvalidTransition)
        );
        state.transition(DspState::Starting).unwrap();
        assert_eq!(
            state.transition(DspState::Starting),
            Err(DspError::InvalidTransition)
        );
        state.transition(DspState::Active).unwrap();
        state.transition(DspState::Starting).unwrap();
        state.transition(DspState::Active).unwrap();
        state
            .transition(DspState::Failed(DspError::CamillaExited))
            .unwrap();
        state.transition(DspState::Stopping).unwrap();
        state.transition(DspState::Inactive).unwrap();
    }

    #[test]
    fn duplicate_start_and_idempotent_stop() {
        let mut supervisor = DspSupervisor::new(NoPipeWire);
        assert_eq!(supervisor.stop(), Ok(()));
        supervisor.state = DspState::Starting;
        assert_eq!(supervisor.start(None), Err(DspError::InvalidTransition));
        supervisor.stop().unwrap();
        assert_eq!(supervisor.stop(), Ok(()));
    }

    #[test]
    fn explicit_binary_resolution_never_searches_path() {
        assert_eq!(
            resolve_camilla_binary(Some(Path::new("/definitely/missing/camilladsp"))),
            Err(DspError::CamillaBinaryMissing)
        );
    }

    #[test]
    fn flat_config_has_no_processing_and_uses_pipewire() {
        let config = flat_config("capture", "playback");
        assert!(config.contains("pipeline: []"));
        assert_eq!(config.matches("type: PipeWire").count(), 2);
        assert!(config.contains("resampler: null"));
        let command = camilla_command(Path::new("/camilladsp"), Path::new("/flat.yml"));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "PIPEWIRE_AUTOCONNECT"
                && value == Some(std::ffi::OsStr::new("0"))));
    }

    #[test]
    fn unexpected_child_exit_fails_closed() {
        let mut supervisor = DspSupervisor::new(NoPipeWire);
        supervisor.state = DspState::Starting;
        supervisor.process = Some(Command::new("true").spawn().unwrap());
        supervisor.process.as_mut().unwrap().wait().unwrap();
        assert_eq!(supervisor.check_process(), Err(DspError::CamillaExited));
        assert_eq!(
            supervisor.state(),
            &DspState::Failed(DspError::CamillaExited)
        );
        supervisor.stop().unwrap();
    }

    #[test]
    fn stale_session_event_cannot_invalidate_new_session() {
        let mut supervisor = DspSupervisor::new(NoPipeWire);
        supervisor.state = DspState::Active;
        supervisor.generation = 7;
        supervisor
            .on_graph_event(6, activation::PlayerState::Playing)
            .unwrap();
        assert_eq!(supervisor.state(), &DspState::Active);
        supervisor.stop().unwrap();
    }

    #[test]
    fn active_snapshot_failure_fails_closed() {
        let mut supervisor = DspSupervisor::new(NoPipeWire);
        supervisor.state = DspState::Active;
        supervisor.generation = 8;
        assert_eq!(
            supervisor.on_graph_event(8, activation::PlayerState::Playing),
            Err(DspError::SnapshotFailed("fake runner".into()))
        );
        assert_eq!(
            supervisor.state(),
            &DspState::Failed(DspError::SnapshotFailed("fake runner".into()))
        );
        supervisor.stop().unwrap();
    }
}
