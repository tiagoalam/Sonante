//! Explicit host-only lifecycle probe: cargo test dsp::harness -- --ignored --nocapture
use super::activation::{
    dsp_mpd_command, dsp_mpd_output_config, DspActivationTransaction, FirstStreamPlayback,
    FirstStreamRoute, MpdSocketControl, PlaybackSnapshot, PlayerState,
};
use super::peq::PcmFormat;
use super::pipewire::{
    exact_node, validate_topology, PipeWireRouteManager, RouteStatus, SystemCommandRunner,
};
use super::runtime::{
    CamillaRuntimeController, LocalWebSocketTransport, RuntimeError, RuntimeHealth,
    RuntimeTransport,
};
use super::{DspState, DspSupervisor};
use crate::equalizer::flat_preset;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::{client, Message};

fn command(program: &str, args: &[&str]) -> Result<Output, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output)
}

fn default_sink() -> Result<String, String> {
    let output = command("pactl", &["info"])?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("Default Sink: ").map(str::to_owned))
        .ok_or_else(|| "pactl did not report a default sink".into())
}

fn write_wav(path: &Path) -> Result<(), String> {
    let samples = 48000_u32 * 120;
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    file.write_all(b"RIFF").map_err(|error| error.to_string())?;
    file.write_all(&(36 + samples * 4).to_le_bytes())
        .map_err(|error| error.to_string())?;
    file.write_all(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x02\x00\x80\xbb\x00\x00\x00\xee\x02\x00\x04\x00\x10\x00data")
        .map_err(|error| error.to_string())?;
    file.write_all(&(samples * 4).to_le_bytes())
        .map_err(|error| error.to_string())?;
    for index in 0..samples {
        let phase = 2.0 * std::f64::consts::PI * f64::from(index % 48) / 48.0;
        let sample = (phase.sin() * 8_000.0) as i16;
        file.write_all(&sample.to_le_bytes())
            .map_err(|error| error.to_string())?;
        file.write_all(&sample.to_le_bytes())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

struct Probe {
    root: PathBuf,
    mpd: Option<Child>,
    pure_mpd: Option<Child>,
    null_module: Option<String>,
    dsp: DspSupervisor<SystemCommandRunner>,
}

#[derive(Clone)]
struct ActivationTrace {
    started: Instant,
    events: Arc<Mutex<Vec<(String, Duration, Option<PlaybackSnapshot>)>>>,
}

impl ActivationTrace {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn record(&self, label: &str, snapshot: Option<PlaybackSnapshot>) {
        if let Ok(mut events) = self.events.lock() {
            events.push((label.into(), self.started.elapsed(), snapshot));
        }
    }
    fn print(&self, label: &str) -> Result<(), String> {
        let events = self
            .events
            .lock()
            .map_err(|_| "activation trace poisoned")?;
        for (name, at, snapshot) in events.iter() {
            println!(
                "{label} trace t={:.3}s {name} {snapshot:?}",
                at.as_secs_f64()
            );
        }
        Ok(())
    }
    fn contains(&self, label: &str) -> Result<bool, String> {
        Ok(self
            .events
            .lock()
            .map_err(|_| "activation trace poisoned")?
            .iter()
            .any(|(name, _, _)| name == label))
    }
    fn first_status(&self) -> Result<PlaybackSnapshot, String> {
        self.events
            .lock()
            .map_err(|_| "activation trace poisoned")?
            .iter()
            .find_map(|(_, _, snapshot)| snapshot.clone())
            .ok_or_else(|| "activation trace has no status".into())
    }
    fn status_after(&self, label: &str) -> Result<PlaybackSnapshot, String> {
        let events = self
            .events
            .lock()
            .map_err(|_| "activation trace poisoned")?;
        let index = events
            .iter()
            .position(|(name, _, _)| name == label)
            .ok_or_else(|| format!("activation trace missing {label}"))?;
        events[index + 1..]
            .iter()
            .find_map(|(_, _, snapshot)| snapshot.clone())
            .ok_or_else(|| format!("activation trace has no status after {label}"))
    }
}

struct TracedPlayback<'a> {
    inner: &'a mut MpdSocketControl,
    trace: ActivationTrace,
}
impl FirstStreamPlayback for TracedPlayback<'_> {
    fn snapshot(&mut self) -> Result<PlaybackSnapshot, super::DspError> {
        let result = self.inner.snapshot();
        self.trace.record("status", result.as_ref().ok().cloned());
        result
    }
    fn contains_song(&mut self, id: u32) -> Result<bool, super::DspError> {
        self.inner.contains_song(id)
    }
    fn play_id(&mut self, id: u32) -> Result<(), super::DspError> {
        self.trace.record("playid intent", None);
        let result = self.inner.play_id(id);
        self.trace.record("playid accepted", None);
        result
    }
    fn pause(&mut self) -> Result<(), super::DspError> {
        self.trace.record("pause intent", None);
        let result = self.inner.pause();
        self.trace.record("pause accepted", None);
        result
    }
    fn seek_id(&mut self, id: u32, elapsed: f64) -> Result<(), super::DspError> {
        self.trace.record("seek intent", None);
        let result = self.inner.seek_id(id, elapsed);
        self.trace.record("seek accepted", None);
        result
    }
    fn resume(&mut self) -> Result<(), super::DspError> {
        self.trace.record("resume intent", None);
        let result = self.inner.resume();
        self.trace.record("resume accepted", None);
        result
    }
    fn stop(&mut self) -> Result<(), super::DspError> {
        self.inner.stop()
    }
}

struct TracedRoute<'a> {
    inner: &'a mut DspSupervisor<SystemCommandRunner>,
    trace: ActivationTrace,
}
impl FirstStreamRoute for TracedRoute<'_> {
    fn ready(&self) -> bool {
        self.inner.ready()
    }
    fn generation(&self) -> u64 {
        self.inner.generation()
    }
    fn preflight(&mut self, name: &str) -> Result<bool, super::DspError> {
        self.trace.record("preflight start", None);
        let result = self.inner.preflight(name);
        self.trace.record(
            if result.as_ref().is_ok_and(|found| *found) {
                "stream available"
            } else {
                "preflight end"
            },
            None,
        );
        result
    }
    fn connect_running(&mut self, name: &str) -> Result<(), super::DspError> {
        self.trace.record("connect start", None);
        let result = self.inner.connect_running(name);
        self.trace.record("connect end", None);
        result
    }
    fn validate_paused(&mut self) -> Result<(), super::DspError> {
        self.trace.record("validate paused start", None);
        let result = self.inner.validate_paused();
        self.trace.record("validate paused end", None);
        result
    }
    fn validate_running(&mut self) -> Result<(), super::DspError> {
        self.trace.record("validate running start", None);
        let result = self.inner.validate_running();
        self.trace.record("validate running end", None);
        result
    }
    fn commit(&mut self) -> Result<(), super::DspError> {
        self.trace.record("commit start", None);
        let result = self.inner.commit();
        self.trace.record("commit end", None);
        result
    }
    fn abort(&mut self, reason: super::DspError) -> Result<(), super::DspError> {
        self.inner.abort(reason)
    }
}

