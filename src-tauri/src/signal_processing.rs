//! Read-only processing metadata. No sample comparison or bit-perfect verdict.
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFailure {
    Busy,
    Unavailable,
    Timeout,
    Protocol,
    Changed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum ReplayGain {
    Off,
    Track,
    Album,
    Auto,
    Unknown,
    Unavailable(ReadFailure),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixRamp {
    Off,
    Possible,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Pcm,
    NativeDsd,
    Dop,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputTransport {
    pub kind: TransportKind,
    pub source_rate: u32,
    pub output_rate: u32,
    pub channels: u8,
    pub epoch: u64,
}
fn parse_transport(lines: &[String]) -> Option<OutputTransport> {
    // Only the enabled main ALSA output counts, never an analyzer/other output.
    let mut matching = Vec::new();
    let starts: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, s)| s.starts_with("outputid: "))
        .map(|(i, _)| i)
        .collect();
    for (index, start) in starts.iter().enumerate() {
        let group = &lines[*start..starts.get(index + 1).copied().unwrap_or(lines.len())];
        if value(group, "outputname") == Some("Sonante Output")
            && value(group, "plugin") == Some("alsa")
            && value(group, "outputenabled") == Some("1")
        {
            let values: Vec<_> = group
                .iter()
                .filter_map(|s| s.strip_prefix("attribute: sonante_transport="))
                .collect();
            if values.len() != 1 {
                return None;
            }
            matching.push(values[0]);
        }
    }
    if matching.len() != 1 {
        return None;
    }
    let fields: Vec<_> = matching[0].split(':').collect();
    if fields.len() != 5 {
        return None;
    }
    let kind = match fields[0] {
        "pcm" => TransportKind::Pcm,
        "native_dsd" => TransportKind::NativeDsd,
        "dop" => TransportKind::Dop,
        _ => return None,
    };
    if fields[1..]
        .iter()
        .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let source_rate = fields[1].parse::<u32>().ok()?;
    let output_rate = fields[2].parse::<u32>().ok()?;
    let channels = fields[3].parse::<u8>().ok()?;
    let epoch = fields[4].parse::<u64>().ok()?;
    if !(1000..=50000000).contains(&source_rate)
        || !(1000..=50000000).contains(&output_rate)
        || !(1..=64).contains(&channels)
        || epoch == 0
    {
        return None;
    }
    Some(OutputTransport {
        kind,
        source_rate,
        output_rate,
        channels,
        epoch,
    })
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingStatus {
    #[serde(default)]
    pub transport: Option<OutputTransport>,
    pub mpd_volume: Option<u8>,
    pub crossfade_seconds: Option<f64>,
    pub mixramp: MixRamp,
    pub replay_gain: ReplayGain,
}
fn value<'a>(lines: &'a [String], key: &str) -> Option<&'a str> {
    let mut values = lines
        .iter()
        .filter_map(|line| line.split_once(": "))
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| v);
    let first = values.next()?;
    if values.next().is_some() {
        None
    } else {
        Some(first)
    }
}
impl ProcessingStatus {
    pub fn from_status(lines: &[String]) -> Self {
        let mpd_volume = value(lines, "volume")
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|v| *v <= 100);
        // MPD 0.24 status omits xfade and mixrampdelay when not positive.
        // Only interpret omission in a complete status envelope; malformed or
        // duplicate fields stay unknown. No extra command or playback mutation.
        let complete_status = matches!(value(lines, "state"), Some("play" | "pause" | "stop"))
            && ["playlist", "playlistlength"].iter().all(|key|
                value(lines, key).is_some_and(|v| !v.is_empty()
                    && v.bytes().all(|b| b.is_ascii_digit()) && v.parse::<u32>().is_ok()))
            && ["repeat", "random"].iter().all(|key|
                matches!(value(lines, key), Some("0" | "1")))
            && matches!(value(lines, "single"), Some("0" | "1" | "oneshot"))
            && matches!(value(lines, "consume"), Some("0" | "1" | "oneshot"))
            && value(lines, "mixrampdb").is_some_and(|v|
                v.parse::<f64>().is_ok_and(|n| n.is_finite()));
        let omitted = |key: &str| !lines.iter().any(|line|
            line.split_once(':').is_some_and(|(name, _)| name == key));
        let crossfade_seconds = value(lines, "xfade")
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite() && (0.0..=86400.0).contains(n))
            .or_else(|| (complete_status && omitted("xfade")).then_some(0.0));
        let mixramp = match value(lines, "mixrampdelay") {
            Some("nan") => MixRamp::Off,
            Some(s) if s.parse::<f64>().is_ok_and(|v| v.is_finite() && v <= 0.0) => MixRamp::Off,
            Some(s) if s.parse::<f64>().is_ok_and(|v| v.is_finite() && v > 0.0) => MixRamp::Possible,
            None if complete_status && omitted("mixrampdelay") => MixRamp::Off,
            _ => MixRamp::Unknown,
        };
        Self {
            transport: None,
            mpd_volume,
            crossfade_seconds,
            mixramp,
            replay_gain: ReplayGain::Unknown,
        }
    }
}
fn parse_mode(lines: &[String]) -> Result<ReplayGain, ReadFailure> {
    match value(lines, "replay_gain_mode") {
        Some("off") => Ok(ReplayGain::Off),
        Some("track") => Ok(ReplayGain::Track),
        Some("album") => Ok(ReplayGain::Album),
        Some("auto") => Ok(ReplayGain::Auto),
        _ => Err(ReadFailure::Protocol),
    }
}

