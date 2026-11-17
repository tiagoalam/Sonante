//! PipeWire observation and links for this owned Camilla playback stream only.
use super::diagnostics;
use super::Error;
use serde_json::Value;
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Default)]
pub(crate) struct Environment {
    pub vars: Vec<(String, String)>,
}
impl Environment {
    pub fn command(&self, program: &str) -> Command {
        let fixed;
        let program =
            if !cfg!(debug_assertions) && matches!(program, "pw-dump" | "pw-link" | "pw-cli") {
                fixed = format!("/usr/bin/{program}");
                fixed.as_str()
            } else {
                program
            };
        let mut c = Command::new(program);
        for (k, v) in &self.vars {
            c.env(k, v);
        }
        c
    }
    pub fn run(&self, program: &str, args: &[&str]) -> Result<Vec<u8>, Error> {
        let started = Instant::now();
        let mut child = self
            .command(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| {
                diagnostics::event(
                    "pipewire.command",
                    "error",
                    Some(started.elapsed().as_millis()),
                    serde_json::json!({"command": safe_command(program), "reason": "spawn_failed"}),
                );
                Error::PipeWireUnavailable
            })?;
        let stdout = child.stdout.take().ok_or(Error::PipeWireUnavailable)?;
        let reader = thread::spawn(move || {
            let mut b = Vec::new();
            stdout.take(16 * 1024 * 1024 + 1).read_to_end(&mut b).ok();
            b
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let bytes = reader.join().map_err(|_| Error::PipeWireUnavailable)?;
        if bytes.len() > 16 * 1024 * 1024 || !status.is_some_and(|s| s.success()) {
            diagnostics::event(
                "pipewire.command",
                "error",
                Some(started.elapsed().as_millis()),
                serde_json::json!({
                    "command": safe_command(program),
                    "reason": if bytes.len() > 16 * 1024 * 1024 { "output_limit" } else if status.is_none() { "timeout" } else { "nonzero_exit" },
                    "exit_code": status.and_then(|s| s.code()),
                    "output_bytes": bytes.len(),
                }),
            );
            return Err(Error::PipeWireUnavailable);
        }
        // Graph snapshots are polled rapidly while the session is being
        // published. Persist their failures and the final graph result, but
        // avoid synchronous disk writes for every successful `pw-dump` poll.
        if safe_command(program) != "pw-dump" {
            diagnostics::event(
                "pipewire.command",
                "ok",
                Some(started.elapsed().as_millis()),
                serde_json::json!({"command": safe_command(program), "output_bytes": bytes.len()}),
            );
        }
        Ok(bytes)
    }
    pub fn graph(&self) -> Result<Graph, Error> {
        Graph::parse(&self.run("pw-dump", &[])?)
    }

    /// Rebuild the live topology for a session whose USB/AUX sink was fully
    /// admitted at startup. Recheck that same PipeWire and ALSA identity, but
    /// do not reinterpret the kernel's entire capability report on every
    /// health poll while the PCM is active.
    pub fn graph_for_session_sink(&self, admitted_sink: &Node) -> Result<Graph, Error> {
        Graph::parse_for_admitted_sink(&self.run("pw-dump", &[])?, admitted_sink)
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub id: u32,
    pub serial: u64,
    pub name: String,
    pub class: String,
    pub pid: Option<u32>,
    /// Explicit channel layout published by PipeWire for this node.
    /// AUX ports are not inherently left/right; this metadata is required
    /// before their index can be interpreted as a stereo position.
    pub audio_channels: Option<usize>,
    pub audio_positions: Vec<String>,
    pub usb_playback: Option<UsbPlayback>,
    pub usb_stereo_verified: bool,
    /// Sanitized reason the independent ALSA proof failed. This is kept with
    /// the same PipeWire snapshot so graph diagnostics can report the exact
    /// failed predicate before teardown changes the environment.
    pub usb_stereo_failure: Option<&'static str>,
    pub usb_proof_observation: Option<UsbProofObservation>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct UsbProofObservation {
    /// Sanitized ALSA/PipeWire numeric coordinates and the effective
    /// channel-map override observed in the same pw-dump snapshot.
    alsa_card_index: Option<u32>,
    alsa_pcm_device: Option<u32>,
    pipewire_device_id: Option<u32>,
    device_alsa_card_index: Option<u32>,
    chmap_property_count: usize,
    chmap_disabled: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UsbPlayback {
    card: u32,
    device: u32,
    pw_device_id: u32,
    pw_device_serial: u64,
    card_id: String,
    usb_id: String,
}
#[derive(Clone)]
pub(crate) struct Port {
    pub id: u32,
    pub node: u32,
    pub name: String,
    pub channel: String,
    pub output: bool,
    pub index: Option<u32>,
    pub physical_terminal: bool,
}
#[derive(Clone)]
struct Link {
    out: u32,
    input: u32,
    active: bool,
}
pub(crate) struct Graph {
    pub nodes: Vec<Node>,
    pub ports: Vec<Port>,
    links: Vec<Link>,
    default: Option<String>,
}
fn number(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
fn audio_positions(value: &Value) -> Vec<String> {
    let raw = match value {
        Value::Array(values) => {
            return values
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default()
                .into_iter()
                .map(str::to_ascii_uppercase)
                .collect();
        }
        Value::String(raw) => raw.as_str(),
        _ => return Vec::new(),
    };
    let trimmed = raw.trim().trim_start_matches('[').trim_end_matches(']');
    trimmed
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
}

fn chmap_observation(info: &Value) -> (usize, Option<bool>) {
    let mut count = 0usize;
    let mut only_value = None;
    if let Some(entries) = info["params"]["Props"].as_array() {
        for pair in entries
            .iter()
            .filter_map(|entry| entry["params"].as_array())
            .flat_map(|params| params.chunks_exact(2))
            .filter(|pair| pair[0] == "api.alsa.use-chmap")
        {
            count = count.saturating_add(1);
            if count == 1 {
                only_value = pair[1].as_bool();
            }
        }
    }
    (count, only_value)
}

fn usb_proof_observation(
    p: &Value,
    info: &Value,
    devices: &std::collections::HashMap<u32, Value>,
) -> UsbProofObservation {
    let pipewire_device_id = number(&p["device.id"]).and_then(|id| u32::try_from(id).ok());
    let device_alsa_card_index = pipewire_device_id
        .and_then(|id| devices.get(&id))
        .and_then(|props| number(&props["alsa.card"]))
        .and_then(|card| u32::try_from(card).ok());
    let (chmap_property_count, chmap_disabled) = chmap_observation(info);
    UsbProofObservation {
        alsa_card_index: number(&p["alsa.card"]).and_then(|card| u32::try_from(card).ok()),
        alsa_pcm_device: number(&p["alsa.device"]).and_then(|device| u32::try_from(device).ok()),
        pipewire_device_id,
        device_alsa_card_index,
        chmap_property_count,
        chmap_disabled,
    }
}

// The USB-audio driver's streamN is created for the same PCM device N. The
// kernel reports the physical channel map for each playback alternate setting.
// A PipeWire Pro sink may expose only the first two channels as AUX0/AUX1 even
// when the USB endpoint also advertises multichannel alternate settings. Prove
// that those first two hardware slots remain FL/FR in every playback setting;
// requiring every setting to contain exactly two channels rejects such devices
// without adding any useful evidence about the selected two-channel node.
fn is_playback_altsetting_header(line: &str) -> bool {
    line.strip_prefix("Interface ")
        .is_some_and(|value| value.parse::<u32>().is_ok())
}

fn usb_playback_report_is_stereo(report: &str) -> Result<(), &'static str> {
    let mut playback = false;
    let mut block = false;
    let mut channels: Option<&str> = None;
    let mut map: Option<&str> = None;
    let mut format = None;
    let mut formats = 0usize;
    let mut stereo_formats = 0usize;
    let mut pcm_found = false;
    for line in report.lines().chain(std::iter::once("Capture:")) {
        let line = line.trim();
        if line == "Playback:" {
            if playback || block {
                return Err("alsa_playback_section_ambiguous");
            }
            playback = true;
            continue;
        }
        // `/proc/asound/.../streamN` also reports the active stream as
        // `Interface = N` under `Status: Running`. That is status metadata,
        // not the capability block header `Interface N`; treating it as a
        // block starts an empty record and falsely reports missing Channels.
        if line == "Capture:" || (playback && is_playback_altsetting_header(line)) {
            if block {
                let channel_count = match channels {
                    None => return Err("alsa_playback_channels_missing"),
                    Some(value) => value
                        .parse::<usize>()
                        .map_err(|_| "alsa_playback_channels_invalid")?,
                };
                let channel_map = map.ok_or("alsa_playback_channel_map_not_fl_fr")?;
                let positions = channel_map
                    .split_whitespace()
                    .map(str::to_ascii_uppercase)
                    .collect::<Vec<_>>();
                if positions.len() != channel_count {
                    return Err("alsa_playback_channel_map_channel_count_mismatch");
                }
                let unique = positions.iter().collect::<std::collections::HashSet<_>>();
                if unique.len() != positions.len() {
                    return Err("alsa_playback_channel_map_ambiguous");
                }
                // Mono alternate settings cannot define a stereo pair and do
                // not contradict the selected two-channel PipeWire node. Every
                // setting with at least two channels must keep FL/FR in slots
                // zero and one; malformed, duplicated or reversed maps fail.
                if channel_count >= 2
                    && (positions.first().map(String::as_str) != Some("FL")
                        || positions.get(1).map(String::as_str) != Some("FR"))
                {
                    return Err("alsa_playback_channel_map_not_fl_fr");
                }
                if format.is_none() {
                    return Err("alsa_playback_format_missing");
                }
                if channel_count == 2 {
                    stereo_formats += 1;
                    pcm_found |= format.is_some_and(|value: &str| {
                        value
                            .split_whitespace()
                            .any(|f| f == "S16_LE" || f == "S32_LE")
                    });
                }
                formats += 1;
            }
            if line == "Capture:" {
                break;
            }
            block = true;
            channels = None;
            map = None;
            format = None;
        } else if playback && block {
            if let Some(value) = line.strip_prefix("Format:") {
                if value.trim().is_empty() || format.replace(value.trim()).is_some() {
                    return Err("alsa_playback_format_ambiguous");
                }
            } else if let Some(value) = line.strip_prefix("Channels:") {
                if channels.replace(value.trim()).is_some() {
                    return Err("alsa_playback_channels_ambiguous");
                }
            } else if let Some(value) = line.strip_prefix("Channel map:") {
                if map.replace(value.trim()).is_some() {
                    return Err("alsa_playback_channel_map_ambiguous");
                }
            }
        }
    }
    if !playback {
        return Err("alsa_playback_section_missing");
    }
    if formats == 0 {
        return Err("alsa_playback_interface_missing");
    }
    if stereo_formats == 0 {
        return Err("alsa_playback_stereo_setting_missing");
    }
    if !pcm_found {
        return Err("alsa_playback_pcm_format_missing");
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcReadFailure {
    NotFound,
    PermissionDenied,
    InvalidUtf8,
    TooLarge,
    Io,
}

fn read_small_file(path: &std::path::Path, limit: u64) -> Result<String, ProcReadFailure> {
    let mut output = String::new();
    let file = std::fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ProcReadFailure::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcReadFailure::PermissionDenied,
        _ => ProcReadFailure::Io,
    })?;
    file.take(limit + 1)
        .read_to_string(&mut output)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::InvalidData => ProcReadFailure::InvalidUtf8,
            std::io::ErrorKind::PermissionDenied => ProcReadFailure::PermissionDenied,
            _ => ProcReadFailure::Io,
        })?;
    if output.len() as u64 > limit {
        return Err(ProcReadFailure::TooLarge);
    }
    Ok(output)
}

fn proc_read_reason(file: &'static str, failure: ProcReadFailure) -> &'static str {
    match (file, failure) {
        ("card_id", ProcReadFailure::NotFound) => "alsa_card_id_not_found",
        ("card_id", ProcReadFailure::PermissionDenied) => "alsa_card_id_permission_denied",
        ("card_id", ProcReadFailure::InvalidUtf8) => "alsa_card_id_invalid_utf8",
        ("card_id", ProcReadFailure::TooLarge) => "alsa_card_id_too_large",
        ("card_id", ProcReadFailure::Io) => "alsa_card_id_io_error",
        ("usb_id", ProcReadFailure::NotFound) => "alsa_usb_id_not_found",
        ("usb_id", ProcReadFailure::PermissionDenied) => "alsa_usb_id_permission_denied",
        ("usb_id", ProcReadFailure::InvalidUtf8) => "alsa_usb_id_invalid_utf8",
        ("usb_id", ProcReadFailure::TooLarge) => "alsa_usb_id_too_large",
        ("usb_id", ProcReadFailure::Io) => "alsa_usb_id_io_error",
        ("stream", ProcReadFailure::NotFound) => "alsa_stream_not_found",
        ("stream", ProcReadFailure::PermissionDenied) => "alsa_stream_permission_denied",
        ("stream", ProcReadFailure::InvalidUtf8) => "alsa_stream_invalid_utf8",
        ("stream", ProcReadFailure::TooLarge) => "alsa_stream_too_large",
        ("stream", ProcReadFailure::Io) => "alsa_stream_io_error",
        ("card_id_after_read", ProcReadFailure::NotFound) => "alsa_card_id_disappeared_after_read",
        ("card_id_after_read", ProcReadFailure::PermissionDenied) => {
            "alsa_card_id_permission_denied_after_read"
        }
        ("card_id_after_read", ProcReadFailure::InvalidUtf8) => {
            "alsa_card_id_invalid_utf8_after_read"
        }
        ("card_id_after_read", ProcReadFailure::TooLarge) => "alsa_card_id_too_large_after_read",
        ("card_id_after_read", ProcReadFailure::Io) => "alsa_card_id_io_error_after_read",
        ("usb_id_after_read", ProcReadFailure::NotFound) => "alsa_usb_id_disappeared_after_read",
        ("usb_id_after_read", ProcReadFailure::PermissionDenied) => {
            "alsa_usb_id_permission_denied_after_read"
        }
        ("usb_id_after_read", ProcReadFailure::InvalidUtf8) => {
            "alsa_usb_id_invalid_utf8_after_read"
        }
        ("usb_id_after_read", ProcReadFailure::TooLarge) => "alsa_usb_id_too_large_after_read",
        ("usb_id_after_read", ProcReadFailure::Io) => "alsa_usb_id_io_error_after_read",
        _ => "alsa_proc_read_error",
    }
}

fn usb_kernel_stereo_proof(endpoint: &UsbPlayback) -> Result<(), &'static str> {
    usb_kernel_stereo_proof_at(std::path::Path::new("/proc/asound"), endpoint)
}

fn usb_kernel_stereo_proof_at(
    root: &std::path::Path,
    endpoint: &UsbPlayback,
) -> Result<(), &'static str> {
    usb_kernel_stereo_proof_with(root, endpoint, read_small_file)
}

fn usb_kernel_identity_proof(endpoint: &UsbPlayback) -> Result<(), &'static str> {
    usb_kernel_identity_proof_at(std::path::Path::new("/proc/asound"), endpoint)
}

fn usb_kernel_identity_proof_at(
    root: &std::path::Path,
    endpoint: &UsbPlayback,
) -> Result<(), &'static str> {
    usb_kernel_identity_proof_with(root, endpoint, read_small_file)
}

