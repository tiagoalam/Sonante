//! EQ-1B first-stream transaction. No production playback path invokes it.
use super::pipewire::{
    exact_node, validate_paused_topology, validate_topology, PipeWireCommandRunner, RouteStatus,
    RouteViolation,
};
use super::{DspError, DspState, DspSupervisor};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

const NODE_TIMEOUT: Duration = Duration::from_secs(2);
const LINK_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_millis(20);
const SEEK_TOLERANCE: f64 = 0.15;

#[derive(Clone)]
pub struct ActivationGate {
    generation: u64,
    current: Arc<AtomicU64>,
    claim: Arc<AtomicU64>,
}

struct ActivationClaim {
    generation: u64,
    claim: Arc<AtomicU64>,
}

impl Drop for ActivationClaim {
    fn drop(&mut self) {
        let _ = self
            .claim
            .compare_exchange(self.generation, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

impl ActivationGate {
    fn reserve(&self) -> Result<ActivationClaim, DspError> {
        self.claim
            .compare_exchange(0, self.generation, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| DspError::InvalidTransition)?;
        Ok(ActivationClaim {
            generation: self.generation,
            claim: Arc::clone(&self.claim),
        })
    }

    fn check(&self, actual: u64) -> Result<(), DspError> {
        if self.current.load(Ordering::SeqCst) == 0 {
            return Err(DspError::ActivationCancelled);
        }
        if self.generation != actual || self.current.load(Ordering::SeqCst) != actual {
            return Err(DspError::SessionStale);
        }
        Ok(())
    }

    pub fn cancel(&self) {
        self.current.store(0, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Stopped,
    Paused,
    Playing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub state: PlayerState,
    pub song_id: Option<u32>,
    pub elapsed: f64,
    pub queue_version: u64,
    pub queue_length: usize,
}

pub trait FirstStreamPlayback {
    fn snapshot(&mut self) -> Result<PlaybackSnapshot, DspError>;
    fn contains_song(&mut self, song_id: u32) -> Result<bool, DspError>;
    fn play_id(&mut self, song_id: u32) -> Result<(), DspError>;
    fn pause(&mut self) -> Result<(), DspError>;
    fn seek_id(&mut self, song_id: u32, elapsed: f64) -> Result<(), DspError>;
    fn resume(&mut self) -> Result<(), DspError>;
    fn stop(&mut self) -> Result<(), DspError>;
}

pub trait FirstStreamRoute {
    fn ready(&self) -> bool;
    fn generation(&self) -> u64;
    fn preflight(&mut self, mpd_name: &str) -> Result<bool, DspError>;
    fn connect_running(&mut self, mpd_name: &str) -> Result<(), DspError>;
    fn validate_paused(&mut self) -> Result<(), DspError>;
    fn validate_running(&mut self) -> Result<(), DspError>;
    fn commit(&mut self) -> Result<(), DspError>;
    fn abort(&mut self, reason: DspError) -> Result<(), DspError>;
}

pub struct DspActivationTransaction {
    pub song_id: u32,
    pub mpd_node_name: String,
    pub gate: ActivationGate,
    node_timeout: Duration,
    link_timeout: Duration,
}

impl DspActivationTransaction {
    pub fn new(song_id: u32, mpd_node_name: String, gate: ActivationGate) -> Self {
        Self {
            song_id,
            mpd_node_name,
            gate,
            node_timeout: NODE_TIMEOUT,
            link_timeout: LINK_TIMEOUT,
        }
    }

    pub fn execute<P: FirstStreamPlayback, R: FirstStreamRoute>(
        &self,
        playback: &mut P,
        route: &mut R,
    ) -> Result<(), DspError> {
        if !route.ready() {
            return Err(DspError::InvalidTransition);
        }
        self.gate.check(route.generation())?;
        let _claim = self.gate.reserve()?;
        self.gate.check(route.generation())?;
        let original = playback
            .snapshot()
            .map_err(|_| DspError::PlaybackSnapshotFailed)?;
        if original.queue_length == 0
            || !playback.contains_song(self.song_id)?
            || !original.elapsed.is_finite()
            || original.elapsed < 0.0
        {
            return Err(DspError::PlaybackSnapshotFailed);
        }
        let elapsed =
            if original.song_id == Some(self.song_id) && original.state != PlayerState::Stopped {
                original.elapsed
            } else {
                0.0
            };
        let result = self.run(playback, route, &original, elapsed);
        if let Err(error) = result {
            let rollback = self.rollback(playback, route, &original, elapsed, error.clone());
            return Err(rollback.err().unwrap_or(error));
        }
        Ok(())
    }

    fn run<P: FirstStreamPlayback, R: FirstStreamRoute>(
        &self,
        playback: &mut P,
        route: &mut R,
        original: &PlaybackSnapshot,
        elapsed: f64,
    ) -> Result<(), DspError> {
        self.gate.check(route.generation())?;
        let node_exists = route.preflight(&self.mpd_node_name)?;
        self.gate.check(route.generation())?;
        match original.state {
            PlayerState::Stopped => playback.play_id(self.song_id)?,
            PlayerState::Paused if original.song_id == Some(self.song_id) => playback.resume()?,
            PlayerState::Playing if original.song_id == Some(self.song_id) => {}
            _ => return Err(DspError::PlaybackSnapshotFailed),
        }
        // Only the selected real queue occurrence is used. No synthetic item is added.
        // MPD was spawned with autoconnect disabled; until the null route is wired,
        // no sample may reach an audible sink.
        let deadline = Instant::now() + self.node_timeout;
        loop {
            self.gate.check(route.generation())?;
            let found = route.preflight(&self.mpd_node_name)?;
            if found {
                break;
            }
            if node_exists || Instant::now() >= deadline {
                return Err(DspError::FirstStreamTimeout);
            }
            thread::sleep(RETRY_DELAY);
        }
        self.verify_player(playback, original, PlayerState::Playing)?;
        self.gate.check(route.generation())?;
        route.connect_running(&self.mpd_node_name)?;
        self.gate.check(route.generation())?;
        if node_exists && original.state == PlayerState::Playing {
            route.validate_running()?;
            self.verify_player(playback, original, PlayerState::Playing)?;
            self.gate.check(route.generation())?;
            route.commit()?;
            return Ok(());
        }
        playback.pause()?;
        self.verify_player(playback, original, PlayerState::Paused)?;
        self.gate.check(route.generation())?;
        playback
            .seek_id(self.song_id, elapsed)
            .map_err(|_| DspError::PlaybackRestoreFailed)?;
        let restored = self.verify_player(playback, original, PlayerState::Paused)?;
        if (restored.elapsed - elapsed).abs() > SEEK_TOLERANCE {
            return Err(DspError::PlaybackRestoreFailed);
        }
        route.validate_paused()?;
        self.gate.check(route.generation())?;
        playback.resume()?;
        self.verify_player(playback, original, PlayerState::Playing)?;
        let deadline = Instant::now() + self.link_timeout;
        loop {
            self.gate.check(route.generation())?;
            match route.validate_running() {
                Ok(()) => break,
                Err(DspError::TopologyInvalid(v))
                    if v.iter()
                        .all(|x| matches!(x, RouteViolation::InactiveLink(_)))
                        && Instant::now() < deadline =>
                {
                    thread::sleep(RETRY_DELAY)
                }
                Err(error) => return Err(error),
            }
        }
        self.gate.check(route.generation())?;
        route.commit()?; // Active is published only after topology and playback confirmation.
        Ok(())
    }

    fn verify_player<P: FirstStreamPlayback>(
        &self,
        playback: &mut P,
        original: &PlaybackSnapshot,
        expected: PlayerState,
    ) -> Result<PlaybackSnapshot, DspError> {
        let now = playback
            .snapshot()
            .map_err(|_| DspError::PlaybackSnapshotFailed)?;
        if now.queue_version != original.queue_version
            || now.queue_length != original.queue_length
            || now.song_id != Some(self.song_id)
            || now.state != expected
        {
            return Err(DspError::PlaybackSnapshotFailed);
        }
        Ok(now)
    }

    fn rollback<P: FirstStreamPlayback, R: FirstStreamRoute>(
        &self,
        playback: &mut P,
        route: &mut R,
        original: &PlaybackSnapshot,
        elapsed: f64,
        reason: DspError,
    ) -> Result<(), DspError> {
        if route.generation() != self.gate.generation {
            return Err(DspError::SessionStale);
        }
        let mut restore_failed = false;
        if playback.pause().is_err() {
            restore_failed = playback.stop().is_err();
        } else if let Ok(now) = playback.snapshot() {
            if now.queue_version == original.queue_version
                && now.song_id == Some(self.song_id)
                && playback.seek_id(self.song_id, elapsed).is_err()
            {
                restore_failed = true;
            }
        } else {
            restore_failed = true;
        }
        let cleanup = route.abort(reason);
        if let Err(error) = cleanup {
            return Err(error);
        }
        if restore_failed {
            Err(DspError::PlaybackRestoreFailed)
        } else {
            Ok(())
        }
    }
}

impl<R: PipeWireCommandRunner> DspSupervisor<R> {
    pub fn activation_gate(&self) -> ActivationGate {
        ActivationGate {
            generation: self.generation,
            current: Arc::clone(&self.activation_generation),
            claim: Arc::clone(&self.activation_claim),
        }
    }
}

impl<R: PipeWireCommandRunner> FirstStreamRoute for DspSupervisor<R> {
    fn ready(&self) -> bool {
        self.state == DspState::Starting
    }
    fn generation(&self) -> u64 {
        self.generation
    }

    fn preflight(&mut self, mpd_name: &str) -> Result<bool, DspError> {
        if self.state != DspState::Starting {
            return Err(DspError::InvalidTransition);
        }
        self.check_process()?;
        self.monitor
            .as_mut()
            .ok_or(DspError::MonitorFailed)?
            .event_pending()?;
        let route = self.route.as_ref().ok_or(DspError::InvalidTransition)?;
        let graph = self.route_manager.snapshot()?;
        let sink = exact_node(&graph, &route.sink_name, "Audio/Sink")?;
        if !sink.virtual_sink
            || sink.name
                != format!(
                    "sonante_dsp_null_{}_{}",
                    std::process::id(),
                    self.generation
                )
        {
            return Err(DspError::MpdNodeUnsafe);
        }
        let capture = exact_node(&graph, &route.capture_name, "Stream/Input/Audio")?;
        let playback = exact_node(&graph, &route.playback_name, "Stream/Output/Audio")?;
        if capture.autoconnect != Some(false) || playback.autoconnect != Some(false) {
            return Err(DspError::MpdNodeUnsafe);
        }
        let mpd = match exact_node(&graph, mpd_name, "Stream/Output/Audio") {
            Ok(node) => node,
            Err(DspError::NodeMissing(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        if mpd.autoconnect != Some(false) {
            return Err(DspError::MpdNodeUnsafe);
        }
        if graph.links.iter().any(|link| {
            graph
                .ports
                .iter()
                .any(|port| port.node_id == mpd.id && port.id == link.output_port)
        }) {
            return Err(DspError::MpdNodeUnsafe);
        }
        Ok(true)
    }

    fn connect_running(&mut self, mpd_name: &str) -> Result<(), DspError> {
        match self.connect_null_route(mpd_name.into())? {
            RouteStatus::RouteReady => Ok(()),
            RouteStatus::Invalid(v) => Err(DspError::TopologyInvalid(v)),
            _ => Err(DspError::FirstStreamTimeout),
        }
    }

    fn validate_paused(&mut self) -> Result<(), DspError> {
        self.check_process()?;
        self.monitor
            .as_mut()
            .ok_or(DspError::MonitorFailed)?
            .event_pending()?;
        let graph = self.route_manager.snapshot()?;
        let route = self.route.as_ref().ok_or(DspError::InvalidTransition)?;
        if self.route_manager.owned_links().len() != 4
            || self
                .route_manager
                .owned_links()
                .iter()
                .any(|id| !graph.links.iter().any(|link| link.id == *id))
        {
            return Err(DspError::TopologyInvalid(vec![
                RouteViolation::MissingMpdStream,
            ]));
        }
        match validate_paused_topology(&graph, route) {
            RouteStatus::RouteReady => Ok(()),
            RouteStatus::Invalid(v) => Err(DspError::TopologyInvalid(v)),
            _ => Err(DspError::FirstStreamTimeout),
        }
    }

    fn validate_running(&mut self) -> Result<(), DspError> {
        self.check_process()?;
        self.monitor
            .as_mut()
            .ok_or(DspError::MonitorFailed)?
            .event_pending()?;
        let graph = self.route_manager.snapshot()?;
        let route = self.route.as_ref().ok_or(DspError::InvalidTransition)?;
        match validate_topology(&graph, route, true) {
            RouteStatus::RouteReady => Ok(()),
            RouteStatus::Invalid(v) => Err(DspError::TopologyInvalid(v)),
            _ => Err(DspError::FirstStreamTimeout),
        }
    }

    fn commit(&mut self) -> Result<(), DspError> {
        self.validate_running()?;
        self.state.transition(DspState::Active)
    }

    fn abort(&mut self, reason: DspError) -> Result<(), DspError> {
        let cleanup = self.stop();
        let reported = cleanup.err().unwrap_or(reason);
        self.state = DspState::Failed(reported.clone());
        if matches!(reported, DspError::CleanupFailed(_)) {
            Err(reported)
        } else {
            Ok(())
        }
    }
}

pub fn dsp_mpd_output_config(name: &str, capture: &str) -> Result<String, DspError> {
    fn escape(value: &str) -> Result<String, DspError> {
        if value.is_empty() || value.contains(['\0', '\r', '\n']) {
            return Err(DspError::InvalidIdentifier);
        }
        Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
    }
    Ok(format!("audio_output {{\n    type \"pipewire\"\n    name \"{}\"\n    target \"{}\"\n    mixer_type \"none\"\n    always_on \"yes\"\n}}",
        escape(name)?, escape(capture)?))
}

pub fn dsp_mpd_command(binary: &Path, config: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("--no-daemon")
        .arg(config)
        .env("PIPEWIRE_AUTOCONNECT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

pub struct MpdSocketControl {
    socket: std::path::PathBuf,
}
impl MpdSocketControl {
    pub fn new(socket: std::path::PathBuf) -> Self {
        Self { socket }
    }

    pub(super) fn command(
        &self,
        operation: &'static str,
        line: &str,
    ) -> Result<Vec<String>, DspError> {
        let mut stream =
            UnixStream::connect(&self.socket).map_err(|_| DspError::MpdCommandFailed(operation))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| DspError::MpdCommandFailed(operation))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|_| DspError::MpdCommandFailed(operation))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|_| DspError::MpdCommandFailed(operation))?,
        );
        let mut greeting = String::new();
        reader
            .read_line(&mut greeting)
            .map_err(|_| DspError::MpdCommandFailed(operation))?;
        if !greeting.starts_with("OK MPD ") {
            return Err(DspError::MpdCommandFailed(operation));
        }
        stream
            .write_all(line.as_bytes())
            .and_then(|_| stream.write_all(b"\n"))
            .map_err(|_| DspError::MpdCommandFailed(operation))?;
        let mut lines = Vec::new();
        loop {
            let mut response = String::new();
            if reader
                .read_line(&mut response)
                .map_err(|_| DspError::MpdCommandFailed(operation))?
                == 0
            {
                return Err(DspError::MpdCommandFailed(operation));
            }
            let response = response.trim_end_matches(['\r', '\n']);
            if response == "OK" {
                return Ok(lines);
            }
            if response.starts_with("ACK ") || lines.len() >= 1024 {
                return Err(DspError::MpdCommandFailed(operation));
            }
            lines.push(response.into());
        }
    }
}

impl FirstStreamPlayback for MpdSocketControl {
    fn snapshot(&mut self) -> Result<PlaybackSnapshot, DspError> {
        let lines = self.command("status", "status")?;
        let mut state = None;
        let mut song_id = None;
        let mut elapsed = 0.0;
        let mut queue_version = None;
        let mut queue_length = None;
        for line in lines {
            let Some((key, value)) = line.split_once(": ") else {
                continue;
            };
            match key {
                "state" => {
                    state = match value {
                        "stop" => Some(PlayerState::Stopped),
                        "pause" => Some(PlayerState::Paused),
                        "play" => Some(PlayerState::Playing),
                        _ => None,
                    }
                }
                "songid" => song_id = value.parse().ok(),
                "elapsed" => {
                    elapsed = value
                        .parse()
                        .map_err(|_| DspError::PlaybackSnapshotFailed)?
                }
                "playlist" => queue_version = value.parse().ok(),
                "playlistlength" => queue_length = value.parse().ok(),
                _ => {}
            }
        }
        Ok(PlaybackSnapshot {
            state: state.ok_or(DspError::PlaybackSnapshotFailed)?,
            song_id,
            elapsed,
            queue_version: queue_version.ok_or(DspError::PlaybackSnapshotFailed)?,
            queue_length: queue_length.ok_or(DspError::PlaybackSnapshotFailed)?,
        })
    }

    fn contains_song(&mut self, song_id: u32) -> Result<bool, DspError> {
        let lines = self.command("playlistid", &format!("playlistid {song_id}"))?;
        Ok(lines.iter().any(|line| line == &format!("Id: {song_id}")))
    }
    fn play_id(&mut self, song_id: u32) -> Result<(), DspError> {
        self.command("playid", &format!("playid {song_id}"))?;
        Ok(())
    }
    fn pause(&mut self) -> Result<(), DspError> {
        self.command("pause", "pause 1")?;
        Ok(())
    }
    fn seek_id(&mut self, song_id: u32, elapsed: f64) -> Result<(), DspError> {
        if !elapsed.is_finite() || elapsed < 0.0 {
            return Err(DspError::PlaybackRestoreFailed);
        }
        self.command("seekid", &format!("seekid {song_id} {elapsed:.3}"))?;
        Ok(())
    }
    fn resume(&mut self) -> Result<(), DspError> {
        self.command("resume", "pause 0")?;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), DspError> {
        self.command("stop", "stop")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakePlayback {
        snapshot: PlaybackSnapshot,
        queue: Vec<u32>,
        calls: Vec<&'static str>,
        fail: Option<&'static str>,
        cancel_on_play: Option<ActivationGate>,
        change_queue_on_play: bool,
    }

    impl FakePlayback {
        fn stopped() -> Self {
            Self {
                snapshot: PlaybackSnapshot {
                    state: PlayerState::Stopped,
                    song_id: None,
                    elapsed: 0.0,
                    queue_version: 4,
                    queue_length: 1,
                },
                queue: vec![7],
                calls: Vec::new(),
                fail: None,
                cancel_on_play: None,
                change_queue_on_play: false,
            }
        }
    }

    impl FirstStreamPlayback for FakePlayback {
        fn snapshot(&mut self) -> Result<PlaybackSnapshot, DspError> {
            self.calls.push("snapshot");
            if self.fail == Some("snapshot") {
                return Err(DspError::PlaybackSnapshotFailed);
            }
            Ok(self.snapshot.clone())
        }
        fn contains_song(&mut self, id: u32) -> Result<bool, DspError> {
            Ok(self.queue.contains(&id))
        }
        fn play_id(&mut self, id: u32) -> Result<(), DspError> {
            self.calls.push("playid");
            if let Some(gate) = &self.cancel_on_play {
                gate.cancel();
            }
            self.snapshot.state = PlayerState::Playing;
            self.snapshot.song_id = Some(id);
            self.snapshot.elapsed = 0.461;
            if self.change_queue_on_play {
                self.snapshot.queue_version += 1;
            }
            Ok(())
        }
        fn pause(&mut self) -> Result<(), DspError> {
            self.calls.push("pause");
            if self.fail == Some("pause") {
                return Err(DspError::MpdCommandFailed("pause"));
            }
            self.snapshot.state = PlayerState::Paused;
            Ok(())
        }
        fn seek_id(&mut self, _: u32, elapsed: f64) -> Result<(), DspError> {
            self.calls.push("seekid");
            if self.fail == Some("seek") {
                return Err(DspError::MpdCommandFailed("seekid"));
            }
            self.snapshot.elapsed = elapsed;
            Ok(())
        }
        fn resume(&mut self) -> Result<(), DspError> {
            self.calls.push("resume");
            if self.fail == Some("resume") {
                return Err(DspError::MpdCommandFailed("resume"));
            }
            self.snapshot.state = PlayerState::Playing;
            Ok(())
        }
        fn stop(&mut self) -> Result<(), DspError> {
            self.calls.push("stop");
            self.snapshot.state = PlayerState::Stopped;
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeRoute {
        generation: u64,
        ready: bool,
        node_after: usize,
        checks: usize,
        fail: Option<&'static str>,
        connected: bool,
        active: bool,
        aborted: bool,
        calls: Vec<&'static str>,
    }

    impl FakeRoute {
        fn new() -> Self {
            Self {
                generation: 9,
                ready: true,
                node_after: 1,
                ..Self::default()
            }
        }
    }

    impl FirstStreamRoute for FakeRoute {
        fn ready(&self) -> bool {
            self.ready
        }
        fn generation(&self) -> u64 {
            self.generation
        }
        fn preflight(&mut self, _: &str) -> Result<bool, DspError> {
            self.calls.push("preflight");
            self.checks += 1;
            match self.fail {
                Some("unsafe") => Err(DspError::MpdNodeUnsafe),
                Some("camilla") => Err(DspError::CamillaExited),
                Some("sink") if self.checks >= 2 => Err(DspError::NodeMissing("null sink".into())),
                Some("stale") if self.checks >= 2 => {
                    self.generation += 1;
                    Ok(true)
                }
                _ => Ok(self.checks > self.node_after),
            }
        }
        fn connect_running(&mut self, _: &str) -> Result<(), DspError> {
            self.calls.push("connect");
            match self.fail {
                Some("link") => Err(DspError::LinkCreateFailed),
                Some("topology") => {
                    Err(DspError::TopologyInvalid(vec![RouteViolation::ExtraLink(
                        1,
                    )]))
                }
                _ => {
                    self.connected = true;
                    Ok(())
                }
            }
        }
        fn validate_paused(&mut self) -> Result<(), DspError> {
            self.calls.push("paused topology");
            if self.fail == Some("paused topology") {
                return Err(DspError::TopologyInvalid(vec![
                    RouteViolation::InactiveLink(1),
                ]));
            }
            Ok(())
        }
        fn validate_running(&mut self) -> Result<(), DspError> {
            self.calls.push("running topology");
            if self.fail == Some("running topology") {
                return Err(DspError::TopologyInvalid(vec![RouteViolation::ExtraLink(
                    2,
                )]));
            }
            Ok(())
        }
        fn commit(&mut self) -> Result<(), DspError> {
            self.calls.push("commit");
            if !self.connected || self.fail == Some("commit") {
                return Err(DspError::InvalidTransition);
            }
            self.active = true;
            Ok(())
        }
        fn abort(&mut self, _: DspError) -> Result<(), DspError> {
            self.calls.push("abort");
            self.aborted = true;
            self.active = false;
            Ok(())
        }
    }

    fn transaction() -> DspActivationTransaction {
        DspActivationTransaction::new(
            7,
            "mpd.dsp".into(),
            ActivationGate {
                generation: 9,
                current: Arc::new(AtomicU64::new(9)),
                claim: Arc::new(AtomicU64::new(0)),
            },
        )
    }

    #[test]
    fn first_stream_appears_and_commits_after_validation() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        transaction().execute(&mut player, &mut route).unwrap();
        assert!(route.active);
        assert_eq!(player.queue, vec![7]);
        assert_eq!(player.snapshot.queue_version, 4);
        assert_eq!(player.snapshot.song_id, Some(7));
        assert_eq!(player.calls.iter().filter(|c| **c == "playid").count(), 1);
        assert_eq!(route.calls.last(), Some(&"commit"));
        assert!(
            route.calls.iter().position(|c| *c == "running topology")
                < route.calls.iter().position(|c| *c == "commit")
        );
    }

    #[test]
    fn existing_node_does_not_replay_or_change_requested_position() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        player.snapshot.state = PlayerState::Playing;
        player.snapshot.song_id = Some(7);
        player.snapshot.elapsed = 12.345;
        route.node_after = 0;
        transaction().execute(&mut player, &mut route).unwrap();
        assert!(!player.calls.contains(&"playid"));
        assert!(!player.calls.contains(&"seekid"));
        assert!(!player.calls.contains(&"pause"));
        assert!((player.snapshot.elapsed - 12.345).abs() < 0.001);
    }

    #[test]
    fn absent_node_times_out_without_active() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        route.node_after = usize::MAX;
        let mut transaction = transaction();
        transaction.node_timeout = Duration::ZERO;
        assert_eq!(
            transaction.execute(&mut player, &mut route),
            Err(DspError::FirstStreamTimeout)
        );
        assert!(!route.active && route.aborted);
        assert_eq!(player.snapshot.state, PlayerState::Paused);
        assert_eq!(player.queue, vec![7]);
    }

    #[test]
    fn unsafe_node_camilla_exit_and_sink_loss_abort() {
        for (failure, expected) in [
            ("unsafe", DspError::MpdNodeUnsafe),
            ("camilla", DspError::CamillaExited),
            ("sink", DspError::NodeMissing("null sink".into())),
        ] {
            let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
            route.fail = Some(failure);
            assert_eq!(
                transaction().execute(&mut player, &mut route),
                Err(expected)
            );
            assert!(route.aborted && !route.active);
        }
    }

    #[test]
    fn link_and_topology_failures_never_publish_active() {
        for failure in ["link", "topology", "paused topology", "running topology"] {
            let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
            route.fail = Some(failure);
            assert!(transaction().execute(&mut player, &mut route).is_err());
            assert!(route.aborted && !route.active);
            assert!(!route.calls.contains(&"commit"));
        }
    }

    #[test]
    fn failed_seek_reports_restore_failure_and_leaves_paused() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        player.fail = Some("seek");
        assert_eq!(
            transaction().execute(&mut player, &mut route),
            Err(DspError::PlaybackRestoreFailed)
        );
        assert!(route.aborted && !route.active);
        assert_eq!(player.snapshot.state, PlayerState::Paused);
    }

    #[test]
    fn cancelled_play_and_stale_generation_cannot_resume() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        player.cancel_on_play = Some(transaction().gate);
        let gate = player.cancel_on_play.clone().unwrap();
        let transaction = DspActivationTransaction::new(7, "mpd.dsp".into(), gate);
        assert_eq!(
            transaction.execute(&mut player, &mut route),
            Err(DspError::ActivationCancelled)
        );
        assert!(!player.calls.contains(&"resume"));
        assert!(route.aborted);
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        route.fail = Some("stale");
        assert_eq!(
            transaction_for_stale().execute(&mut player, &mut route),
            Err(DspError::SessionStale)
        );
        assert!(!route.active);
        assert!(!route.aborted);
        assert!(!player.calls.contains(&"pause"));
    }

    fn transaction_for_stale() -> DspActivationTransaction {
        transaction()
    }

    #[test]
    fn duplicate_activation_is_rejected_without_tearing_down_active_route() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        route.ready = false;
        route.active = true;
        assert_eq!(
            transaction().execute(&mut player, &mut route),
            Err(DspError::InvalidTransition)
        );
        assert!(route.active && !route.aborted);
    }

    #[test]
    fn concurrent_activation_claim_is_rejected_and_released() {
        let transaction = transaction();
        let claim = transaction.gate.reserve().unwrap();
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        assert_eq!(
            transaction.execute(&mut player, &mut route),
            Err(DspError::InvalidTransition)
        );
        assert!(player.calls.is_empty());
        assert!(!route.aborted);
        drop(claim);
        transaction.execute(&mut player, &mut route).unwrap();
    }

    #[test]
    fn output_config_and_spawn_command_are_isolated() {
        let config = dsp_mpd_output_config("Sonante DSP", "sonante_capture").unwrap();
        assert!(config.contains("type \"pipewire\""));
        assert!(config.contains("always_on \"yes\""));
        assert_eq!(config.matches("audio_output {").count(), 1);
        assert_eq!(
            dsp_mpd_output_config("bad\nname", "capture"),
            Err(DspError::InvalidIdentifier)
        );
        let command = dsp_mpd_command(Path::new("/mpd"), Path::new("/config"));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "PIPEWIRE_AUTOCONNECT"
                && value == Some(std::ffi::OsStr::new("0"))));
    }

    #[test]
    fn paused_existing_node_resumes_without_playid_then_restores_position() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        player.snapshot.state = PlayerState::Paused;
        player.snapshot.song_id = Some(7);
        player.snapshot.elapsed = 4.25;
        route.node_after = 0;
        transaction().execute(&mut player, &mut route).unwrap();
        assert!(!player.calls.contains(&"playid"));
        assert!((player.snapshot.elapsed - 4.25).abs() < 0.001);
    }

    #[test]
    fn queue_change_or_restore_failure_never_resumes_after_rollback() {
        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        player.change_queue_on_play = true;
        assert_eq!(
            transaction().execute(&mut player, &mut route),
            Err(DspError::PlaybackSnapshotFailed)
        );
        assert!(route.aborted && !route.active);
        assert!(!player.calls.contains(&"seekid"));

        let (mut player, mut route) = (FakePlayback::stopped(), FakeRoute::new());
        player.fail = Some("pause");
        assert!(transaction().execute(&mut player, &mut route).is_err());
        assert_eq!(player.snapshot.state, PlayerState::Stopped);
        assert!(route.aborted && !route.active);
    }
}