static READING: AtomicBool = AtomicBool::new(false);
struct Permit<'a>(&'a AtomicBool);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
fn enter(flag: &AtomicBool) -> Option<Permit<'_>> {
    flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()
        .map(|_| Permit(flag))
}
fn remaining(deadline: Instant) -> Result<Duration, ReadFailure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(ReadFailure::Timeout)
}
fn io_error(error: std::io::Error) -> ReadFailure {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        ReadFailure::Timeout
    } else {
        ReadFailure::Unavailable
    }
}
// A fixed read-only command. Nonblocking connect avoids waiting in a full Unix
// socket backlog; all subsequent reads/writes share a 150ms absolute budget.
pub(crate) fn connect(path: &Path, expected_pid: u32, deadline: Instant) -> Result<UnixStream, ReadFailure> {
    remaining(deadline)?;
    let bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
        return Err(ReadFailure::Unavailable);
    }
    address.sun_family = libc::AF_UNIX as _;
    for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
        *dst = *src as libc::c_char;
    }
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err(ReadFailure::Unavailable);
    }
    // OwnedFd closes on every error path; it refers only to this diagnostic socket.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    let length =
        (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            &address as *const _ as *const libc::sockaddr,
            length,
        )
    } != 0
    {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(ReadFailure::Unavailable);
        }
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let result =
            unsafe { libc::poll(&mut poll, 1, remaining(deadline)?.as_millis().max(1) as i32) };
        if result == 0 {
            return Err(ReadFailure::Timeout);
        }
        if result < 0 {
            return Err(ReadFailure::Unavailable);
        }
        let mut error: libc::c_int = 0;
        let mut len = std::mem::size_of_val(&error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                &mut error as *mut _ as *mut _,
                &mut len,
            )
        } != 0
            || error != 0
        {
            return Err(ReadFailure::Unavailable);
        }
    }
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut _,
            &mut len,
        )
    } != 0
        || credentials.pid <= 0
        || credentials.pid as u32 != expected_pid
    {
        return Err(ReadFailure::Changed);
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false).map_err(io_error)?;
    remaining(deadline)?;
    Ok(stream)
}
fn query_lines(path: &Path, pid: u32, outputs: bool) -> Result<Vec<String>, ReadFailure> {
    let deadline = Instant::now() + Duration::from_millis(150);
    let mut stream = connect(path, pid, deadline)?;
    stream
        .set_read_timeout(Some(remaining(deadline)?))
        .map_err(io_error)?;
    let mut reader =
        BufReader::with_capacity(256, stream.try_clone().map_err(io_error)?.take(16385));
    let mut consumed = 0usize;
    let mut line = String::new();
    let read_line = |reader: &mut BufReader<std::io::Take<UnixStream>>,
                     line: &mut String,
                     consumed: &mut usize|
     -> Result<(), ReadFailure> {
        line.clear();
        let mut bytes = Vec::new();
        loop {
            // Recompute the deadline before every physical read, including a
            // peer that trickles bytes without finishing a line.
            reader
                .get_ref()
                .get_ref()
                .set_read_timeout(Some(remaining(deadline)?))
                .map_err(io_error)?;
            let buffer = reader.fill_buf().map_err(io_error)?;
            if buffer.is_empty() {
                return Err(ReadFailure::Protocol);
            }
            let length = buffer
                .iter()
                .position(|b| *b == b'\n')
                .map_or(buffer.len(), |i| i + 1);
            *consumed += length;
            if *consumed > if outputs { 16384 } else { 4096 } {
                return Err(ReadFailure::Protocol);
            }
            bytes.extend_from_slice(&buffer[..length]);
            reader.consume(length);
            if bytes.last() == Some(&b'\n') {
                break;
            }
        }
        *line = String::from_utf8(bytes).map_err(|_| ReadFailure::Protocol)?;
        remaining(deadline)?;
        Ok(())
    };
    read_line(&mut reader, &mut line, &mut consumed)?;
    if !line.starts_with("OK MPD ") {
        return Err(ReadFailure::Protocol);
    }
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(io_error)?;
    stream
        .write_all(if outputs { b"outputs\n" } else { b"replay_gain_status\n" })
        .map_err(io_error)?;
    let mut lines = Vec::new();
    for _ in 0..if outputs { 128 } else { 16 } {
        read_line(&mut reader, &mut line, &mut consumed)?;
        let text = line.trim_end_matches('\n');
        if text == "OK" {
            return Ok(lines);
        }
        if text.starts_with("ACK") {
            return Err(ReadFailure::Protocol);
        }
        lines.push(text.to_owned());
    }
    Err(ReadFailure::Protocol)
}
fn query(path: &Path, pid: u32) -> Result<ReplayGain, ReadFailure> {
    parse_mode(&query_lines(path, pid, false)?)
}
pub(crate) fn read_output_transport(path: &Path, pid: u32) -> Option<OutputTransport> {
    let _permit = enter(&READING)?;
    parse_transport(&query_lines(path, pid, true).ok()?)
}
// The caller invalidates the result if its owned MPD/output context changed.
pub(crate) fn read_replay_gain(path: &Path, pid: u32) -> ReplayGain {
    let Some(_permit) = enter(&READING) else {
        return ReplayGain::Unavailable(ReadFailure::Busy);
    };
    query(path, pid).unwrap_or_else(ReplayGain::Unavailable)
}

#[cfg(test)]
mod tests;