fn usb_kernel_identity_proof_with(
    root: &std::path::Path,
    endpoint: &UsbPlayback,
    mut read: impl FnMut(&std::path::Path, u64) -> Result<String, ProcReadFailure>,
) -> Result<(), &'static str> {
    let card = root.join(format!("card{}", endpoint.card));
    let id = read(&card.join("id"), 256).map_err(|failure| proc_read_reason("card_id", failure))?;
    let usb_id = read(&card.join("usbid"), 256).map_err(|failure| proc_read_reason("usb_id", failure))?;
    if id.trim() != endpoint.card_id {
        return Err("alsa_card_identity_mismatch");
    }
    if !usb_id.trim().eq_ignore_ascii_case(&endpoint.usb_id) {
        return Err("alsa_usb_identity_mismatch");
    }

    // Check both identifiers again so a card replacement during the live
    // topology read cannot inherit the admission proof from the old device.
    let final_id = read(&card.join("id"), 256)
        .map_err(|failure| proc_read_reason("card_id_after_read", failure))?;
    if final_id.trim() != endpoint.card_id {
        return Err("alsa_card_identity_changed");
    }
    let final_usb_id = read(&card.join("usbid"), 256)
        .map_err(|failure| proc_read_reason("usb_id_after_read", failure))?;
    if !final_usb_id.trim().eq_ignore_ascii_case(&endpoint.usb_id) {
        return Err("alsa_usb_identity_changed");
    }
    Ok(())
}

fn usb_kernel_stereo_proof_with(
    root: &std::path::Path,
    endpoint: &UsbPlayback,
    mut read: impl FnMut(&std::path::Path, u64) -> Result<String, ProcReadFailure>,
) -> Result<(), &'static str> {
    let card = root.join(format!("card{}", endpoint.card));
    let id = read(&card.join("id"), 256).map_err(|failure| proc_read_reason("card_id", failure))?;
    let usb_id =
        read(&card.join("usbid"), 256).map_err(|failure| proc_read_reason("usb_id", failure))?;
    if id.trim() != endpoint.card_id {
        return Err("alsa_card_identity_mismatch");
    }
    if !usb_id.trim().eq_ignore_ascii_case(&endpoint.usb_id) {
        return Err("alsa_usb_identity_mismatch");
    }
    let report = read(&card.join(format!("stream{}", endpoint.device)), 65536)
        .map_err(|failure| proc_read_reason("stream", failure))?;
    usb_playback_report_is_stereo(&report)?;

    // Re-read the identity after the stream report. A card removed and
    // replaced while the report was being inspected must fail closed.
    let final_id = read(&card.join("id"), 256)
        .map_err(|failure| proc_read_reason("card_id_after_read", failure))?;
    if final_id.trim() != endpoint.card_id {
        return Err("alsa_card_identity_changed");
    }
    let final_usb_id = read(&card.join("usbid"), 256)
        .map_err(|failure| proc_read_reason("usb_id_after_read", failure))?;
    if !final_usb_id.trim().eq_ignore_ascii_case(&endpoint.usb_id) {
        return Err("alsa_usb_identity_changed");
    }
    Ok(())
}