impl Probe {
    fn new() -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("sonante-eq1c-{}-{stamp}", std::process::id()));
        fs::create_dir(&root).map_err(|e| e.to_string())?;
        Ok(Self {
            dsp: DspSupervisor::new(SystemCommandRunner).with_runtime_parent(root.clone()),
            root,
            mpd: None,
            pure_mpd: None,
            null_module: None,
        })
    }

    fn close(&mut self) -> Result<(), String> {
        let mut problems = Vec::new();
        if let Err(error) = self.dsp.stop() {
            problems.push(format!("DSP: {error:?}"));
        }
        if let Some(mut mpd) = self.mpd.take() {
            if mpd.try_wait().map_err(|e| e.to_string())?.is_none() {
                if let Err(error) = mpd.kill() {
                    problems.push(format!("MPD kill: {error}"));
                }
            }
            if let Err(error) = mpd.wait() {
                problems.push(format!("MPD wait: {error}"));
            }
        }
        if let Some(mut mpd) = self.pure_mpd.take() {
            if mpd.try_wait().map_err(|e| e.to_string())?.is_none() {
                if let Err(error) = mpd.kill() {
                    problems.push(format!("pure MPD kill: {error}"));
                }
            }
            if let Err(error) = mpd.wait() {
                problems.push(format!("pure MPD wait: {error}"));
            }
        }
        if let Some(module) = self.null_module.take() {
            if let Err(error) = command("pactl", &["unload-module", &module]) {
                problems.push(error);
            }
        }
        if let Err(error) = fs::remove_dir_all(&self.root) {
            problems.push(format!("temporary files: {error}"));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn restart_dsp(&mut self, binary: &Path, default_before: &str) -> Result<u64, String> {
        self.dsp.stop().map_err(|e| format!("DSP stop: {e:?}"))?;
        if let Some(module) = self.null_module.take() {
            command("pactl", &["unload-module", &module])?;
        }
        let (session, sink_name) = self
            .dsp
            .start(Some(binary))
            .map_err(|e| format!("DSP restart: {e:?}"))?;
        let module = command(
            "pactl",
            &[
                "load-module",
                "module-null-sink",
                &format!("sink_name={sink_name}"),
                "sink_properties=device.description=SonanteEQ1CNull",
            ],
        )?;
        self.null_module = Some(String::from_utf8_lossy(&module.stdout).trim().to_owned());
        if default_sink()? != default_before {
            return Err("default sink changed".into());
        }
        let graph = self
            .dsp
            .route_manager
            .snapshot()
            .map_err(|e| format!("snapshot: {e:?}"))?;
        if !exact_node(&graph, &sink_name, "Audio/Sink")
            .map_err(|e| format!("sink: {e:?}"))?
            .virtual_sink
        {
            return Err("restart sink is not virtual".into());
        }
        Ok(session)
    }
}

fn no_hardware_fallback(probe: &mut Probe, mpd_name: &str) -> Result<(), String> {
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let capture_name = &probe
        .dsp
        .route
        .as_ref()
        .ok_or("route missing")?
        .capture_name;
    if let Some(mpd) = graph.nodes.iter().find(|n| n.name == mpd_name) {
        let mpd_outputs: Vec<_> = graph
            .ports
            .iter()
            .filter(|p| p.node_id == mpd.id)
            .map(|p| p.id)
            .collect();
        for link in &graph.links {
            if mpd_outputs.contains(&link.output_port) {
                let input = graph
                    .ports
                    .iter()
                    .find(|p| p.id == link.input_port)
                    .ok_or("unknown link input")?;
                let destination = graph
                    .nodes
                    .iter()
                    .find(|n| n.id == input.node_id)
                    .ok_or("unknown link destination")?;
                if destination.name != *capture_name {
                    return Err(format!("MPD fallback link to {}", destination.name));
                }
            }
        }
    }
    Ok(())
}

fn reactivation_probe(
    probe: &mut Probe,
    player: &mut MpdSocketControl,
    song_id: u32,
    mpd_name: &str,
    queue_before: &[String],
    label: &str,
    expected_state: PlayerState,
) -> Result<(), String> {
    let before = player
        .snapshot()
        .map_err(|e| format!("{label} before: {e:?}"))?;
    if before.state != expected_state {
        return Err(format!("{label}: wrong initial state {before:?}"));
    }
    probe
        .dsp
        .route_manager
        .remove_owned_links()
        .map_err(|e| format!("{label} disconnect: {e:?}"))?;
    probe.dsp.route.as_mut().ok_or("route missing")?.mpd_name = None;
    probe.dsp.route_status = RouteStatus::CamillaReady;
    probe
        .dsp
        .state
        .transition(DspState::Starting)
        .map_err(|e| format!("{label} prepare: {e:?}"))?;
    no_hardware_fallback(probe, mpd_name)?;
    let trace = ActivationTrace::new();
    let transaction =
        DspActivationTransaction::new(song_id, mpd_name.into(), probe.dsp.activation_gate());
    transaction
        .execute(
            &mut TracedPlayback {
                inner: player,
                trace: trace.clone(),
            },
            &mut TracedRoute {
                inner: &mut probe.dsp,
                trace: trace.clone(),
            },
        )
        .map_err(|e| format!("{label} activation: {e:?}"))?;
    trace.print(label)?;
    let after = player
        .snapshot()
        .map_err(|e| format!("{label} after: {e:?}"))?;
    if expected_state == PlayerState::Playing {
        if trace.contains("playid intent")?
            || trace.contains("seek intent")?
            || trace.contains("pause intent")?
            || after.elapsed < trace.first_status()?.elapsed
        {
            return Err(format!("{label}: playing position was reset"));
        }
    } else {
        let restored = trace.status_after("seek accepted")?;
        if restored.state != PlayerState::Paused || (restored.elapsed - before.elapsed).abs() > 0.15
        {
            return Err(format!(
                "{label}: paused seek restored {restored:?} from {before:?}"
            ));
        }
    }
    if after.song_id != Some(song_id)
        || after.state != PlayerState::Playing
        || player
            .command("playlistinfo", "playlistinfo")
            .map_err(|e| format!("{label} queue: {e:?}"))?
            != queue_before
    {
        return Err(format!("{label}: state or queue changed"));
    }
    if route_identity(probe).is_err() {
        return Err(format!("{label}: route invalid"));
    }
    println!(
        "{label}: before={:.3}s {:?}, after={:.3}s, activation={:.3}s",
        before.elapsed,
        before.state,
        after.elapsed,
        trace.started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn route_identity(
    probe: &mut Probe,
) -> Result<(u32, (u32, Option<u64>), (u32, Option<u64>), Vec<u32>), String> {
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let route = probe.dsp.route.as_ref().ok_or("route missing")?;
    let status = validate_topology(&graph, route, true);
    if status != RouteStatus::RouteReady {
        return Err(format!(
            "route changed during preset update: {status:?}, owned links={:?}",
            probe.dsp.route_manager.owned_links()
        ));
    }
    let capture = exact_node(&graph, &route.capture_name, "Stream/Input/Audio")
        .map_err(|e| format!("capture: {e:?}"))?;
    let playback = exact_node(&graph, &route.playback_name, "Stream/Output/Audio")
        .map_err(|e| format!("playback: {e:?}"))?;
    let mut links = probe.dsp.route_manager.owned_links().to_vec();
    links.sort_unstable();
    if links.len() != 4 {
        return Err("expected exactly four owned links".into());
    }
    let pid = probe
        .dsp
        .process
        .as_ref()
        .ok_or("Camilla process missing")?
        .id();
    Ok((
        pid,
        (capture.id, capture.serial),
        (playback.id, playback.serial),
        links,
    ))
}

fn assert_transition_gain_zero(probe: &Probe) -> Result<(), String> {
    let port = probe
        .dsp
        .runtime_endpoint
        .as_ref()
        .ok_or("endpoint missing")?
        .port;
    let reply = LocalWebSocketTransport
        .request(port, json!("GetVolume"))
        .map_err(|e| format!("GetVolume: {e:?}"))?;
    let gain = reply
        .pointer("/GetVolume/value")
        .and_then(Value::as_f64)
        .ok_or("GetVolume has no numeric value")?;
    if gain != 0.0 {
        return Err(format!("transition gain remained {gain} dB"));
    }
    Ok(())
}

fn runtime_switch_probe(probe: &mut Probe, session: u64) -> Result<(), String> {
    let baseline = route_identity(probe)?;
    let controller = probe
        .dsp
        .runtime_controller()
        .map_err(|e| format!("controller: {e:?}"))?;
    let flat = flat_preset();
    let mut a = flat.clone();
    a.id = "probe_a".into();
    a.name = "Probe A".into();
    a.bands.truncate(1);
    a.bands[0].frequency_hz = 1000.0;
    a.bands[0].gain_db = -6.0;
    let mut b = flat.clone();
    b.id = "probe_b".into();
    b.name = "Probe B".into();
    b.preamp_db = -3.0;
    b.bands[5].gain_db = 3.0;
    let format = PcmFormat {
        sample_rate_hz: 48_000,
        channels: 2,
    };
    for (label, preset) in [("Flat", &flat), ("A", &a), ("B", &b), ("Flat again", &flat)] {
        controller
            .apply_preset(session, preset, format)
            .map_err(|e| format!("apply {label}: {e:?}"))?;
        let after = route_identity(probe)?;
        println!(
            "preset {label}: PID={}, capture={:?}, playback={:?}, links={:?}",
            after.0, after.1, after.2, after.3
        );
        if after != baseline {
            return Err(format!("preset {label} recreated nodes or links"));
        }
        assert_transition_gain_zero(probe)?;
    }
    // Same filter identity across zero gain, enable, and parameter edits.
    let mut changed = a.clone();
    changed.bands[0].gain_db = 0.0;
    for gain in [3.0, 0.0] {
        changed.bands[0].gain_db = gain;
        controller
            .apply_preset(session, &changed, format)
            .map_err(|e| format!("gain {gain}: {e:?}"))?;
        if route_identity(probe)? != baseline {
            return Err("gain update recreated route".into());
        }
    }
    changed.bands[0].enabled = false;
    controller
        .apply_preset(session, &changed, format)
        .map_err(|e| format!("disable: {e:?}"))?;
    changed.bands[0].enabled = true;
    changed.bands[0].frequency_hz = 1200.0;
    changed.bands[0].q = 2.0;
    changed.bands[0].gain_db = -3.0;
    controller
        .apply_preset(session, &changed, format)
        .map_err(|e| format!("freq/Q: {e:?}"))?;
    controller
        .apply_preset(session, &flat, format)
        .map_err(|e| format!("restore Flat: {e:?}"))?;
    if route_identity(probe)? != baseline {
        return Err("final route differs".into());
    }
    Ok(())
}

fn websocket_control_probe(probe: &mut Probe, session: u64) -> Result<(), String> {
    let endpoint = probe
        .dsp
        .runtime_endpoint
        .clone()
        .ok_or("endpoint missing")?;
    let before = probe
        .dsp
        .runtime_controller()
        .map_err(|e| format!("controller: {e:?}"))?
        .published_preset()
        .map_err(|e| format!("published: {e:?}"))?;
    let stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port))
        .map_err(|e| format!("real WebSocket TCP: {e}"))?;
    let (mut socket, _) = client(format!("ws://127.0.0.1:{}/", endpoint.port), stream)
        .map_err(|e| format!("real WebSocket handshake: {e}"))?;
    socket
        .send(Message::Text("\"GetState\"".into()))
        .map_err(|e| format!("GetState send: {e}"))?;
    socket.read().map_err(|e| format!("GetState read: {e}"))?;
    socket
        .get_mut()
        .shutdown(Shutdown::Both)
        .map_err(|e| format!("WebSocket shutdown: {e}"))?;
    let closed = socket.read().expect_err("closed WebSocket read succeeded");
    println!("real Camilla WebSocket closed by client: {closed:?}; controller uses a new connection per request");
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("refused port reserve: {e}"))?;
    let refused_port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    let mut refused_endpoint = endpoint.clone();
    refused_endpoint.port = refused_port;
    let refused = CamillaRuntimeController::new(
        refused_endpoint,
        Arc::clone(&probe.dsp.activation_generation),
        Arc::clone(&probe.dsp.runtime_shared),
        LocalWebSocketTransport,
    );
    let result = refused.apply_preset(
        session,
        &flat_preset(),
        PcmFormat {
            sample_rate_hz: 48_000,
            channels: 2,
        },
    );
    if result != Err(RuntimeError::WebSocketConnectFailed) {
        return Err(format!("refused connection: {result:?}"));
    }
    let after = probe
        .dsp
        .runtime_controller()
        .map_err(|e| format!("controller: {e:?}"))?
        .published_preset()
        .map_err(|e| format!("published: {e:?}"))?;
    if before != after {
        return Err("WebSocket failures published a preset".into());
    }
    assert_transition_gain_zero(probe)?;
    println!(
        "real TCP refusal: WebSocketConnectFailed, preset unchanged, Camilla control still healthy"
    );
    Ok(())
}

