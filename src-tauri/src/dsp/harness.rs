//! Explicit host-only lifecycle probe: cargo test dsp::harness -- --ignored --nocapture
use super::activation::{
    dsp_mpd_command, dsp_mpd_output_config, DspActivationTransaction, FirstStreamPlayback,
    MpdSocketControl, PlayerState,
};
use super::pipewire::{
    exact_node, validate_topology, PipeWireRouteManager, RouteStatus, SystemCommandRunner,
};
use super::{DspState, DspSupervisor};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    let samples = 48000_u32 * 30;
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    file.write_all(b"RIFF").map_err(|error| error.to_string())?;
    file.write_all(&(36 + samples * 4).to_le_bytes())
        .map_err(|error| error.to_string())?;
    file.write_all(b"WAVEfmt \x10\x00\x00\x00\x01\x00\x02\x00\x80\xbb\x00\x00\x00\xee\x02\x00\x04\x00\x10\x00data")
        .map_err(|error| error.to_string())?;
    file.write_all(&(samples * 4).to_le_bytes())
        .map_err(|error| error.to_string())?;
    for index in 0..samples {
        let sample = if index % 48 < 24 {
            8_000_i16
        } else {
            -8_000_i16
        };
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
    null_module: Option<String>,
    dsp: DspSupervisor<SystemCommandRunner>,
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
        let _ = command(
            tool,
            &[if tool == "mpd" {
                "--version"
            } else if tool == "pactl" {
                "info"
            } else {
                "--help"
            }],
        )?;
    }
    let default_before = default_sink()?;
    let mut probe = Probe::new()?;
    let music = probe.root.join("music");
    fs::create_dir(&music).map_err(|e| e.to_string())?;
    write_wav(&music.join("real_track.wav"))?;
    let (session, sink_name) = probe
        .dsp
        .start(Some(&binary))
        .map_err(|e| format!("DSP start: {e:?}"))?;
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
    transaction
        .execute(&mut player, &mut probe.dsp)
        .map_err(|e| format!("first activation: {e:?}"))?;
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
    transaction
        .execute(&mut player, &mut probe.dsp)
        .map_err(|e| format!("existing stream activation: {e:?}"))?;
    let after = player
        .snapshot()
        .map_err(|e| format!("status nonzero: {e:?}"))?;
    let position_error = after.elapsed - before.elapsed;
    println!(
        "existing stream activation: before={:.3}s, after={:.3}s, error={:+.3}s",
        before.elapsed, after.elapsed, position_error
    );
    if after.elapsed < 3.0 || position_error.abs() > 1.5 {
        return Err("existing stream lost position".into());
    }
    if player
        .command("playlistinfo", "playlistinfo")
        .map_err(|e| format!("queue: {e:?}"))?
        != queue_before
    {
        return Err("queue changed on second activation".into());
    }

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
    wait_for(Duration::from_secs(2), || {
        inspector.snapshot().is_ok_and(|graph| {
            !graph.nodes.iter().any(|node| {
                node.name.starts_with(&marker)
                    || node.name.starts_with(&null_marker)
                    || node.name == mpd_name
            })
        })
    })?;
    if probe.root.exists() || probe.mpd.is_some() || probe.dsp.process.is_some() {
        return Err("owned process or temporary runtime remains".into());
    }
    println!("cleanup: no EQ-1C processes, nodes, links or runtime files remain");
    Ok(())
}