fn usb_playback_identity(
    p: &Value,
    info: &Value,
    devices: &std::collections::HashMap<u32, Value>,
) -> Result<UsbPlayback, &'static str> {
    // A disabled ALSA channel-map override preserves the hardware's native
    // channel order. Require an explicit, unambiguous value from Props.
    let (chmap_count, chmap_disabled) = chmap_observation(info);
    if chmap_count == 0 {
        return Err("pipewire_chmap_property_missing");
    }
    if chmap_count != 1 {
        return Err("pipewire_chmap_property_ambiguous");
    }
    if chmap_disabled != Some(false) {
        return Err("pipewire_chmap_override_not_disabled");
    }
    if p["alsa.driver_name"] != "snd_usb_audio" {
        return Err("pipewire_alsa_driver_not_usb_audio");
    }
    if p["api.alsa.pcm.stream"] != "playback" {
        return Err("pipewire_pcm_not_playback");
    }
    if p["device.bus"] != "usb" {
        return Err("pipewire_device_bus_not_usb");
    }
    if number(&p["alsa.subdevice"]) != Some(0) {
        return Err("pipewire_subdevice_not_zero");
    }
    let card = u32::try_from(number(&p["alsa.card"]).ok_or("pipewire_alsa_card_missing")?)
        .map_err(|_| "pipewire_alsa_card_invalid")?;
    let device = u32::try_from(number(&p["alsa.device"]).ok_or("pipewire_alsa_device_missing")?)
        .map_err(|_| "pipewire_alsa_device_invalid")?;
    let expected_path = format!("hw:{card},{device}");
    if card > 255 || device > 255 {
        return Err("pipewire_alsa_coordinates_out_of_range");
    }
    if number(&p["api.alsa.pcm.card"]) != Some(card.into()) {
        return Err("pipewire_alsa_pcm_card_mismatch");
    }
    if p["api.alsa.path"].as_str() != Some(expected_path.as_str()) {
        return Err("pipewire_alsa_pcm_path_mismatch");
    }
    let card_id = p["alsa.id"]
        .as_str()
        .ok_or("pipewire_alsa_card_identity_missing")?;
    if card_id.is_empty()
        || card_id.len() > 32
        || !card_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err("pipewire_alsa_card_identity_invalid");
    }
    let usb_id = p["alsa.components"]
        .as_str()
        .ok_or("pipewire_usb_identity_missing")?
        .strip_prefix("USB")
        .ok_or("pipewire_usb_identity_invalid")?;
    if usb_id.len() != 9
        || usb_id.as_bytes()[4] != b':'
        || !usb_id
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 4 || c.is_ascii_hexdigit())
    {
        return Err("pipewire_usb_identity_invalid");
    }
    let pw_device_id =
        u32::try_from(number(&p["device.id"]).ok_or("pipewire_device_reference_missing")?)
            .map_err(|_| "pipewire_device_reference_invalid")?;
    let device_props = devices
        .get(&pw_device_id)
        .ok_or("pipewire_device_object_missing")?;
    let pw_device_serial =
        number(&device_props["object.serial"]).ok_or("pipewire_device_serial_missing")?;
    if number(&device_props["alsa.card"]) != Some(card.into()) {
        return Err("pipewire_device_card_mismatch");
    }
    if device_props["alsa.driver_name"] != "snd_usb_audio" {
        return Err("pipewire_device_driver_mismatch");
    }
    if device_props["device.bus"] != "usb" {
        return Err("pipewire_device_bus_mismatch");
    }
    if device_props["alsa.id"].as_str() != Some(card_id) {
        return Err("pipewire_device_alsa_id_mismatch");
    }
    if device_props["alsa.components"] != p["alsa.components"] {
        return Err("pipewire_device_usb_id_mismatch");
    }
    Ok(UsbPlayback {
        card,
        device,
        pw_device_id,
        pw_device_serial,
        card_id: card_id.into(),
        usb_id: usb_id.into(),
    })
}
impl Graph {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        Self::parse_with_usb_proof(bytes, usb_kernel_stereo_proof)
    }
    fn parse_with_usb_proof(
        bytes: &[u8],
        usb_proof: impl Fn(&UsbPlayback) -> Result<(), &'static str>,
    ) -> Result<Self, Error> {
        Self::parse_with_scoped_usb_proof(bytes, None, |_, endpoint| {
            Some(usb_proof(endpoint))
        })
    }

    fn parse_for_admitted_sink(bytes: &[u8], admitted_sink: &Node) -> Result<Self, Error> {
        Self::parse_with_admitted_sink_proof(bytes, admitted_sink, usb_kernel_identity_proof)
    }

    fn parse_with_admitted_sink_proof(
        bytes: &[u8],
        admitted_sink: &Node,
        identity_proof: impl Fn(&UsbPlayback) -> Result<(), &'static str>,
    ) -> Result<Self, Error> {
        Self::parse_with_scoped_usb_proof(
            bytes,
            Some(admitted_sink.name.as_str()),
            |name, endpoint| {
                if name != admitted_sink.name {
                    return None;
                }
                Some(match admitted_sink.usb_playback.as_ref() {
                    Some(_) if !admitted_sink.usb_stereo_verified => {
                        Err("alsa_stereo_admission_missing")
                    }
                    Some(admitted) if admitted == endpoint => identity_proof(endpoint),
                    Some(_) => Err("alsa_playback_identity_changed"),
                    None => Err("alsa_stereo_admission_missing"),
                })
            },
        )
    }

    fn parse_with_scoped_usb_proof(
        bytes: &[u8],
        diagnostic_sink: Option<&str>,
        mut usb_proof: impl FnMut(&str, &UsbPlayback) -> Option<Result<(), &'static str>>,
    ) -> Result<Self, Error> {
        let objects: Vec<Value> = serde_json::from_slice(bytes).map_err(|_| {
            diagnostics::graph_event(
                "dump_invalid_json",
                serde_json::json!({"graph": {"parseable": false, "input_bytes": bytes.len()}}),
            );
            Error::GraphInvalid
        })?;
        if objects.len() > 100_000 {
            diagnostics::graph_event(
                "dump_object_limit",
                serde_json::json!({"graph": {"parseable": true, "object_count": objects.len()}}),
            );
            return Err(Error::GraphInvalid);
        }
        let clients = objects
            .iter()
            .filter(|v| v["type"] == "PipeWire:Interface:Client")
            .filter_map(|v| {
                Some((
                    number(&v["id"])? as u32,
                    number(&v["info"]["props"]["application.process.id"])? as u32,
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let devices = objects
            .iter()
            .filter(|v| v["type"] == "PipeWire:Interface:Device")
            .filter_map(|v| {
                Some((
                    u32::try_from(number(&v["id"])?).ok()?,
                    v["info"]["props"].clone(),
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();
        let mut g = Self {
            nodes: vec![],
            ports: vec![],
            links: vec![],
            default: None,
        };
        for v in objects {
            let Some(id) = number(&v["id"]).and_then(|i| u32::try_from(i).ok()) else {
                continue;
            };
            let p = &v["info"]["props"];
            match v["type"].as_str() {
                Some("PipeWire:Interface:Node") => {
                    if let (Some(name), Some(class), Some(serial)) = (
                        p["node.name"].as_str(),
                        p["media.class"].as_str(),
                        number(&p["object.serial"]),
                    ) {
                        let pid = number(&p["application.process.id"])
                            .map(|n| n as u32)
                            .or_else(|| {
                                number(&p["client.id"])
                                    .and_then(|n| clients.get(&(n as u32)).copied())
                            });
                        let aux_usb_candidate = class == "Audio/Sink"
                            && audio_positions(&p["audio.position"]) == ["AUX0", "AUX1"];
                        let usb_proof_observation = aux_usb_candidate
                            .then(|| usb_proof_observation(p, &v["info"], &devices));
                        let (usb_playback, usb_stereo_verified, usb_stereo_failure) =
                            if aux_usb_candidate {
                                match usb_playback_identity(p, &v["info"], &devices) {
                                    Ok(endpoint) => match usb_proof(name, &endpoint) {
                                        Some(Ok(())) => (Some(endpoint), true, None),
                                        Some(Err(reason)) => (Some(endpoint), false, Some(reason)),
                                        None => (Some(endpoint), false, None),
                                    },
                                    Err(reason) => (None, false, Some(reason)),
                                }
                            } else {
                                (None, false, None)
                            };
                        g.nodes.push(Node {
                            id,
                            serial,
                            name: name.into(),
                            class: class.into(),
                            pid,
                            audio_channels: number(&p["audio.channels"])
                                .and_then(|count| usize::try_from(count).ok()),
                            audio_positions: audio_positions(&p["audio.position"]),
                            usb_playback,
                            usb_stereo_verified,
                            usb_stereo_failure,
                            usb_proof_observation,
                        });
                    }
                }
                Some("PipeWire:Interface:Port") => {
                    if let (Some(node), Some(name), Some(channel), Some(dir)) = (
                        number(&p["node.id"]).and_then(|n| u32::try_from(n).ok()),
                        p["port.name"].as_str(),
                        p["audio.channel"].as_str(),
                        p["port.direction"].as_str(),
                    ) {
                        let output = match dir {
                            "out" => true,
                            "in" => false,
                            _ => continue,
                        };
                        g.ports.push(Port {
                            id,
                            node,
                            name: name.into(),
                            channel: channel.into(),
                            output,
                            index: number(&p["port.id"]).and_then(|n| u32::try_from(n).ok()),
                            physical_terminal: p["port.physical"] == true
                                && p["port.terminal"] == true,
                        });
                    }
                }
                Some("PipeWire:Interface:Link") => {
                    if let (Some(out), Some(input)) = (
                        number(&v["info"]["output-port-id"]),
                        number(&v["info"]["input-port-id"]),
                    ) {
                        g.links.push(Link {
                            out: out as u32,
                            input: input as u32,
                            active: v["info"]["state"] == "active",
                        });
                    }
                }
                Some("PipeWire:Interface:Metadata") => {
                    if let Some(items) = v["metadata"]
                        .as_array()
                        .or_else(|| v["info"]["metadata"].as_array())
                    {
                        for item in items {
                            if item["key"] == "default.audio.sink" {
                                let value = if let Some(s) = item["value"].as_str() {
                                    serde_json::from_str(s).unwrap_or(Value::Null)
                                } else {
                                    item["value"].clone()
                                };
                                if let Some(name) = value["name"].as_str() {
                                    g.default = Some(name.into());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        // Report the selected sink only. Unselected AUX-capable devices are
        // not route candidates and must not flood the session health log.
        let selected_sink_name = diagnostic_sink
            .or(g.default.as_deref())
            .or_else(|| {
                let mut sinks = g.nodes.iter().filter(|node| node.class == "Audio/Sink");
                let only_sink = sinks.next()?;
                sinks.next().is_none().then_some(only_sink.name.as_str())
            });
        for node in g.nodes.iter().filter(|node| {
            node.class == "Audio/Sink"
                && node.audio_positions == ["AUX0", "AUX1"]
                && node.usb_stereo_failure.is_some()
                && selected_sink_name == Some(node.name.as_str())
        }) {
            if let Some(reason) = node.usb_stereo_failure {
                diagnostics::graph_event(reason, g.sanitized_summary(None, Some(node)));
            }
        }
        Ok(g)
    }
    pub fn node(&self, name: &str, class: &str) -> Result<&Node, Error> {
        let mut nodes = self
            .nodes
            .iter()
            .filter(|n| n.name == name && n.class == class);
        let n = nodes.next().ok_or(Error::NodeMissing)?;
        if nodes.next().is_some() {
            diagnostics::graph_event(
                "node_ambiguous",
                serde_json::json!({"class": class, "matching_nodes": self.nodes.iter().filter(|n| n.name == name && n.class == class).count()}),
            );
            return Err(Error::GraphInvalid);
        }
        Ok(n)
    }
    pub fn default_sink(&self) -> Result<Node, Error> {
        if let Some(name) = &self.default {
            return match self.node(name, "Audio/Sink") {
                Ok(node) => Ok(node.clone()),
                Err(error) => {
                    if error == Error::NodeMissing {
                        diagnostics::graph_event(
                            "default_sink_target_missing",
                            serde_json::json!({"default_present": true, "audio_sink_count": self.nodes.iter().filter(|n| n.class == "Audio/Sink").count()}),
                        );
                    } else if error == Error::GraphInvalid {
                        diagnostics::graph_event(
                            "default_sink_target_ambiguous",
                            serde_json::json!({"default_present": true, "audio_sink_count": self.nodes.iter().filter(|n| n.class == "Audio/Sink").count()}),
                        );
                    }
                    Err(error)
                }
            };
        }
        let mut sinks = self.nodes.iter().filter(|n| n.class == "Audio/Sink");
        let Some(n) = sinks.next() else {
            diagnostics::graph_event("default_sink_missing", self.sanitized_summary(None, None));
            return Err(Error::SinkUnavailable);
        };
        if sinks.next().is_some() {
            diagnostics::graph_event(
                "default_sink_ambiguous",
                serde_json::json!({"graph": {"sink_count": self.nodes.iter().filter(|node| node.class == "Audio/Sink").count(), "default_present": false}}),
            );
            return Err(Error::SinkUnavailable);
        }
        Ok(n.clone())
    }
    pub fn stereo(&self, node: &Node, output: bool) -> Result<[Port; 2], Error> {
        let role = if node.class == "Audio/Sink" {
            "selected_sink"
        } else {
            "owned_stream"
        };
        let direction = if output { "output" } else { "input" };
        let relevant = self
            .ports
            .iter()
            .filter(|port| port.node == node.id && port.output == output)
            .collect::<Vec<_>>();
        let exact = |channel: &str| {
            let mut matches = relevant
                .iter()
                .copied()
                .filter(|port| port.channel == channel);
            let first = matches.next().cloned();
            (first, matches.next().is_none())
        };

        let (left, left_unique) = exact("FL");
        let (right, right_unique) = exact("FR");
        if left.is_some() && right.is_some() {
            if left_unique && right_unique {
                // If the node publishes its own two-channel layout, reject
                // contradictory metadata instead of trusting inconsistent
                // port and node maps.
                if node.audio_channels == Some(2)
                    && !node.audio_positions.is_empty()
                    && node.audio_positions != ["FL", "FR"]
                {
                    diagnostics::graph_event(
                        "stereo_channel_map_conflict",
                        serde_json::json!({"role": role, "direction": direction, "node_id": node.id, "graph": self.sanitized_summary(Some(node), None)}),
                    );
                    return Err(Error::GraphInvalid);
                }
                return Ok([left.unwrap(), right.unwrap()]);
            }
            diagnostics::graph_event(
                "stereo_port_ambiguous",
                serde_json::json!({"role": role, "direction": direction, "node_id": node.id, "graph": self.sanitized_summary(Some(node), None)}),
            );
            return Err(Error::GraphInvalid);
        }
        if !left_unique || !right_unique {
            diagnostics::graph_event(
                "stereo_port_ambiguous",
                serde_json::json!({"role": role, "direction": direction, "node_id": node.id, "graph": self.sanitized_summary(Some(node), None)}),
            );
            return Err(Error::GraphInvalid);
        }

        // PipeWire AUX channels are generic/unpositioned. Accept AUX0/AUX1
        // only when the selected sink independently declares exactly two
        // channels, the associated USB PCM proves that hardware slots 0/1 are
        // FL/FR, and this direction exposes exactly those two indexed ports.
        // The canonical index order is required; reversed or partial layouts
        // remain unsupported rather than guessed.
        let aux0 = relevant
            .iter()
            .copied()
            .filter(|port| port.channel == "AUX0")
            .collect::<Vec<_>>();
        let aux1 = relevant
            .iter()
            .copied()
            .filter(|port| port.channel == "AUX1")
            .collect::<Vec<_>>();
        let aux_is_exact_pair = relevant.len() == 2 && aux0.len() == 1 && aux1.len() == 1;
        let explicit_pipewire_stereo = node.audio_positions == ["FL", "FR"];
        let verified_usb_stereo = node.class == "Audio/Sink"
            && !output
            && node.audio_positions == ["AUX0", "AUX1"]
            && node.usb_stereo_verified
            && aux_is_exact_pair
            && aux0[0].index == Some(0)
            && aux1[0].index == Some(1)
            && aux0[0].physical_terminal
            && aux1[0].physical_terminal;
        if aux_is_exact_pair
            && node.audio_channels == Some(2)
            && (explicit_pipewire_stereo || verified_usb_stereo)
        {
            return Ok([aux0[0].clone(), aux1[0].clone()]);
        }

        let reason = if aux0.len() + aux1.len() > 0 {
            if !aux_is_exact_pair {
                "stereo_aux_mapping_ambiguous"
            } else {
                "stereo_aux_mapping_unproven"
            }
        } else {
            "stereo_port_missing"
        };
        diagnostics::graph_event(
            reason,
            serde_json::json!({"role": role, "direction": direction, "node_id": node.id, "audio_channels": node.audio_channels, "audio_positions": node.audio_positions, "usb_stereo_failure": node.usb_stereo_failure, "graph": self.sanitized_summary(Some(node), None)}),
        );
        Err(Error::GraphInvalid)
    }
    pub fn validate(&self, stream: &Node, sink: &Node) -> Result<bool, Error> {
        // The route is explicitly linked to `sink`; a later change to
        // PipeWire's session-wide default is not a redirect by itself. The
        // checks below still require the original sink identity, ports and
        // exact active links, and fail if the route actually changes.
        let actual = self
            .node(&stream.name, "Stream/Output/Audio")
            .map_err(|error| {
                if error == Error::NodeMissing {
                    diagnostics::graph_event(
                        "owned_stream_missing",
                        self.sanitized_summary(Some(stream), Some(sink)),
                    );
                }
                error
            })?;
        let dest = self.node(&sink.name, "Audio/Sink").map_err(|error| {
            if error == Error::NodeMissing {
                diagnostics::graph_event(
                    "selected_sink_missing",
                    self.sanitized_summary(Some(stream), Some(sink)),
                );
            }
            error
        })?;
        if actual.serial != stream.serial
            || actual.pid != stream.pid
            || dest.serial != sink.serial
            || dest.usb_playback != sink.usb_playback
            || dest.usb_stereo_verified != sink.usb_stereo_verified
        {
            diagnostics::graph_event(
                "node_identity_changed",
                serde_json::json!({
                    "stream_serial_matches": actual.serial == stream.serial,
                    "stream_pid_matches": actual.pid == stream.pid,
                    "sink_serial_matches": dest.serial == sink.serial,
                    "graph": self.sanitized_summary(Some(stream), Some(sink)),
                }),
            );
            return Err(Error::GraphInvalid);
        }
        let outputs = self.stereo(actual, true)?;
        let inputs = self.stereo(dest, false)?;
        let mut active = true;
        let mut inactive_channels = Vec::new();
        for i in 0..2 {
            let links = self
                .links
                .iter()
                .filter(|l| l.out == outputs[i].id)
                .collect::<Vec<_>>();
            if links.len() != 1 || links[0].input != inputs[i].id {
                diagnostics::graph_event(
                    "links_missing_duplicate_or_wrong",
                    serde_json::json!({
                        "channel": outputs[i].channel,
                        "expected_input_port_id": inputs[i].id,
                        "observed_link_count": links.len(),
                        "observed_input_port_ids": links.iter().map(|link| link.input).collect::<Vec<_>>(),
                        "graph": self.sanitized_summary(Some(stream), Some(sink)),
                    }),
                );
                return Err(Error::GraphInvalid);
            }
            active &= links[0].active;
            if !links[0].active {
                inactive_channels.push(outputs[i].channel.as_str());
            }
        }
        if !active {
            diagnostics::graph_event(
                "links_present_but_inactive",
                serde_json::json!({
                    "inactive_channels": inactive_channels,
                    "graph": self.sanitized_summary(Some(stream), Some(sink)),
                }),
            );
        }
        Ok(active)
    }
    fn connection_plan(&self, stream: &Node, sink: &Node) -> Result<Vec<(Port, Port)>, Error> {
        if self.default.as_ref().is_some_and(|name| name != &sink.name) {
            diagnostics::graph_event(
                "default_sink_changed_before_connect",
                self.sanitized_summary(Some(stream), Some(sink)),
            );
            return Err(Error::GraphInvalid);
        }
        let current_stream = self
            .node(&stream.name, "Stream/Output/Audio")
            .map_err(|error| {
                if error == Error::NodeMissing {
                    diagnostics::graph_event(
                        "owned_stream_missing_before_connect",
                        self.sanitized_summary(Some(stream), Some(sink)),
                    );
                }
                error
            })?;
        let current_sink = self.node(&sink.name, "Audio/Sink").map_err(|error| {
            if error == Error::NodeMissing {
                diagnostics::graph_event(
                    "selected_sink_missing_before_connect",
                    self.sanitized_summary(Some(stream), Some(sink)),
                );
            }
            error
        })?;
        if current_stream.serial != stream.serial
            || current_stream.pid != stream.pid
            || current_sink.serial != sink.serial
            || current_sink.usb_playback != sink.usb_playback
            || current_sink.usb_stereo_verified != sink.usb_stereo_verified
        {
            diagnostics::graph_event(
                "node_identity_changed_before_connect",
                serde_json::json!({
                    "stream_serial_matches": current_stream.serial == stream.serial,
                    "stream_pid_matches": current_stream.pid == stream.pid,
                    "sink_serial_matches": current_sink.serial == sink.serial,
                    "graph": self.sanitized_summary(Some(stream), Some(sink)),
                }),
            );
            return Err(Error::GraphInvalid);
        }
        let outputs = self.stereo(current_stream, true)?;
        let inputs = self.stereo(current_sink, false)?;
        let mut missing = Vec::with_capacity(2);
        for i in 0..2 {
            let links = self
                .links
                .iter()
                .filter(|link| link.out == outputs[i].id)
                .collect::<Vec<_>>();
            match links.as_slice() {
                [] => missing.push((outputs[i].clone(), inputs[i].clone())),
                [link] if link.input == inputs[i].id => {}
                // Never accept a duplicate, cross-channel, or foreign route.
                _ => {
                    diagnostics::graph_event(
                        "link_conflict_before_connect",
                        serde_json::json!({
                            "channel": outputs[i].channel,
                            "expected_input_port_id": inputs[i].id,
                            "observed_link_count": links.len(),
                            "observed_input_port_ids": links.iter().map(|link| link.input).collect::<Vec<_>>(),
                            "graph": self.sanitized_summary(Some(stream), Some(sink)),
                        }),
                    );
                    return Err(Error::GraphInvalid);
                }
            }
        }
        Ok(missing)
    }
    pub fn connect(&self, env: &Environment, stream: &Node, sink: &Node) -> Result<(), Error> {
        // PipeWire session managers may create the exact requested links before
        // this process runs pw-link. Treat those links as already applied, but
        // reject every conflicting or ambiguous route.
        let plan = self.connection_plan(stream, sink)?;
        diagnostics::event(
            "pipewire.connection_plan",
            "ok",
            None,
            serde_json::json!({
                "stream_node_id": stream.id, "sink_node_id": sink.id,
                "existing_or_planned_links": 2, "links_to_create": plan.len(),
                "channels": plan.iter().map(|(out, _)| out.channel.as_str()).collect::<Vec<_>>(),
            }),
        );
        for (output, input) in plan {
            let out = endpoint(&stream.name, &output.name)?;
            let input_port_id = input.id;
            let input = endpoint(&sink.name, &input.name)?;
            let started = Instant::now();
            diagnostics::event(
                "pipewire.link_apply",
                "started",
                Some(started.elapsed().as_millis()),
                serde_json::json!({"channel": output.channel, "output_port_id": output.id, "input_port_id": input_port_id}),
            );
            if let Err(error) = env.run("pw-link", &[&out, &input]) {
                diagnostics::event(
                    "pipewire.link_apply",
                    "error",
                    Some(started.elapsed().as_millis()),
                    serde_json::json!({"channel": output.channel, "output_port_id": output.id, "input_port_id": input_port_id, "code": "pw_link_failed"}),
                );
                return Err(error);
            }
            diagnostics::event(
                "pipewire.link_created",
                "ok",
                Some(started.elapsed().as_millis()),
                serde_json::json!({"channel": output.channel}),
            );
        }
        Ok(())
    }

    /// Keep only PipeWire object identity and channel/link topology; never write
    /// node names, device descriptions, application names or file paths.
    pub(crate) fn sanitized_summary(&self, stream: Option<&Node>, sink: Option<&Node>) -> Value {
        let mut selected = std::collections::BTreeSet::new();
        if let Some(node) = stream {
            selected.insert(node.id);
        }
        if let Some(node) = sink {
            selected.insert(node.id);
        }
        let stream_port_ids = self
            .ports
            .iter()
            .filter(|port| stream.is_some_and(|n| port.node == n.id) && port.output)
            .map(|port| port.id)
            .collect::<std::collections::BTreeSet<_>>();
        let relevant_links = self
            .links
            .iter()
            .filter(|link| stream_port_ids.contains(&link.out))
            .collect::<Vec<_>>();
        let nodes = self.nodes.iter().filter(|node| selected.contains(&node.id)).map(|node| serde_json::json!({
            "role": if node.class == "Audio/Sink" { "selected_sink" } else { "owned_stream" },
            "id": node.id, "serial": node.serial, "class": match node.class.as_str() { "Audio/Sink" => "audio_sink", "Stream/Output/Audio" => "audio_output_stream", _ => "other" },
            "pid": if node.class == "Stream/Output/Audio" { node.pid } else { None },
            "audio_channels": node.audio_channels,
            "audio_positions": node.audio_positions,
            "usb_stereo_verified": node.usb_stereo_verified,
            "usb_playback_identity_verified": node.usb_playback.is_some(),
            "usb_stereo_failure": node.usb_stereo_failure,
            "usb_proof_observation": node.usb_proof_observation.as_ref().map(|sample| serde_json::json!({
                "alsa_card_index": sample.alsa_card_index,
                "alsa_pcm_device": sample.alsa_pcm_device,
                "pipewire_device_id": sample.pipewire_device_id,
                "device_alsa_card_index": sample.device_alsa_card_index,
                "chmap_property_count": sample.chmap_property_count,
                "chmap_disabled": sample.chmap_disabled,
            })),
            "alsa_card_index": node.usb_playback.as_ref().map(|endpoint| endpoint.card),
            "alsa_pcm_device": node.usb_playback.as_ref().map(|endpoint| endpoint.device),
        })).collect::<Vec<_>>();
        let ports = self.ports.iter().filter(|port| selected.contains(&port.node)).map(|port| serde_json::json!({
            "id": port.id, "node_id": port.node, "direction": if port.output { "output" } else { "input" }, "channel": port.channel,
            "index": port.index, "physical_terminal": port.physical_terminal,
        })).collect::<Vec<_>>();
        serde_json::json!({
            "parseable": true,
            "object_counts": {"nodes": self.nodes.len(), "ports": self.ports.len(), "links": self.links.len()},
            "default_sink_present": self.default.is_some(),
            // `stereo()` can be called with the selected sink as its `stream`
            // argument and no separate sink argument. Do not report a false
            // mismatch just because of that call shape.
            "default_sink_matches_selected": match (self.default.as_ref(), sink.or_else(|| stream.filter(|node| node.class == "Audio/Sink"))) { (Some(name), Some(sink)) => Some(name == &sink.name), (None, _) => None, _ => None },
            "nodes": nodes, "ports": ports,
            "links_from_owned_stream": relevant_links.iter().map(|link| serde_json::json!({"output_port_id": link.out, "input_port_id": link.input, "active": link.active})).collect::<Vec<_>>(),
        })
    }
}
fn endpoint(node: &str, port: &str) -> Result<String, Error> {
    if [node, port]
        .iter()
        .any(|s| s.is_empty() || s.contains(['\0', '\n', '\r', ':']))
    {
        diagnostics::graph_event(
            "unsafe_endpoint_rejected",
            serde_json::json!({"node_or_port_empty": node.is_empty() || port.is_empty(), "graph": {"endpoint_rejected": true}}),
        );
        return Err(Error::GraphInvalid);
    }
    Ok(format!("{node}:{port}"))
}

fn safe_command(program: &str) -> &'static str {
    match program.rsplit('/').next().unwrap_or(program) {
        "pw-dump" => "pw-dump",
        "pw-link" => "pw-link",
        "pw-cli" => "pw-cli",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Graph {
        Graph {
            nodes: vec![
                Node {
                    id: 1,
                    serial: 11,
                    name: "owned".into(),
                    class: "Stream/Output/Audio".into(),
                    pid: Some(100),
                    audio_channels: Some(2),
                    audio_positions: vec!["FL".into(), "FR".into()],
                    usb_playback: None,
                    usb_stereo_verified: false,
                    usb_stereo_failure: None,
                    usb_proof_observation: None,
                },
                Node {
                    id: 2,
                    serial: 22,
                    name: "sink".into(),
                    class: "Audio/Sink".into(),
                    pid: None,
                    audio_channels: Some(2),
                    audio_positions: vec!["FL".into(), "FR".into()],
                    usb_playback: None,
                    usb_stereo_verified: false,
                    usb_stereo_failure: None,
                    usb_proof_observation: None,
                },
            ],
            ports: [
                (3, 1, "FL", true),
                (4, 1, "FR", true),
                (5, 2, "FL", false),
                (6, 2, "FR", false),
            ]
            .into_iter()
            .map(|(id, node, c, output)| Port {
                id,
                node,
                name: c.into(),
                channel: c.into(),
                output,
                index: None,
                physical_terminal: false,
            })
            .collect(),
            links: vec![
                Link {
                    out: 3,
                    input: 5,
                    active: true,
                },
                Link {
                    out: 4,
                    input: 6,
                    active: true,
                },
            ],
            default: Some("sink".into()),
        }
    }
    #[test]
    fn route_requires_exact_owned_process_serial_channels_single_links_and_sink() {
        let mut g = fixture();
        let stream = g.nodes[0].clone();
        let sink = g.nodes[1].clone();
        assert_eq!(g.validate(&stream, &sink), Ok(true));
        g.links[0].active = false;
        assert_eq!(g.validate(&stream, &sink), Ok(false));
        g.links.push(Link {
            out: 3,
            input: 6,
            active: true,
        });
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
        g.links.pop();
        g.nodes[0].pid = Some(999);
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
        g.nodes[0] = stream.clone();
        g.links[0].active = true;
        g.default = Some("other".into());
        // A changed session preference does not redirect an explicitly linked
        // route. The exact sink identity and active links are still checked.
        assert_eq!(g.validate(&stream, &sink), Ok(true));
        g.default = Some("sink".into());
        g.nodes[1].serial += 1;
        assert_eq!(g.validate(&stream, &sink), Err(Error::GraphInvalid));
    }

    #[test]
    fn selected_route_survives_default_change_but_rejects_real_redirection() {
        let mut graph = fixture();
        let stream = graph.nodes[0].clone();
        let selected_sink = graph.nodes[1].clone();

        graph.default = Some("unrelated-default-sink".into());
        assert_eq!(graph.validate(&stream, &selected_sink), Ok(true));

        let mut redirected = fixture();
        let stream = redirected.nodes[0].clone();
        let selected_sink = redirected.nodes[1].clone();
        redirected.nodes.push(Node {
            id: 10,
            serial: 110,
            name: "other-sink".into(),
            class: "Audio/Sink".into(),
            pid: None,
            audio_channels: Some(2),
            audio_positions: vec!["FL".into(), "FR".into()],
            usb_playback: None,
            usb_stereo_verified: false,
            usb_stereo_failure: None,
            usb_proof_observation: None,
        });
        redirected.ports.push(Port {
            id: 10,
            node: 10,
            name: "FL".into(),
            channel: "FL".into(),
            output: false,
            index: Some(0),
            physical_terminal: true,
        });
        redirected.ports.push(Port {
            id: 11,
            node: 10,
            name: "FR".into(),
            channel: "FR".into(),
            output: false,
            index: Some(1),
            physical_terminal: true,
        });
        redirected.links[0].input = 10;
        assert_eq!(
            redirected.validate(&stream, &selected_sink),
            Err(Error::GraphInvalid)
        );
    }
    #[test]
    fn already_connected_exact_pipewire_route_is_accepted_without_relinking() {
        let mut g = fixture();
        g.links = vec![
            Link {
                out: 3,
                input: 5,
                active: true,
            },
            Link {
                out: 4,
                input: 6,
                active: true,
            },
        ];
        // Reproduces graph_invalid from the former connect() guard: an owned
        // session manager had already connected both channels to the selected sink.
        assert!(g
            .connection_plan(&g.nodes[0], &g.nodes[1])
            .unwrap()
            .is_empty());
        assert_eq!(g.validate(&g.nodes[0], &g.nodes[1]), Ok(true));
    }
    #[test]
    fn connection_plan_rejects_changed_default_or_replaced_endpoints() {
        let mut g = fixture();
        g.default = Some("other".into());
        assert!(matches!(
            g.connection_plan(&g.nodes[0], &g.nodes[1]),
            Err(Error::GraphInvalid)
        ));
        g.default = Some("sink".into());
        let mut replaced_sink = g.nodes[1].clone();
        replaced_sink.serial += 1;
        assert!(matches!(
            g.connection_plan(&g.nodes[0], &replaced_sink),
            Err(Error::GraphInvalid)
        ));
    }
    #[test]
    fn partially_connected_exact_route_only_plans_the_missing_channel() {
        let mut g = fixture();
        g.links = vec![Link {
            out: 3,
            input: 5,
            active: true,
        }];
        let plan = g.connection_plan(&g.nodes[0], &g.nodes[1]).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!((plan[0].0.id, plan[0].1.id), (4, 6));
    }
    #[test]
    fn foreign_duplicate_or_cross_channel_route_is_rejected() {
        let mut g = fixture();
        g.links = vec![Link {
            out: 3,
            input: 99,
            active: true,
        }];
        assert!(matches!(
            g.connection_plan(&g.nodes[0], &g.nodes[1]),
            Err(Error::GraphInvalid)
        ));
        g.links = vec![
            Link {
                out: 3,
                input: 5,
                active: true,
            },
            Link {
                out: 3,
                input: 5,
                active: true,
            },
        ];
        assert!(matches!(
            g.connection_plan(&g.nodes[0], &g.nodes[1]),
            Err(Error::GraphInvalid)
        ));
        g.links = vec![Link {
            out: 3,
            input: 6,
            active: true,
        }];
        assert!(matches!(
            g.connection_plan(&g.nodes[0], &g.nodes[1]),
            Err(Error::GraphInvalid)
        ));
    }
    #[test]
    fn node_ambiguity_and_unknown_channel_are_not_assumed_stereo() {
        let mut g = fixture();
        let s = g.nodes[0].clone();
        g.ports[0].channel = "MONO".into();
        assert!(g.stereo(&s, true).is_err());
        g.nodes.push(s);
        assert!(g.node("owned", "Stream/Output/Audio").is_err());
        assert!(endpoint("sink:foreign", "FL").is_err());
    }

    fn aux_sink_fixture() -> Graph {
        let mut g = fixture();
        g.nodes[1].audio_channels = Some(2);
        g.nodes[1].audio_positions = vec!["FL".into(), "FR".into()];
        g.ports[2].channel = "AUX1".into();
        g.ports[2].name = "AUX1".into();
        g.ports[3].channel = "AUX0".into();
        g.ports[3].name = "AUX0".into();
        g
    }

    #[test]
    fn aux_pair_is_accepted_only_with_explicit_stereo_layout_and_exact_ports() {
        let mut g = aux_sink_fixture();
        let pair = g.stereo(&g.nodes[1], false).unwrap();
        // Port enumeration order is irrelevant; channel indexes are mapped
        // to the explicitly published FL/FR node layout.
        assert_eq!(
            (pair[0].channel.as_str(), pair[1].channel.as_str()),
            ("AUX0", "AUX1")
        );
        g.links.clear();
        let plan = g.connection_plan(&g.nodes[0], &g.nodes[1]).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(
            plan.iter().map(|(_, input)| input.id).collect::<Vec<_>>(),
            [6, 5]
        );
    }

    #[test]
    fn aux_pair_without_proof_or_with_ambiguous_layout_is_rejected() {
        let mut g = aux_sink_fixture();
        g.nodes[1].audio_channels = None;
        assert!(g.stereo(&g.nodes[1], false).is_err());
        g.nodes[1].audio_channels = Some(2);
        g.nodes[1].audio_positions.clear();
        assert!(g.stereo(&g.nodes[1], false).is_err());
        g.nodes[1].audio_positions = vec!["FL".into(), "FR".into(), "FC".into(), "LFE".into()];
        g.nodes[1].audio_channels = Some(4);
        assert!(g.stereo(&g.nodes[1], false).is_err());
    }

    #[test]
    fn aux_ports_with_reversed_layout_are_rejected_instead_of_swapped() {
        let mut g = aux_sink_fixture();
        g.nodes[1].audio_positions = vec!["FR".into(), "FL".into()];
        assert!(g.stereo(&g.nodes[1], false).is_err());
    }

    #[test]
    fn aux_missing_or_extra_port_is_rejected() {
        let mut g = aux_sink_fixture();
        g.ports
            .retain(|port| !(port.node == 2 && !port.output && port.channel == "AUX1"));
        assert!(g.stereo(&g.nodes[1], false).is_err());
        let mut g = aux_sink_fixture();
        g.ports.push(Port {
            id: 7,
            node: 2,
            name: "AUX2".into(),
            channel: "AUX2".into(),
            output: false,
            index: Some(2),
            physical_terminal: true,
        });
        assert!(g.stereo(&g.nodes[1], false).is_err());
    }

    #[test]
    fn conventional_fl_fr_remains_semantic_even_when_port_order_is_reversed() {
        let mut g = fixture();
        g.ports.swap(0, 1);
        let pair = g.stereo(&g.nodes[0], true).unwrap();
        assert_eq!(
            (pair[0].channel.as_str(), pair[1].channel.as_str()),
            ("FL", "FR")
        );
    }

    #[test]
    fn wrong_sink_replaced_identity_and_bad_preexisting_links_are_rejected() {
        let g = fixture();
        let mut wrong_sink = g.nodes[1].clone();
        wrong_sink.name = "another-sink".into();
        assert!(g.connection_plan(&g.nodes[0], &wrong_sink).is_err());

        let replaced = fixture();
        let mut changed_sink = replaced.nodes[1].clone();
        changed_sink.serial += 1;
        assert!(replaced
            .connection_plan(&replaced.nodes[0], &changed_sink)
            .is_err());

        let mut incorrect_links = fixture();
        incorrect_links.links[0].input = incorrect_links.ports[3].id;
        assert!(incorrect_links
            .connection_plan(&incorrect_links.nodes[0], &incorrect_links.nodes[1])
            .is_err());
        assert!(incorrect_links
            .validate(&incorrect_links.nodes[0], &incorrect_links.nodes[1])
            .is_err());
    }

    #[test]
    fn selected_sink_diagnostic_reports_default_match_for_sink_argument_shape() {
        let g = fixture();
        let summary = g.sanitized_summary(Some(&g.nodes[1]), None);
        assert_eq!(summary["default_sink_matches_selected"], true);
    }

    #[test]
    fn usb_pro_aux_sink_uses_verified_kernel_stereo_map() {
        let dump = br#"[
          {"id":103,"type":"PipeWire:Interface:Device","info":{"props":{"object.serial":14029,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.id":"K17","alsa.components":"USB2972:0107"}}},
          {"id":95,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"synthetic-usb-sink","media.class":"Audio/Sink","object.serial":14030,"audio.channels":2,"audio.position":"[ AUX0, AUX1 ]","device.id":103,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.device":0,"alsa.subdevice":0,"alsa.id":"K17","alsa.components":"USB2972:0107","api.alsa.path":"hw:4,0","api.alsa.pcm.card":4,"api.alsa.pcm.stream":"playback"},"params":{"Props":[{"params":["api.alsa.use-chmap",false]}]}}},
          {"id":98,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":0,"port.name":"playback_AUX0","audio.channel":"AUX0","port.direction":"in","port.physical":true,"port.terminal":true}}},
          {"id":102,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":1,"port.name":"playback_AUX1","audio.channel":"AUX1","port.direction":"in","port.physical":true,"port.terminal":true}}}
        ]"#;
        let report = "Playback:\n  Status: Stop\n  Interface 1\n    Altset 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n  Interface 1\n    Altset 2\n    Format: S16_LE\n    Channels: 2\n    Channel map: FL FR\n  Interface 1\n    Altset 3\n    Format: DSD_U32_BE\n    Channels: 2\n    Channel map: FL FR\n  Interface 1\n    Altset 4\n    Format: S32_LE\n    Channels: 8\n    Channel map: FL FR FC LFE RL RR SL SR\n";
        let g = Graph::parse_with_usb_proof(dump, |endpoint| {
            if endpoint.card == 4
                && endpoint.device == 0
                && endpoint.card_id == "K17"
                && endpoint.usb_id == "2972:0107"
                && usb_playback_report_is_stereo(report).is_ok()
            {
                Ok(())
            } else {
                Err("fixture_usb_proof_mismatch")
            }
        })
        .unwrap();
        assert!(g.nodes[0].usb_stereo_verified);
        assert_eq!(
            g.stereo(&g.nodes[0], false)
                .unwrap()
                .map(|port| port.channel),
            ["AUX0", "AUX1"]
        );
        let unproved = Graph::parse_with_usb_proof(dump, |_| Err("alsa_stream_not_found")).unwrap();
        assert_eq!(
            unproved.nodes[0].usb_stereo_failure,
            Some("alsa_stream_not_found")
        );
        assert!(unproved.stereo(&unproved.nodes[0], false).is_err());
        let mut bad_device: Value = serde_json::from_slice(dump).unwrap();
        bad_device[0]["info"]["props"]["alsa.card"] = serde_json::json!(9);
        let bad_device =
            Graph::parse_with_usb_proof(&serde_json::to_vec(&bad_device).unwrap(), |_| Ok(()))
                .unwrap();
        assert_eq!(
            bad_device.nodes[0].usb_stereo_failure,
            Some("pipewire_device_card_mismatch")
        );
        assert!(bad_device.stereo(&bad_device.nodes[0], false).is_err());
        let mut override_map: Value = serde_json::from_slice(dump).unwrap();
        override_map[1]["info"]["params"]["Props"][0]["params"][1] = serde_json::json!(true);
        let override_map =
            Graph::parse_with_usb_proof(&serde_json::to_vec(&override_map).unwrap(), |_| Ok(()))
                .unwrap();
        assert_eq!(
            override_map.nodes[0].usb_stereo_failure,
            Some("pipewire_chmap_override_not_disabled")
        );
        assert!(override_map.stereo(&override_map.nodes[0], false).is_err());
        let mut swapped = g;
        swapped.ports[0].index = Some(1);
        assert!(swapped.stereo(&swapped.nodes[0], false).is_err());
        swapped.ports[0].index = Some(0);
        swapped.ports[0].physical_terminal = false;
        assert!(swapped.stereo(&swapped.nodes[0], false).is_err());
    }

    #[test]
    fn kernel_usb_report_requires_complete_consistent_playback_maps() {
        let good = "Playback:\n  Interface 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n  Interface 1\n    Format: S16_LE\n    Channels: 2\n    Channel map: FL FR\nCapture:\n";
        assert_eq!(usb_playback_report_is_stereo(good), Ok(()));
        // Actual `/proc/asound/cardN/streamN` shape while a PCM is running:
        // the status field `Interface = 1` precedes capability `Interface 1`.
        let running = "Playback:\n  Status: Running\n    Interface = 1\n    Altset = 1\n    Momentary freq = 48000 Hz\n  Interface 1\n    Altset 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n";
        assert_eq!(usb_playback_report_is_stereo(running), Ok(()));
        assert_eq!(
            usb_playback_report_is_stereo(&good.replace("FL FR", "FR FL")),
            Err("alsa_playback_channel_map_not_fl_fr")
        );
        assert_eq!(
            usb_playback_report_is_stereo(&good.replacen(
                "Channel map: FL FR",
                "Channel map: AUX0 AUX1",
                1
            )),
            Err("alsa_playback_channel_map_not_fl_fr")
        );
        assert_eq!(
            usb_playback_report_is_stereo(&good.replacen("Channels: 2", "Channels: 8", 1)),
            Err("alsa_playback_channel_map_channel_count_mismatch")
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Interface 1\n    Channels: 2\n    Channel map: FL FR\n"
            ),
            Err("alsa_playback_format_missing")
        );
        assert_eq!(
            usb_playback_report_is_stereo("Playback:\n  Interface 1\n    Channels: 2\n"),
            Err("alsa_playback_channel_map_not_fl_fr")
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Status: Running\n  Interface 1\n    Altset 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n"
            ),
            Ok(())
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Interface 1\n    Format: S32_LE\n    Channel map: FL FR\n    Rates: 48000\nCapture:\n  Interface 2\n    Format: invalid-capture-only\n"
            ),
            Err("alsa_playback_channels_missing")
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Interface 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\nCapture:\n  Interface 2\n    Format: invalid-capture-only\n"
            ),
            Ok(())
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Status: Running\n  Interface 1\n    Altset 1\n    Format: S32_LE\n    Channels: active\n    Channel map: FL FR\n"
            ),
            Err("alsa_playback_channels_invalid")
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Status: Running\n  Interface 1\n    Altset 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL\n"
            ),
            Err("alsa_playback_channel_map_channel_count_mismatch")
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Capture:\n  Interface 1\n    Channels: 2\n    Channel map: FL FR\n"
            ),
            Err("alsa_playback_section_missing")
        );
    }

    #[test]
    fn pro_profile_aux_pair_is_proven_by_first_two_kernel_positions() {
        // Pro-mode PipeWire exposes AUX0/AUX1 for a two-channel node. The USB
        // endpoint may also advertise multichannel formats; the selected
        // node's exact stereo width and disabled channel-map override are
        // checked separately by usb_playback_identity and Graph::stereo.
        let report = "Playback:\n  Interface 1\n    Altset 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n  Interface 1\n    Altset 2\n    Format: S16_LE\n    Channels: 2\n    Channel map: FL FR\n  Interface 1\n    Altset 3\n    Format: S32_LE\n    Channels: 8\n    Channel map: FL FR FC LFE RL RR SL SR\n";
        assert_eq!(usb_playback_report_is_stereo(report), Ok(()));

        let conventional = "Playback:\n  Interface 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n";
        assert_eq!(usb_playback_report_is_stereo(conventional), Ok(()));

        let stereo_with_unrelated_mono = format!(
            "{report}  Interface 1\n    Altset 4\n    Format: S16_LE\n    Channels: 1\n    Channel map: MONO\n"
        );
        assert_eq!(
            usb_playback_report_is_stereo(&stereo_with_unrelated_mono),
            Ok(())
        );
        assert_eq!(
            usb_playback_report_is_stereo(
                "Playback:\n  Interface 1\n    Format: S16_LE\n    Channels: 1\n    Channel map: MONO\n"
            ),
            Err("alsa_playback_stereo_setting_missing")
        );

        assert_eq!(
            usb_playback_report_is_stereo(&report.replace(
                "Channels: 8\n    Channel map: FL FR FC LFE RL RR SL SR",
                "Channels: 8\n    Channel map: FR FL FC LFE RL RR SL SR"
            )),
            Err("alsa_playback_channel_map_not_fl_fr")
        );
        assert_eq!(
            usb_playback_report_is_stereo(&report.replace(
                "Channels: 8\n    Channel map: FL FR FC LFE RL RR SL SR",
                "Channels: 8\n    Channel map: FL FR FC LFE RL RR RL SR"
            )),
            Err("alsa_playback_channel_map_ambiguous")
        );
        assert_eq!(
            usb_playback_report_is_stereo(&report.replace(
                "Channels: 8\n    Channel map: FL FR FC LFE RL RR SL SR",
                "Channels: 8\n    Channel map: FL FR FC LFE RL RR SL"
            )),
            Err("alsa_playback_channel_map_channel_count_mismatch")
        );
        assert_eq!(
            usb_playback_report_is_stereo(&report.replace(
                "Channels: 8\n    Channel map: FL FR FC LFE RL RR SL SR",
                "Channels: 8\n    Channel map: AUX0 AUX1 FC LFE RL RR SL SR"
            )),
            Err("alsa_playback_channel_map_not_fl_fr")
        );
    }

    #[test]
    fn kernel_usb_proof_requires_matching_card_device_and_usb_identity() {
        let root = tempfile::tempdir().unwrap();
        let card = root.path().join("card4");
        std::fs::create_dir(&card).unwrap();
        std::fs::write(card.join("id"), "K17\n").unwrap();
        std::fs::write(card.join("usbid"), "2972:0107\n").unwrap();
        std::fs::write(card.join("stream0"), "Playback:\n  Interface 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n").unwrap();
        let endpoint = UsbPlayback {
            card: 4,
            device: 0,
            pw_device_id: 103,
            pw_device_serial: 14029,
            card_id: "K17".into(),
            usb_id: "2972:0107".into(),
        };
        assert_eq!(usb_kernel_stereo_proof_at(root.path(), &endpoint), Ok(()));
        std::fs::write(card.join("usbid"), "0000:0000\n").unwrap();
        assert_eq!(
            usb_kernel_stereo_proof_at(root.path(), &endpoint),
            Err("alsa_usb_identity_mismatch")
        );
        std::fs::write(card.join("usbid"), "2972:0107\n").unwrap();
        std::fs::remove_file(card.join("stream0")).unwrap();
        assert_eq!(
            usb_kernel_stereo_proof_at(root.path(), &endpoint),
            Err("alsa_stream_not_found")
        );
        std::fs::remove_file(card.join("id")).unwrap();
        assert_eq!(
            usb_kernel_stereo_proof_at(root.path(), &endpoint),
            Err("alsa_card_id_not_found")
        );
    }

    #[test]
    fn kernel_usb_proof_rejects_identity_replacement_during_sampling() {
        let root = std::path::Path::new("/synthetic/proc/asound");
        let endpoint = UsbPlayback {
            card: 4,
            device: 0,
            pw_device_id: 103,
            pw_device_serial: 14029,
            card_id: "K17".into(),
            usb_id: "2972:0107".into(),
        };
        let report = "Playback:\n  Interface 1\n    Format: S32_LE\n    Channels: 2\n    Channel map: FL FR\n".to_string();
        let mut id_reads = 0;
        let result = usb_kernel_stereo_proof_with(root, &endpoint, |path, _| {
            match path.file_name().and_then(|name| name.to_str()) {
                Some("id") => {
                    id_reads += 1;
                    Ok(if id_reads == 1 { "K17\n" } else { "REPLACED\n" }.into())
                }
                Some("usbid") => Ok("2972:0107\n".into()),
                Some("stream0") => Ok(report.clone()),
                _ => Err(ProcReadFailure::NotFound),
            }
        });
        assert_eq!(result, Err("alsa_card_identity_changed"));

        let mut usb_reads = 0;
        let result = usb_kernel_stereo_proof_with(root, &endpoint, |path, _| {
            match path.file_name().and_then(|name| name.to_str()) {
                Some("id") => Ok("K17\n".into()),
                Some("usbid") => {
                    usb_reads += 1;
                    Ok(if usb_reads == 1 {
                        "2972:0107\n".into()
                    } else {
                        "0000:0000\n".into()
                    })
                }
                Some("stream0") => Ok(report.clone()),
                _ => Err(ProcReadFailure::NotFound),
            }
        });
        assert_eq!(result, Err("alsa_usb_identity_changed"));
    }

    #[test]
    fn kernel_usb_proof_distinguishes_temporarily_unavailable_stream_data() {
        let root = std::path::Path::new("/synthetic/proc/asound");
        let endpoint = UsbPlayback {
            card: 4,
            device: 0,
            pw_device_id: 103,
            pw_device_serial: 14029,
            card_id: "K17".into(),
            usb_id: "2972:0107".into(),
        };
        let result = usb_kernel_stereo_proof_with(root, &endpoint, |path, _| {
            match path.file_name().and_then(|name| name.to_str()) {
                Some("id") => Ok("K17\n".into()),
                Some("usbid") => Ok("2972:0107\n".into()),
                Some("stream0") => Err(ProcReadFailure::PermissionDenied),
                _ => Err(ProcReadFailure::NotFound),
            }
        });
        assert_eq!(result, Err("alsa_stream_permission_denied"));
    }

    #[test]
    fn continuous_usb_health_reuses_stereo_admission_but_rechecks_device_identity() {
        let endpoint = UsbPlayback {
            card: 4,
            device: 0,
            pw_device_id: 103,
            pw_device_serial: 14029,
            card_id: "K17".into(),
            usb_id: "2972:0107".into(),
        };
        let root = std::path::Path::new("/synthetic/proc/asound");
        let mut observed_paths = Vec::new();
        assert_eq!(
            usb_kernel_identity_proof_with(root, &endpoint, |path, _| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                observed_paths.push(name.clone());
                match name.as_str() {
                    "id" => Ok("K17\n".into()),
                    "usbid" => Ok("2972:0107\n".into()),
                    // A missing streamN is expected here: continuous health
                    // reuses admission rather than reparsing capability blocks
                    // that can be transient while playback is active.
                    "stream0" => Err(ProcReadFailure::NotFound),
                    _ => Err(ProcReadFailure::NotFound),
                }
            }),
            Ok(())
        );
        assert_eq!(observed_paths, ["id", "usbid", "id", "usbid"]);

        let mut id_reads = 0;
        let result = usb_kernel_identity_proof_with(root, &endpoint, |path, _| {
            match path.file_name().and_then(|name| name.to_str()) {
                Some("id") => {
                    id_reads += 1;
                    Ok(if id_reads == 1 { "K17\n" } else { "REPLACED\n" }.into())
                }
                Some("usbid") => Ok("2972:0107\n".into()),
                _ => Err(ProcReadFailure::NotFound),
            }
        });
        assert_eq!(result, Err("alsa_card_identity_changed"));

        assert_eq!(
            usb_kernel_identity_proof_with(root, &endpoint, |path, _| {
                match path.file_name().and_then(|name| name.to_str()) {
                    Some("id") => Ok("K17\n".into()),
                    Some("usbid") => Err(ProcReadFailure::NotFound),
                    _ => Err(ProcReadFailure::NotFound),
                }
            }),
            Err("alsa_usb_id_not_found")
        );
    }

    #[test]
    fn admitted_pipewire_sink_survives_default_change_but_not_device_replacement() {
        let dump = br#"[
          {"id":103,"type":"PipeWire:Interface:Device","info":{"props":{"object.serial":14029,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.id":"K17","alsa.components":"USB2972:0107"}}},
          {"id":95,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"synthetic-usb-sink","media.class":"Audio/Sink","object.serial":14030,"audio.channels":2,"audio.position":"[ AUX0, AUX1 ]","device.id":103,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.device":0,"alsa.subdevice":0,"alsa.id":"K17","alsa.components":"USB2972:0107","api.alsa.path":"hw:4,0","api.alsa.pcm.card":4,"api.alsa.pcm.stream":"playback"},"params":{"Props":[{"params":["api.alsa.use-chmap",false]}]}},"params":{}},
          {"id":98,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":0,"port.name":"playback_AUX0","audio.channel":"AUX0","port.direction":"in","port.physical":true,"port.terminal":true}}},
          {"id":102,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":1,"port.name":"playback_AUX1","audio.channel":"AUX1","port.direction":"in","port.physical":true,"port.terminal":true}}}
        ]"#;
        let admitted = Graph::parse_with_usb_proof(dump, |_| Ok(()))
            .unwrap()
            .nodes
            .into_iter()
            .next()
            .unwrap();
        assert!(admitted.usb_stereo_verified);

        let mut current: Vec<Value> = serde_json::from_slice(dump).unwrap();
        current.push(serde_json::json!({
            "id": 200,
            "type": "PipeWire:Interface:Metadata",
            "metadata": [{"key":"default.audio.sink", "value":{"name":"other-default"}}]
        }));
        let current = serde_json::to_vec(&current).unwrap();
        let identity_checks = std::sync::atomic::AtomicU64::new(0);
        let graph = Graph::parse_with_admitted_sink_proof(&current, &admitted, |_| {
            identity_checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            identity_checks.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(graph.default.as_deref(), Some("other-default"));
        assert!(graph.nodes[0].usb_stereo_verified);
        assert_eq!(graph.nodes[0].serial, admitted.serial);

        let mut replaced: Vec<Value> = serde_json::from_slice(dump).unwrap();
        replaced[0]["info"]["props"]["object.serial"] = serde_json::json!(14031);
        let replaced = serde_json::to_vec(&replaced).unwrap();
        let graph =
            Graph::parse_with_admitted_sink_proof(&replaced, &admitted, |_| Ok(())).unwrap();
        assert!(!graph.nodes[0].usb_stereo_verified);
        assert_eq!(
            graph.nodes[0].usb_stereo_failure,
            Some("alsa_playback_identity_changed")
        );
        assert!(graph.stereo(&graph.nodes[0], false).is_err());

        let mut admission_missing = admitted;
        admission_missing.usb_stereo_verified = false;
        let graph =
            Graph::parse_with_admitted_sink_proof(dump, &admission_missing, |_| Ok(())).unwrap();
        assert_eq!(
            graph.nodes[0].usb_stereo_failure,
            Some("alsa_stereo_admission_missing")
        );
        assert!(graph.stereo(&graph.nodes[0], false).is_err());
    }

    #[test]
    fn usb_identity_parser_reports_exact_missing_pipewire_predicate() {
        let dump = br#"[
          {"id":103,"type":"PipeWire:Interface:Device","info":{"props":{"object.serial":14029,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.id":"K17","alsa.components":"USB2972:0107"}}},
          {"id":95,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"synthetic-usb-sink","media.class":"Audio/Sink","object.serial":14030,"audio.channels":2,"audio.position":"[ AUX0, AUX1 ]","device.id":103,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.device":0,"alsa.subdevice":0,"alsa.id":"K17","alsa.components":"USB2972:0107","api.alsa.path":"hw:4,0","api.alsa.pcm.card":4,"api.alsa.pcm.stream":"playback"},"params":{"Props":[{"params":["api.alsa.use-chmap",true]}]}}},
          {"id":98,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":0,"port.name":"playback_AUX0","audio.channel":"AUX0","port.direction":"in","port.physical":true,"port.terminal":true}}},
          {"id":102,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":1,"port.name":"playback_AUX1","audio.channel":"AUX1","port.direction":"in","port.physical":true,"port.terminal":true}}}
        ]"#;
        let graph = Graph::parse_with_usb_proof(dump, |_| Ok(())).unwrap();
        assert_eq!(
            graph.nodes[0].usb_stereo_failure,
            Some("pipewire_chmap_override_not_disabled")
        );
        let observed = graph.nodes[0].usb_proof_observation.as_ref().unwrap();
        assert_eq!(observed.alsa_card_index, Some(4));
        assert_eq!(observed.alsa_pcm_device, Some(0));
        assert_eq!(observed.device_alsa_card_index, Some(4));
        assert_eq!(observed.chmap_property_count, 1);
        assert_eq!(observed.chmap_disabled, Some(true));
        assert!(graph.stereo(&graph.nodes[0], false).is_err());

        let mut missing_property: Value = serde_json::from_slice(dump).unwrap();
        missing_property[1]["info"]["params"]
            .as_object_mut()
            .unwrap()
            .remove("Props");
        let missing_property = Graph::parse_with_usb_proof(
            &serde_json::to_vec(&missing_property).unwrap(),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(
            missing_property.nodes[0].usb_stereo_failure,
            Some("pipewire_chmap_property_missing")
        );
    }

    #[test]
    fn parser_reads_only_explicit_channel_position_properties() {
        let dump = br#"[
          {"id":1,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"sink","media.class":"Audio/Sink","object.serial":"22","audio.channels":"2","audio.position":"[ FL, FR ]"}}},
          {"id":2,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":"1","port.name":"playback_0","audio.channel":"AUX0","port.direction":"in"}}},
          {"id":3,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":"1","port.name":"playback_1","audio.channel":"AUX1","port.direction":"in"}}},
          {"id":4,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":"1","port.name":"fake","audio.channel":"FL","port.direction":"sideways"}}}
        ]"#;
        let g = Graph::parse(dump).unwrap();
        assert_eq!(g.nodes[0].audio_channels, Some(2));
        assert_eq!(g.nodes[0].audio_positions, ["FL", "FR"]);
        assert_eq!(g.ports.len(), 2);
        assert_eq!(
            g.stereo(&g.nodes[0], false)
                .unwrap()
                .map(|port| port.channel),
            ["AUX0", "AUX1"]
        );
    }
    #[test]
    fn graph_diagnostic_summary_keeps_topology_but_redacts_device_and_node_names() {
        let g = fixture();
        let summary = g.sanitized_summary(Some(&g.nodes[0]), Some(&g.nodes[1]));
        let encoded = summary.to_string();
        assert!(encoded.contains("owned_stream"));
        assert!(encoded.contains("selected_sink"));
        assert!(encoded.contains("output_port_id"));
        assert!(encoded.contains("FL"));
        assert!(!encoded.contains("\"owned\""));
        assert!(!encoded.contains("\"sink\""));
    }

    #[test]
    fn usb_proof_diagnostic_snapshot_is_useful_without_device_names_or_ids() {
        let dump = br#"[
          {"id":103,"type":"PipeWire:Interface:Device","info":{"props":{"object.serial":14029,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.id":"PRIVATE_NAME","alsa.components":"USB2972:0107"}}},
          {"id":95,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"PRIVATE_NODE","media.class":"Audio/Sink","object.serial":14030,"audio.channels":2,"audio.position":"[ AUX0, AUX1 ]","device.id":103,"device.bus":"usb","alsa.driver_name":"snd_usb_audio","alsa.card":4,"alsa.device":0,"alsa.subdevice":0,"alsa.id":"PRIVATE_NAME","alsa.components":"USB2972:0107","api.alsa.path":"hw:4,0","api.alsa.pcm.card":4,"api.alsa.pcm.stream":"playback"},"params":{"Props":[{"params":["api.alsa.use-chmap",false]}]}}},
          {"id":98,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":0,"port.name":"private-port-a","audio.channel":"AUX0","port.direction":"in","port.physical":true,"port.terminal":true}}},
          {"id":102,"type":"PipeWire:Interface:Port","info":{"props":{"node.id":95,"port.id":1,"port.name":"private-port-b","audio.channel":"AUX1","port.direction":"in","port.physical":true,"port.terminal":true}}}
        ]"#;
        let graph = Graph::parse_with_usb_proof(dump, |_| Err("alsa_stream_not_found")).unwrap();
        let summary = graph.sanitized_summary(None, Some(&graph.nodes[0]));
        let encoded = summary.to_string();
        assert!(encoded.contains("alsa_stream_not_found"));
        assert!(encoded.contains("alsa_card_index"));
        assert!(encoded.contains("device_alsa_card_index"));
        assert!(encoded.contains("chmap_disabled"));
        assert!(encoded.contains("AUX0"));
        assert!(!encoded.contains("PRIVATE_NAME"));
        assert!(!encoded.contains("PRIVATE_NODE"));
        assert!(!encoded.contains("2972:0107"));
        assert!(!encoded.contains("hw:4,0"));
        assert!(!encoded.contains("private-port"));
    }
}