fn exercise_stop_play(
    player: &mut MpdSocketControl,
    child: &mut Child,
    song_id: u32,
    label: &str,
) -> Result<(), String> {
    for sequence in ["stop_play", "pause_play", "stop_playid"]
        .into_iter()
        .cycle()
        .take(9)
    {
        match sequence {
            "pause_play" => player
                .pause()
                .map_err(|e| format!("{label} pause: {e:?}"))?,
            _ => player.stop().map_err(|e| format!("{label} stop: {e:?}"))?,
        }
        let before = player
            .snapshot()
            .map_err(|e| format!("{label} {sequence} before: {e:?}"))?;
        if sequence == "stop_playid" {
            player
                .play_id(song_id)
                .map_err(|e| format!("{label} playid: {e:?}"))?;
        } else {
            player
                .command("play", "play")
                .map_err(|e| format!("{label} play: {e:?}"))?;
        }
        wait_for(Duration::from_secs(2), || {
            player
                .snapshot()
                .is_ok_and(|now| now.state == PlayerState::Playing)
        })?;
        thread::sleep(Duration::from_millis(100));
        let after = player
            .snapshot()
            .map_err(|e| format!("{label} {sequence} after: {e:?}"))?;
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err(format!("{label} exited during {sequence}"));
        }
        println!(
            "{label} {sequence}: {:?} {:.3}s -> {:?} {:.3}s, MPD alive",
            before.state, before.elapsed, after.state, after.elapsed
        );
    }
    Ok(())
}

fn pure_pipewire_mpd_probe(
    probe: &mut Probe,
    music: &Path,
    sink_name: &str,
    mpd_name: &str,
) -> Result<(), String> {
    let root = probe.root.join("pure_mpd");
    fs::create_dir(&root).map_err(|e| e.to_string())?;
    let socket = root.join("mpd.sock");
    let output_name = format!("EQ3D pure {}", std::process::id());
    let node_name = format!("mpd.{output_name}");
    let output = dsp_mpd_output_config(&output_name, sink_name)
        .map_err(|e| format!("pure output: {e:?}"))?;
    let config = format!("music_directory \"{}\"\nplaylist_directory \"{}\"\ndb_file \"{}\"\nlog_file \"{}\"\npid_file \"{}\"\nbind_to_address \"{}\"\nauto_update \"no\"\nreplaygain \"off\"\n{}\n",
        music.display(), root.display(), root.join("db").display(), root.join("mpd.log").display(), root.join("mpd.pid").display(), socket.display(), output);
    let config_path = root.join("mpd.conf");
    fs::write(&config_path, config).map_err(|e| e.to_string())?;
    probe.pure_mpd = Some(
        dsp_mpd_command(Path::new("mpd"), &config_path)
            .spawn()
            .map_err(|e| format!("pure MPD spawn: {e}"))?,
    );
    let mut player = MpdSocketControl::new(socket);
    wait_for(Duration::from_secs(3), || player.snapshot().is_ok())?;
    player
        .command("update", "update")
        .map_err(|e| format!("pure update: {e:?}"))?;
    wait_for(Duration::from_secs(3), || {
        player
            .command("status", "status")
            .is_ok_and(|lines| !lines.iter().any(|line| line.starts_with("updating_db:")))
    })?;
    player
        .command("add", "add \"real_track.wav\"")
        .map_err(|e| format!("pure add: {e:?}"))?;
    let queue = player
        .command("playlistinfo", "playlistinfo")
        .map_err(|e| format!("pure queue: {e:?}"))?;
    let id = queue
        .iter()
        .find_map(|line| {
            line.strip_prefix("Id: ")
                .and_then(|value| value.parse::<u32>().ok())
        })
        .ok_or("pure song id missing")?;
    player
        .play_id(id)
        .map_err(|e| format!("pure initial play: {e:?}"))?;
    wait_for(Duration::from_secs(2), || {
        probe
            .dsp
            .route_manager
            .snapshot()
            .is_ok_and(|graph| exact_node(&graph, &node_name, "Stream/Output/Audio").is_ok())
    })?;
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("pure graph: {e:?}"))?;
    let pure = exact_node(&graph, &node_name, "Stream/Output/Audio")
        .map_err(|e| format!("pure node: {e:?}"))?;
    if pure.autoconnect != Some(false)
        || graph.links.iter().any(|link| {
            graph
                .ports
                .iter()
                .any(|port| port.node_id == pure.id && port.id == link.output_port)
        })
    {
        return Err("pure MPD autoconnected or linked to a sink".into());
    }
    let child = probe.pure_mpd.as_mut().ok_or("pure MPD child missing")?;
    exercise_stop_play(&mut player, child, id, "pure PipeWire")?;
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("pure final graph: {e:?}"))?;
    let pure = exact_node(&graph, &node_name, "Stream/Output/Audio")
        .map_err(|e| format!("pure final node: {e:?}"))?;
    if pure.autoconnect != Some(false)
        || graph.links.iter().any(|link| {
            graph
                .ports
                .iter()
                .any(|port| port.node_id == pure.id && port.id == link.output_port)
        })
    {
        return Err("pure MPD gained an output link".into());
    }
    if player
        .command("playlistinfo", "playlistinfo")
        .map_err(|e| format!("pure queue: {e:?}"))?
        != queue
    {
        return Err("pure MPD queue changed".into());
    }
    no_hardware_fallback(probe, mpd_name)?;
    Ok(())
}

fn rms(samples: &[i16], start: usize, end: usize) -> f64 {
    let sum: f64 = samples[start..end]
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum();
    (sum / (end - start) as f64).sqrt()
}

fn max_step(samples: &[i16], start: usize, end: usize) -> i32 {
    samples[start..end]
        .windows(2)
        .map(|pair| (i32::from(pair[1]) - i32::from(pair[0])).abs())
        .max()
        .unwrap_or(0)
}

fn max_cycle_peak_change(samples: &[i16], start: usize, end: usize) -> i32 {
    // The test tone is 1 kHz at 48 kHz: one cycle is exactly 48 frames.
    let peaks: Vec<i32> = samples[start..end]
        .chunks_exact(48)
        .map(|cycle| {
            cycle
                .iter()
                .map(|sample| i32::from(*sample).abs())
                .max()
                .unwrap_or(0)
        })
        .collect();
    peaks
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .max()
        .unwrap_or(0)
}

fn periodic_residual(samples: &[i16], start: usize, end: usize) -> (f64, f64) {
    // A 1 kHz sine at 48 kHz satisfies this recurrence at *any* amplitude.
    // A large residual therefore isolates nonperiodic transition energy from
    // the ordinary slope of an amplified sine.
    let coefficient = 2.0 * (2.0 * std::f64::consts::PI / 48.0).cos();
    let residuals = (start.max(2)..end).map(|n| {
        f64::from(samples[n]) - coefficient * f64::from(samples[n - 1]) + f64::from(samples[n - 2])
    });
    let mut peak: f64 = 0.0;
    let mut energy = 0.0;
    let mut count = 0;
    for value in residuals {
        peak = peak.max(value.abs());
        energy += value * value;
        count += 1;
    }
    (peak, (energy / f64::from(count)).sqrt())
}

fn low_level_frames(samples: &[i16], start: usize, end: usize, threshold: f64) -> usize {
    samples[start..end]
        .chunks_exact(48)
        .filter(|cycle| rms(cycle, 0, 48) < threshold)
        .count()
        * 48
}

fn record_transition(
    probe: &mut Probe,
    label: &str,
    apply: impl FnOnce() -> Result<(), String>,
) -> Result<(f64, f64, i32), String> {
    let sink_name = probe
        .dsp
        .route
        .as_ref()
        .ok_or("route missing")?
        .sink_name
        .clone();
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let sink =
        exact_node(&graph, &sink_name, "Audio/Sink").map_err(|e| format!("null sink: {e:?}"))?;
    if !sink.virtual_sink {
        return Err("record target is not a virtual sink".into());
    }
    if graph.nodes.iter().any(|node| node.name == "pw-record") {
        return Err("another pw-record is present".into());
    }
    let sample_count = if label == "rollback_timeout" {
        "240000"
    } else {
        "96000"
    };
    let mut child = Command::new("pw-record")
        .args([
            "--target",
            "0",
            "--rate",
            "48000",
            "--channels",
            "2",
            "--format",
            "s16",
            "-a",
            "-n",
            sample_count,
            "-",
        ])
        .env("PIPEWIRE_AUTOCONNECT", "0")
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("pw-record: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("pw-record stdout missing")?;
    let frame_count = Arc::new(AtomicUsize::new(0));
    let reader_count = Arc::clone(&frame_count);
    let reader = thread::spawn(move || -> Result<Vec<i16>, String> {
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let count = stdout.read(&mut chunk).map_err(|e| e.to_string())?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            reader_count.store(bytes.len() / 4, Ordering::Release);
        }
        Ok(bytes
            .chunks_exact(4)
            .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
            .collect())
    });
    let result: Result<(Instant, Instant, usize, usize), String> = (|| {
        wait_for(Duration::from_secs(2), || {
            probe.dsp.route_manager.snapshot().is_ok_and(|graph| {
                graph
                    .nodes
                    .iter()
                    .any(|node| node.name == "pw-record" && node.autoconnect == Some(false))
            })
        })?;
        for channel in ["FL", "FR"] {
            command(
                "pw-link",
                &[
                    &format!("{sink_name}:monitor_{channel}"),
                    &format!("pw-record:input_{channel}"),
                ],
            )?;
        }
        thread::sleep(Duration::from_millis(750));
        let start_time = Instant::now();
        let start_frame = frame_count.load(Ordering::Acquire);
        apply()?;
        let end_time = Instant::now();
        let end_frame = frame_count.load(Ordering::Acquire);
        wait_for(Duration::from_secs(7), || {
            child.try_wait().is_ok_and(|status| status.is_some())
        })?;
        Ok((start_time, end_time, start_frame, end_frame))
    })();
    if child.try_wait().map_err(|e| e.to_string())?.is_none() {
        child.kill().map_err(|e| e.to_string())?;
    }
    child.wait().map_err(|e| e.to_string())?;
    let samples = reader.join().map_err(|_| "recorder reader panicked")??;
    let (start_time, end_time, start_frame, end_frame) = result?;
    if samples.len() < 80_000 || start_frame < 9_600 || end_frame + 14_400 >= samples.len() {
        return Err(format!(
            "short or misaligned recording: {} frames, operation {start_frame}..{end_frame}",
            samples.len()
        ));
    }
    let before = rms(&samples, start_frame - 9_600, start_frame - 4_800);
    let after = rms(&samples, end_frame + 9_600, end_frame + 14_400);
    let window_start = start_frame - 2_400;
    let window_end = end_frame + 9_600;
    let step = max_step(&samples, window_start, window_end);
    let cycle_change = max_cycle_peak_change(&samples, window_start, window_end);
    let peak = samples[window_start..window_end]
        .iter()
        .map(|sample| i32::from(*sample).abs())
        .max()
        .unwrap_or(0);
    let (residual_peak, residual_rms) = periodic_residual(&samples, window_start, window_end);
    let (baseline_peak, baseline_rms) =
        periodic_residual(&samples, start_frame - 9_600, start_frame - 4_800);
    let dip_frames = low_level_frames(&samples, start_frame, window_end, before.min(after) * 0.1);
    println!("{label}: operation frames={start_frame}..{end_frame}, wall={:?}..{:?}, duration_ms={:.1}, RMS={before:.1}->{after:.1}, peak={peak}, step={step}, cycle_peak_change={cycle_change}, periodic_residual peak={residual_peak:.1} baseline={baseline_peak:.1}, rms={residual_rms:.1} baseline={baseline_rms:.1}, dip_ms={:.1}", start_time, end_time, (end_time - start_time).as_secs_f64() * 1000.0, dip_frames as f64 / 48.0);
    Ok((before, after, step))
}

fn signal_probe(probe: &mut Probe, session: u64, mitigate: bool) -> Result<(), String> {
    let controller = probe
        .dsp
        .runtime_controller()
        .map_err(|e| format!("controller: {e:?}"))?;
    let format = PcmFormat {
        sample_rate_hz: 48_000,
        channels: 2,
    };
    let flat = flat_preset();
    let mut plus = flat.clone();
    plus.bands.truncate(1);
    plus.bands[0].frequency_hz = 1000.0;
    plus.bands[0].gain_db = 6.0;
    let mut minus = plus.clone();
    minus.bands[0].gain_db = -6.0;
    let mut ten = flat.clone();
    ten.id = "ten_band".into();
    for (index, band) in ten.bands.iter_mut().enumerate() {
        band.gain_db = if index % 2 == 0 { 3.0 } else { -3.0 };
    }
    let mode = if mitigate { "ramp" } else { "direct" };
    let apply = |preset: &crate::equalizer::EqPreset| {
        let result = if mitigate {
            controller.apply_preset(session, preset, format)
        } else {
            controller.apply_without_transition_for_probe(session, preset, format)
        };
        result.map_err(|e| format!("{mode} apply: {e:?}"))
    };
    apply(&flat)?;
    let (_, _, baseline_step) =
        record_transition(probe, &format!("{mode}_flat_flat"), || apply(&flat))?;
    let (flat_rms, plus_rms, plus_step) =
        record_transition(probe, &format!("{mode}_flat_plus6"), || apply(&plus))?;
    let (plus_before, minus_rms, minus_step) =
        record_transition(probe, &format!("{mode}_plus6_minus6"), || apply(&minus))?;
    let (minus_before, flat_again, flat_step) =
        record_transition(probe, &format!("{mode}_minus6_flat"), || apply(&flat))?;
    record_transition(probe, &format!("{mode}_flat_ten"), || apply(&ten))?;
    println!("{mode} transition max steps: baseline={baseline_step}, Flat->+6={plus_step}, +6->-6={minus_step}, -6->Flat={flat_step}");
    if !(1.7..2.3).contains(&(plus_rms / flat_rms))
        || !(0.20..0.32).contains(&(minus_rms / plus_before))
        || !(1.7..2.3).contains(&(flat_again / minus_before))
    {
        return Err("measured PEQ response outside expected ranges".into());
    }
    if mitigate {
        for (label, attenuation, settle_ms) in [
            ("settle125", -60.0, 125),
            ("settle75", -60.0, 75),
            ("attenuation40", -40.0, 150),
        ] {
            controller
                .apply_preset(session, &plus, format)
                .map_err(|e| format!("variant setup: {e:?}"))?;
            record_transition(probe, label, || {
                controller
                    .apply_transition_for_probe(
                        session,
                        &minus,
                        format,
                        attenuation,
                        Duration::from_millis(settle_ms),
                    )
                    .map_err(|e| format!("variant {label}: {e:?}"))
            })?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum FaultMode {
    Invalid,
    Semantic,
    LostReply,
    Timeout,
    RollbackRejected,
    RealSocketDrop,
}

struct FaultTransport {
    mode: FaultMode,
    armed: AtomicBool,
    patches: AtomicUsize,
}

impl RuntimeTransport for FaultTransport {
    fn request(&self, port: u16, mut command: Value) -> Result<Value, RuntimeError> {
        if command.get("PatchConfig").is_some() {
            let patch_number = self.patches.fetch_add(1, Ordering::SeqCst) + 1;
            if matches!(self.mode, FaultMode::RollbackRejected) {
                command["PatchConfig"]["pipeline"] = if patch_number == 1 {
                    json!([])
                } else {
                    json!("invalid")
                };
                return LocalWebSocketTransport.request(port, command);
            }
        }
        if command.get("PatchConfig").is_some() && self.armed.swap(false, Ordering::SeqCst) {
            match self.mode {
                FaultMode::Invalid => command["PatchConfig"]["pipeline"] = json!("invalid"),
                FaultMode::Semantic => command["PatchConfig"]["pipeline"] = json!([]),
                FaultMode::LostReply | FaultMode::Timeout => {
                    LocalWebSocketTransport.request(port, command)?;
                    if matches!(self.mode, FaultMode::Timeout) {
                        thread::sleep(Duration::from_millis(2_100));
                    }
                    return Err(if matches!(self.mode, FaultMode::Timeout) {
                        RuntimeError::RequestTimeout
                    } else {
                        RuntimeError::WebSocketConnectFailed
                    });
                }
                FaultMode::RealSocketDrop => {
                    let stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
                        .map_err(|_| RuntimeError::WebSocketConnectFailed)?;
                    let (mut socket, _) = client(format!("ws://127.0.0.1:{port}/"), stream)
                        .map_err(|_| RuntimeError::WebSocketConnectFailed)?;
                    socket
                        .send(Message::Text(command.to_string().into()))
                        .map_err(|_| RuntimeError::WebSocketClosed)?;
                    socket
                        .get_mut()
                        .shutdown(Shutdown::Both)
                        .map_err(|_| RuntimeError::WebSocketClosed)?;
                    let error = socket
                        .read()
                        .expect_err("dropped WebSocket returned a reply");
                    println!("real PatchConfig socket drop: tungstenite {error:?}");
                    return Err(RuntimeError::WebSocketClosed);
                }
                FaultMode::RollbackRejected => unreachable!(),
            }
        }
        LocalWebSocketTransport.request(port, command)
    }
}

fn rollback_signal_probe(probe: &mut Probe, session: u64, mpd_name: &str) -> Result<(), String> {
    let baseline = route_identity(probe)?;
    let controller = probe
        .dsp
        .runtime_controller()
        .map_err(|e| format!("controller: {e:?}"))?;
    let format = PcmFormat {
        sample_rate_hz: 48_000,
        channels: 2,
    };
    let flat = flat_preset();
    let mut a = flat.clone();
    a.id = "rollback_a".into();
    a.bands.truncate(1);
    a.bands[0].frequency_hz = 1000.0;
    a.bands[0].gain_db = 6.0;
    let mut b = a.clone();
    b.id = "rollback_b".into();
    b.bands[0].gain_db = -6.0;
    controller
        .apply_preset(session, &flat, format)
        .map_err(|e| format!("Flat: {e:?}"))?;
    let (_, a_rms, _) = record_transition(probe, "rollback_flat_a", || {
        controller
            .apply_preset(session, &a, format)
            .map_err(|e| format!("A: {e:?}"))
    })?;
    for (label, mode, expected) in [
        ("invalid", FaultMode::Invalid, RuntimeError::ApplyRejected),
        (
            "semantic",
            FaultMode::Semantic,
            RuntimeError::VerificationFailed,
        ),
        (
            "lost_reply",
            FaultMode::LostReply,
            RuntimeError::WebSocketConnectFailed,
        ),
        (
            "real_socket_drop",
            FaultMode::RealSocketDrop,
            RuntimeError::WebSocketClosed,
        ),
        ("timeout", FaultMode::Timeout, RuntimeError::RequestTimeout),
    ] {
        let fault = CamillaRuntimeController::new(
            probe
                .dsp
                .runtime_endpoint
                .clone()
                .ok_or("endpoint missing")?,
            Arc::clone(&probe.dsp.activation_generation),
            Arc::clone(&probe.dsp.runtime_shared),
            FaultTransport {
                mode,
                armed: AtomicBool::new(true),
                patches: AtomicUsize::new(0),
            },
        );
        let (before, after, _) = record_transition(probe, &format!("rollback_{label}"), || {
            let result = fault.apply_preset(session, &b, format);
            if result != Err(expected.clone()) {
                return Err(format!("{label}: unexpected apply result {result:?}"));
            }
            Ok(())
        })?;
        if (before / a_rms - 1.0).abs() > 0.03 || (after / a_rms - 1.0).abs() > 0.03 {
            return Err(format!("{label}: A signal not restored: A={a_rms:.1}, before={before:.1}, after={after:.1}"));
        }
        if controller
            .published_preset()
            .map_err(|e| format!("published: {e:?}"))?
            != Some(a.id.clone())
        {
            return Err(format!("{label}: B published"));
        }
        if route_identity(probe)? != baseline {
            return Err(format!("{label}: route changed"));
        }
        assert_transition_gain_zero(probe)?;
        no_hardware_fallback(probe, mpd_name)?;
        println!("rollback {label}: A RMS={a_rms:.1}, before={before:.1}, after={after:.1}, PID/nodes/4 links stable");
    }
    if std::env::var("SONANTE_DSP_ROLLBACK_FAIL").as_deref() == Ok("1") {
        let fault = CamillaRuntimeController::new(
            probe
                .dsp
                .runtime_endpoint
                .clone()
                .ok_or("endpoint missing")?,
            Arc::clone(&probe.dsp.activation_generation),
            Arc::clone(&probe.dsp.runtime_shared),
            FaultTransport {
                mode: FaultMode::RollbackRejected,
                armed: AtomicBool::new(true),
                patches: AtomicUsize::new(0),
            },
        );
        let (before, after, _) = record_transition(probe, "rollback_rejected", || {
            let result = fault.apply_preset(session, &b, format);
            if result != Err(RuntimeError::RollbackFailed) {
                return Err(format!("rollback rejection: {result:?}"));
            }
            Ok(())
        })?;
        if after > before / 100.0 {
            return Err(format!(
                "rollback failure did not attenuate signal: {before:.1}->{after:.1}"
            ));
        }
        let gain = LocalWebSocketTransport
            .request(
                probe
                    .dsp
                    .runtime_endpoint
                    .as_ref()
                    .ok_or("endpoint missing")?
                    .port,
                json!("GetVolume"),
            )
            .map_err(|e| format!("GetVolume: {e:?}"))?;
        if gain.pointer("/GetVolume/value").and_then(Value::as_f64) != Some(-60.0) {
            return Err(format!("rollback failure left unknown gain: {gain}"));
        }
        if controller
            .published_preset()
            .map_err(|e| format!("published: {e:?}"))?
            != Some(a.id.clone())
            || controller.apply_preset(session, &a, format) != Err(RuntimeError::RuntimePoisoned)
        {
            return Err("rollback failure published or accepted another apply".into());
        }
        if controller.health() != RuntimeHealth::Poisoned {
            return Err("rollback failure did not poison runtime".into());
        }
        if route_identity(probe)? != baseline {
            return Err("rollback failure changed route".into());
        }
        no_hardware_fallback(probe, mpd_name)?;
        println!("rollback rejected: A RMS={before:.1}, fail-safe RMS={after:.1}, RollbackFailed, no new apply or fallback");
    }
    Ok(())
}

struct CrashTransport<'a> {
    child: Mutex<&'a mut Child>,
    armed: AtomicBool,
}

impl RuntimeTransport for CrashTransport<'_> {
    fn request(&self, port: u16, command: Value) -> Result<Value, RuntimeError> {
        if command.get("PatchConfig").is_some() && self.armed.swap(false, Ordering::SeqCst) {
            self.child
                .lock()
                .map_err(|_| RuntimeError::RuntimeUnavailable)?
                .kill()
                .map_err(|_| RuntimeError::RuntimeUnavailable)?;
            thread::sleep(Duration::from_millis(20));
            return LocalWebSocketTransport.request(port, command);
        }
        LocalWebSocketTransport.request(port, command)
    }
}

fn crash_during_apply_probe(probe: &mut Probe, session: u64, mpd_name: &str) -> Result<(), String> {
    let endpoint = probe
        .dsp
        .runtime_endpoint
        .clone()
        .ok_or("endpoint missing")?;
    let generation = Arc::clone(&probe.dsp.activation_generation);
    let shared = Arc::clone(&probe.dsp.runtime_shared);
    let child = probe.dsp.process.as_mut().ok_or("Camilla child missing")?;
    let controller = CamillaRuntimeController::new(
        endpoint,
        generation,
        shared,
        CrashTransport {
            child: Mutex::new(child),
            armed: AtomicBool::new(true),
        },
    );
    let result = controller.apply_preset(
        session,
        &flat_preset(),
        PcmFormat {
            sample_rate_hz: 48_000,
            channels: 2,
        },
    );
    if result != Err(RuntimeError::RollbackFailed) {
        return Err(format!("crash during apply: {result:?}"));
    }
    if controller
        .published_preset()
        .map_err(|e| format!("published: {e:?}"))?
        != Some("rollback_a".into())
    {
        return Err("crashed apply published preset".into());
    }
    if probe.dsp.check_process() != Err(super::DspError::CamillaExited) {
        return Err("supervisor did not identify dead Camilla".into());
    }
    no_hardware_fallback(probe, mpd_name)?;
    println!("crash during apply: RollbackFailed, A remains published, supervisor reports CamillaExited, no fallback");
    Ok(())
}

impl Drop for Probe {
    fn drop(&mut self) {
        if self.root.exists() {
            if let Err(error) = self.close() {
                eprintln!("EQ-1C cleanup: {error}");
            }
        }
    }
}

fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut condition: F) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while !condition() {
        if Instant::now() >= deadline {
            return Err("timed out".into());
        }
        thread::sleep(Duration::from_millis(30));
    }
    Ok(())
}

#[test]
#[ignore = "requires SONANTE_DSP_HARNESS=1, CamillaDSP, MPD and a PipeWire session"]
fn real_dsp_lifecycle_null_sink_only() -> Result<(), String> {
    if std::env::var("SONANTE_DSP_HARNESS").as_deref() != Ok("1") {
        return Err("set SONANTE_DSP_HARNESS=1 to opt in".into());
    }
    let binary = super::resolve_camilla_binary(None).map_err(|e| format!("CamillaDSP: {e:?}"))?;
    for tool in ["mpd", "pw-dump", "pw-link", "pactl"] {
        let output = command(
            tool,
            &[if tool == "mpd" {
                "--version"
            } else if tool == "pactl" {
                "info"
            } else {
                "--help"
            }],
        )?;
        if tool == "mpd" {
            let version = String::from_utf8_lossy(&output.stdout);
            if !version.contains("pipewire") {
                return Err("MPD has no PipeWire output plugin".into());
            }
            println!(
                "MPD under test: {} (PipeWire output available)",
                version.lines().next().unwrap_or("unknown")
            );
        }
    }
    let default_before = default_sink()?;
    let mut probe = Probe::new()?;
    let music = probe.root.join("music");
    fs::create_dir(&music).map_err(|e| e.to_string())?;
    write_wav(&music.join("real_track.wav"))?;
    let camilla_start = Instant::now();
    let (session, sink_name) = probe
        .dsp
        .start(Some(&binary))
        .map_err(|e| format!("DSP start: {e:?}"))?;
    println!(
        "Camilla/runtime initialization: {:.3}s",
        camilla_start.elapsed().as_secs_f64()
    );
    let capture = probe
        .dsp
        .capture_node_name()
        .ok_or("capture name missing")?
        .to_owned();
    let module = command(
        "pactl",
        &[
            "load-module",
            "module-null-sink",
            &format!("sink_name={sink_name}"),
            "sink_properties=device.description=SonanteEQ1CNull",
        ],
    )?;
    probe.null_module = Some(String::from_utf8_lossy(&module.stdout).trim().to_owned());
    if default_sink()? != default_before {
        return Err("null sink changed the default sink".into());
    }
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let sink =
        exact_node(&graph, &sink_name, "Audio/Sink").map_err(|e| format!("null sink: {e:?}"))?;
    if !sink.virtual_sink {
        return Err("selected sink is not virtual".into());
    }
    println!("before MPD play: null sink={sink_name}, default sink={default_before}, DSP session={session}");

    let output_name = format!("EQ1C guarded {} {session}", std::process::id());
    let mpd_name = format!("mpd.{output_name}");
    let socket = probe.root.join("mpd.sock");
    let output =
        dsp_mpd_output_config(&output_name, &capture).map_err(|e| format!("output: {e:?}"))?;
    let config = format!("music_directory \"{}\"\nplaylist_directory \"{}\"\ndb_file \"{}\"\nlog_file \"{}\"\npid_file \"{}\"\nbind_to_address \"{}\"\nauto_update \"no\"\nreplaygain \"off\"\n{}\n",
        music.display(), probe.root.display(), probe.root.join("db").display(), probe.root.join("mpd.log").display(), probe.root.join("mpd.pid").display(), socket.display(), output);
    let config_path = probe.root.join("mpd.conf");
    fs::write(&config_path, config).map_err(|e| e.to_string())?;
    probe.mpd = Some(
        dsp_mpd_command(Path::new("mpd"), &config_path)
            .spawn()
            .map_err(|e| format!("MPD spawn: {e}"))?,
    );
    let mut player = MpdSocketControl::new(socket);
    wait_for(Duration::from_secs(3), || player.snapshot().is_ok())?;
    player
        .command("update", "update")
        .map_err(|e| format!("update: {e:?}"))?;
    wait_for(Duration::from_secs(3), || {
        player
            .command("status", "status")
            .is_ok_and(|lines| !lines.iter().any(|line| line.starts_with("updating_db:")))
    })?;
    player
        .command("add", "add \"real_track.wav\"")
        .map_err(|e| format!("add: {e:?}"))?;
    let queue_before = player
        .command("playlistinfo", "playlistinfo")
        .map_err(|e| format!("queue: {e:?}"))?;
    let song_id = queue_before
        .iter()
        .find_map(|line| {
            line.strip_prefix("Id: ")
                .and_then(|id| id.parse::<u32>().ok())
        })
        .ok_or("missing song ID")?;
    if queue_before
        .iter()
        .filter(|line| line.starts_with("file: "))
        .count()
        != 1
        || !queue_before
            .iter()
            .any(|line| line == "file: real_track.wav")
    {
        return Err("test queue is not exactly its WAV".into());
    }
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    if graph.nodes.iter().any(|node| node.name == mpd_name)
        || graph.links.iter().any(|link| {
            graph.ports.iter().any(|port| {
                (port.node_id == sink.id
                    || port.node_id
                        == exact_node(&graph, &capture, "Stream/Input/Audio")
                            .map(|n| n.id)
                            .unwrap_or(0))
                    && (port.id == link.input_port || port.id == link.output_port)
            })
        })
    {
        return Err("unexpected node or link before first play".into());
    }
    let transaction =
        DspActivationTransaction::new(song_id, mpd_name.clone(), probe.dsp.activation_gate());
    let first_trace = ActivationTrace::new();
    transaction
        .execute(
            &mut TracedPlayback {
                inner: &mut player,
                trace: first_trace.clone(),
            },
            &mut TracedRoute {
                inner: &mut probe.dsp,
                trace: first_trace.clone(),
            },
        )
        .map_err(|e| format!("first activation: {e:?}"))?;
    first_trace.print("first")?;
    let first = player.snapshot().map_err(|e| format!("status: {e:?}"))?;
    let graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let mpd = exact_node(&graph, &mpd_name, "Stream/Output/Audio")
        .map_err(|e| format!("MPD node: {e:?}"))?;
    if mpd.autoconnect != Some(false)
        || probe.dsp.route_manager.owned_links().len() != 4
        || graph.links.iter().any(|link| {
            probe.dsp.route_manager.owned_links().contains(&link.id)
                && !probe.dsp.route.as_ref().is_some_and(|route| {
                    validate_topology(&graph, route, true) == RouteStatus::RouteReady
                })
        })
    {
        return Err("unsafe active route".into());
    }
    if first.state != PlayerState::Playing || first.elapsed > 1.5 {
        return Err(format!("start position not restored: {first:?}"));
    }
    println!(
        "first activation: elapsed={:.3}s, links=4, node.autoconnect=false",
        first.elapsed
    );
    let queue_after = player
        .command("playlistinfo", "playlistinfo")
        .map_err(|e| format!("queue: {e:?}"))?;
    if queue_after != queue_before {
        return Err("queue changed".into());
    }

    runtime_switch_probe(&mut probe, session)?;
    websocket_control_probe(&mut probe, session)?;
    if std::env::var("SONANTE_DSP_EQ3D_FOCUS").as_deref() == Ok("1") {
        pure_pipewire_mpd_probe(&mut probe, &music, &sink_name, &mpd_name)?;
    }
    if std::env::var("SONANTE_DSP_EQ3D_FOCUS").as_deref() != Ok("1") {
        if std::env::var("SONANTE_DSP_HARNESS_FAST").as_deref() != Ok("1") {
            signal_probe(&mut probe, session, false)?;
            signal_probe(&mut probe, session, true)?;
        }
        rollback_signal_probe(&mut probe, session, &mpd_name)?;
    }
    if std::env::var("SONANTE_DSP_EQ3C_ONLY").as_deref() == Ok("1") {
        no_hardware_fallback(&mut probe, &mpd_name)?;
        if player
            .command("playlistinfo", "playlistinfo")
            .map_err(|e| format!("queue: {e:?}"))?
            != queue_before
        {
            return Err("EQ-3C changed the queue".into());
        }
        if std::env::var("SONANTE_DSP_ROLLBACK_FAIL").as_deref() != Ok("1") {
            crash_during_apply_probe(&mut probe, session, &mpd_name)?;
        }
        probe.close()?;
        if default_sink()? != default_before {
            return Err("default sink changed after EQ-3C cleanup".into());
        }
        return Ok(());
    }

    let _ = probe
        .dsp
        .monitor
        .as_mut()
        .ok_or("monitor missing")?
        .event_pending();
    player.pause().map_err(|e| format!("pause: {e:?}"))?;
    thread::sleep(Duration::from_millis(100));
    let paused = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("pause snapshot: {e:?}"))?;
    println!(
        "pause link states: {:?}",
        paused
            .links
            .iter()
            .filter(|l| probe.dsp.route_manager.owned_links().contains(&l.id))
            .map(|l| l.state)
            .collect::<Vec<_>>()
    );
    let pause_event = probe
        .dsp
        .monitor
        .as_mut()
        .ok_or("monitor missing")?
        .event_pending()
        .map_err(|e| format!("pause monitor: {e:?}"))?;
    println!("pw-link monitor event after pause: {pause_event}");
    probe
        .dsp
        .on_graph_event(session, PlayerState::Paused)
        .map_err(|e| format!("pause observation: {e:?}"))?;
    probe
        .dsp
        .drain_monitor(PlayerState::Paused)
        .map_err(|e| format!("pause monitor: {e:?}"))?;
    println!("DSP after pause: {:?}", probe.dsp.state());
    player.resume().map_err(|e| format!("resume: {e:?}"))?;
    thread::sleep(Duration::from_millis(100));
    let resume_event = probe
        .dsp
        .monitor
        .as_mut()
        .ok_or("monitor missing")?
        .event_pending()
        .map_err(|e| format!("resume monitor: {e:?}"))?;
    println!("pw-link monitor event after resume: {resume_event}");
    if let Err(error) = wait_for(Duration::from_secs(2), || {
        player.seek_id(song_id, 3.0).is_ok()
    }) {
        return Err(format!(
            "seek nonzero: {error}; status={:?}; log={}",
            player.snapshot(),
            fs::read_to_string(probe.root.join("mpd.log")).unwrap_or_default()
        ));
    }
    let before = player
        .snapshot()
        .map_err(|e| format!("snapshot nonzero: {e:?}"))?;
    let position_anchor = Instant::now();
    if before.elapsed < 3.0 {
        return Err(format!("nonzero snapshot too early: {before:?}"));
    }
    // Disconnect only the four owned links after the real MPD stream is running.
    probe
        .dsp
        .route_manager
        .remove_owned_links()
        .map_err(|e| format!("disconnect: {e:?}"))?;
    probe.dsp.route.as_mut().ok_or("route missing")?.mpd_name = None;
    probe.dsp.route_status = RouteStatus::CamillaReady;
    probe
        .dsp
        .state
        .transition(DspState::Starting)
        .map_err(|e| format!("prepare again: {e:?}"))?;
    let second_session = session;
    let second_graph = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("snapshot: {e:?}"))?;
    let existing = exact_node(&second_graph, &mpd_name, "Stream/Output/Audio")
        .map_err(|e| format!("existing MPD node: {e:?}"))?;
    if existing.autoconnect != Some(false) {
        return Err("existing MPD node autoconnected".into());
    }
    no_hardware_fallback(&mut probe, &mpd_name)?;
    let transaction =
        DspActivationTransaction::new(song_id, mpd_name.clone(), probe.dsp.activation_gate());
    let second_trace = ActivationTrace::new();
    transaction
        .execute(
            &mut TracedPlayback {
                inner: &mut player,
                trace: second_trace.clone(),
            },
            &mut TracedRoute {
                inner: &mut probe.dsp,
                trace: second_trace.clone(),
            },
        )
        .map_err(|e| format!("existing stream activation: {e:?}"))?;
    second_trace.print("existing")?;
    let after = player
        .snapshot()
        .map_err(|e| format!("status nonzero: {e:?}"))?;
    let position_error = after.elapsed - before.elapsed;
    let elapsed_wall = position_anchor.elapsed().as_secs_f64();
    let semantic_start = second_trace.first_status()?;
    println!(
        "existing stream activation: before={:.3}s, transaction_start={:.3}s, after={:.3}s, advance={:+.3}s, wall={:.3}s",
        before.elapsed, semantic_start.elapsed, after.elapsed, position_error, elapsed_wall
    );
    // MPD reports decoder/output progress in coarse chunks on this host. For
    // an already-playing stream the semantic invariant is monotonic progress
    // without replay, pause or seek while the queue and song ID stay fixed.
    if after.elapsed < semantic_start.elapsed
        || semantic_start.elapsed < before.elapsed
        || second_trace.contains("playid intent")?
        || second_trace.contains("pause intent")?
        || second_trace.contains("seek intent")?
        || second_trace.contains("resume intent")?
    {
        return Err("existing stream lost position".into());
    }
    if player
        .command("playlistinfo", "playlistinfo")
        .map_err(|e| format!("queue: {e:?}"))?
        != queue_before
    {
        return Err("queue changed on second activation".into());
    }

    player
        .seek_id(song_id, 30.0)
        .map_err(|e| format!("seek 30 playing: {e:?}"))?;
    reactivation_probe(
        &mut probe,
        &mut player,
        song_id,
        &mpd_name,
        &queue_before,
        "playing near 30",
        PlayerState::Playing,
    )?;
    player.pause().map_err(|e| format!("pause 30: {e:?}"))?;
    player
        .seek_id(song_id, 30.0)
        .map_err(|e| format!("seek 30 paused: {e:?}"))?;
    reactivation_probe(
        &mut probe,
        &mut player,
        song_id,
        &mpd_name,
        &queue_before,
        "paused near 30",
        PlayerState::Paused,
    )?;
    player.pause().map_err(|e| format!("pause 3: {e:?}"))?;
    player
        .seek_id(song_id, 3.0)
        .map_err(|e| format!("seek 3 paused: {e:?}"))?;
    reactivation_probe(
        &mut probe,
        &mut player,
        song_id,
        &mpd_name,
        &queue_before,
        "paused near 3",
        PlayerState::Paused,
    )?;

    // Kill only the Child owned by this supervisor, then check fail-closed behavior.
    probe
        .dsp
        .process
        .as_mut()
        .ok_or("Camilla child missing")?
        .kill()
        .map_err(|e| e.to_string())?;
    wait_for(Duration::from_secs(2), || {
        probe.dsp.check_process().is_err()
    })?;
    if !matches!(probe.dsp.state(), DspState::Failed(_)) {
        return Err("Camilla crash stayed Active".into());
    }
    no_hardware_fallback(&mut probe, &mpd_name)?;
    println!("Camilla crash: {:?}, no MPD fallback", probe.dsp.state());

    let third_session = probe.restart_dsp(&binary, &default_before)?;
    let transaction =
        DspActivationTransaction::new(song_id, mpd_name.clone(), probe.dsp.activation_gate());
    transaction
        .execute(&mut player, &mut probe.dsp)
        .map_err(|e| format!("third activation: {e:?}"))?;
    exercise_stop_play(
        &mut player,
        probe.mpd.as_mut().ok_or("MPD child missing")?,
        song_id,
        "Camilla route",
    )?;
    player.stop().map_err(|e| format!("stop: {e:?}"))?;
    thread::sleep(Duration::from_millis(100));
    let stop_event = probe
        .dsp
        .monitor
        .as_mut()
        .ok_or("monitor missing")?
        .event_pending()
        .map_err(|e| format!("stop monitor: {e:?}"))?;
    println!("pw-link monitor event after stop: {stop_event}");
    let stopped = probe
        .dsp
        .route_manager
        .snapshot()
        .map_err(|e| format!("stop snapshot: {e:?}"))?;
    println!(
        "stop: mpd node present={}, owned links present={}, states={:?}",
        stopped.nodes.iter().any(|n| n.name == mpd_name),
        stopped
            .links
            .iter()
            .filter(|l| probe.dsp.route_manager.owned_links().contains(&l.id))
            .count(),
        stopped
            .links
            .iter()
            .filter(|l| probe.dsp.route_manager.owned_links().contains(&l.id))
            .map(|l| l.state)
            .collect::<Vec<_>>()
    );
    probe
        .dsp
        .on_graph_event(third_session, PlayerState::Stopped)
        .map_err(|e| format!("stop observation: {e:?}"))?;
    if matches!(probe.dsp.state(), DspState::Failed(_)) {
        return Err("normal stop failed DSP".into());
    }
    let module = probe.null_module.take().ok_or("null module missing")?;
    command("pactl", &["unload-module", &module])?;
    probe
        .dsp
        .on_graph_event(third_session, PlayerState::Stopped)
        .map_err(|e| format!("sink event: {e:?}"))?;
    if !matches!(probe.dsp.state(), DspState::Failed(_)) {
        return Err("sink disappearance stayed Active".into());
    }
    no_hardware_fallback(&mut probe, &mpd_name)?;
    println!(
        "sink disappearance: {:?}, no MPD fallback",
        probe.dsp.state()
    );
    probe
        .dsp
        .on_graph_event(second_session, PlayerState::Playing)
        .map_err(|e| format!("stale event: {e:?}"))?;
    probe.close()?;
    if default_sink()? != default_before {
        return Err("default sink changed after cleanup".into());
    }
    let mut inspector = PipeWireRouteManager::new(SystemCommandRunner);
    let marker = format!("sonante_dsp_{}", std::process::id());
    let null_marker = format!("sonante_dsp_null_{}", std::process::id());
    let pure_mpd_name = format!("mpd.EQ3D pure {}", std::process::id());
    wait_for(Duration::from_secs(2), || {
        inspector.snapshot().is_ok_and(|graph| {
            !graph.nodes.iter().any(|node| {
                node.name.starts_with(&marker)
                    || node.name.starts_with(&null_marker)
                    || node.name == mpd_name
                    || node.name == pure_mpd_name
            })
        })
    })?;
    if probe.root.exists()
        || probe.mpd.is_some()
        || probe.pure_mpd.is_some()
        || probe.dsp.process.is_some()
    {
        return Err("owned process or temporary runtime remains".into());
    }
    println!("cleanup: no EQ-1C processes, nodes, links or runtime files remain");
    Ok(())
}
