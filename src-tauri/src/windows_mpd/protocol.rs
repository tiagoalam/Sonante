//! Bounded MPD text transport for the Windows pilot. No engine is launched here.
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, &'static str>;
const LIMIT: usize = 128 * 1024;
const LINE_LIMIT: usize = 16 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2);

pub fn quote(value: &str) -> Result<String> {
    if value.contains(['\0', '\r', '\n']) {
        return Err("invalid_argument");
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

fn io_error(error: std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => "mpd_timeout",
        _ => "mpd_io",
    }
}

pub struct Client {
    reader: BufReader<TcpStream>,
    broken: bool,
}
impl Client {
    // Ownership is checked before reading a greeting or disclosing the secret.
    pub fn connect(
        address: SocketAddr,
        secret: &str,
        verify: impl FnOnce(&TcpStream) -> Result<()>,
    ) -> Result<Self> {
        if !address.ip().is_loopback() {
            return Err("peer_unverified");
        }
        let stream = TcpStream::connect_timeout(&address, TIMEOUT).map_err(io_error)?;
        verify(&stream)?;
        stream.set_nodelay(true).map_err(io_error)?;
        let mut client = Self {
            reader: BufReader::new(stream),
            broken: false,
        };
        let greeting = client.line(Instant::now() + TIMEOUT)?;
        let version = greeting.strip_prefix("OK MPD ").ok_or("mpd_protocol")?;
        if version.is_empty()
            || version.len() > 32
            || !version.bytes().all(|c| c.is_ascii_digit() || c == b'.')
        {
            return Err("mpd_protocol");
        }
        client.command(&format!("password {}", quote(secret)?))?;
        Ok(client)
    }

    fn line(&mut self, deadline: Instant) -> Result<String> {
        let mut line = Vec::new();
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("mpd_timeout")?;
            self.reader
                .get_mut()
                .set_read_timeout(Some(remaining))
                .map_err(io_error)?;
            let available = self.reader.fill_buf().map_err(io_error)?;
            if available.is_empty() {
                return Err("mpd_eof");
            }
            let count = available
                .iter()
                .position(|c| *c == b'\n')
                .map_or(available.len(), |i| i + 1);
            if line.len() + count > LINE_LIMIT {
                return Err("mpd_protocol");
            }
            let done = available[count - 1] == b'\n';
            line.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            if done {
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return String::from_utf8(line).map_err(|_| "mpd_protocol");
            }
        }
    }

    pub fn command(&mut self, command: &str) -> Result<Vec<String>> {
        self.execute(command, false)
    }

    // Fixed internal policy lists only; renderer input never supplies MPD text.
    pub(crate) fn command_list(&mut self, commands: &[&str]) -> Result<Vec<String>> {
        if commands.is_empty() || commands.len() > 16 || commands.iter().any(|c| c.is_empty() || c.contains(['\0','\r','\n']) || c.starts_with("command_list_")) {
            return Err("invalid_argument");
        }
        self.execute(&format!("command_list_begin\n{}\ncommand_list_end",commands.join("\n")), true)
    }

    fn execute(&mut self, command: &str, list: bool) -> Result<Vec<String>> {
        if self.broken {
            return Err("mpd_disconnected");
        }
        let result = self.exchange(command, list);
        // After a partial reply, late bytes must never satisfy a later command.
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    fn exchange(&mut self, command: &str, list: bool) -> Result<Vec<String>> {
        if command.is_empty() || command.contains(['\0', '\r']) || !list && command.contains('\n') || list && command.len()>4096 {
            return Err("invalid_argument");
        }
        let deadline = Instant::now() + TIMEOUT;
        let stream = self.reader.get_mut();
        stream.set_write_timeout(Some(TIMEOUT)).map_err(io_error)?;
        stream
            .write_all(format!("{command}\n").as_bytes())
            .map_err(io_error)?;
        let mut response = Vec::new();
        let mut bytes = 0;
        loop {
            let line = self.line(deadline)?;
            bytes += line.len() + 1;
            if bytes > LIMIT || response.len() > 1024 {
                return Err("mpd_protocol");
            }
            if line == "OK" {
                return Ok(response);
            }
            // The native bridge intentionally does not expose raw ACKs: they may
            // include file paths or configuration data. Their failure is preserved.
            if line.starts_with("ACK ") {
                return Err("mpd_ack");
            }
            response.push(line);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    pub state: String,
    pub elapsed: f64,
    pub duration: f64,
    pub volume: u8,
    pub audio: String,
}

pub fn field<'a>(lines: &'a [String], key: &str) -> Result<Option<&'a str>> {
    let prefix = format!("{key}: ");
    let mut found = lines.iter().filter_map(|line| line.strip_prefix(&prefix));
    let first = found.next();
    if found.next().is_some() {
        return Err("mpd_protocol");
    }
    Ok(first)
}

pub fn number(value: &str) -> Result<f64> {
    let value: f64 = value.parse().map_err(|_| "mpd_protocol")?;
    if !value.is_finite() || !(0.0..=1_000_000_000.0).contains(&value) {
        return Err("mpd_protocol");
    }
    Ok(value)
}

pub fn playback(lines: &[String]) -> Result<Playback> {
    if field(lines, "error")?.is_some() {
        return Err("mpd_playback_error");
    }
    let state = field(lines, "state")?.ok_or("mpd_protocol")?;
    if !["play", "pause", "stop"].contains(&state) {
        return Err("mpd_protocol");
    }
    let volume: u8 = field(lines, "volume")?
        .ok_or("mpd_protocol")?
        .parse()
        .map_err(|_| "mpd_protocol")?;
    if volume > 100 {
        return Err("mpd_protocol");
    }
    let elapsed = number(field(lines, "elapsed")?.unwrap_or("0"))?;
    let duration = number(field(lines, "duration")?.unwrap_or("0"))?;
    if duration > 0.0 && elapsed > duration + 2.0 {
        return Err("mpd_protocol");
    }
    Ok(Playback {
        state: state.into(),
        elapsed,
        duration,
        volume,
        audio: field(lines, "audio")?.unwrap_or("").into(),
    })
}

// Correct Windows argv quoting, including trailing slashes before a closing
// quote. The executable is also passed separately as lpApplicationName.
pub fn windows_argument(value: &str) -> Result<String> {
    if value.contains('\0') {
        return Err("invalid_argument");
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        out.push_str(&"\\".repeat(if c == '"' { slashes * 2 + 1 } else { slashes }));
        slashes = 0;
        out.push(c);
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    Ok(out)
}

#[cfg(test)]
#[path = "protocol_tests.rs"]
mod tests;

// Bind the projected cursor to the exact owned queue, not just a numeric slot.
pub fn queue_position(lines: &[String], ids: &[u32], previous: usize) -> Result<usize> {
    let length = field(lines, "playlistlength")?
        .ok_or("mpd_protocol")?
        .parse::<usize>()
        .map_err(|_| "mpd_protocol")?;
    if length != ids.len() || ids.is_empty() || previous >= ids.len() {
        return Err("mpd_protocol");
    }
    match (field(lines, "song")?, field(lines, "songid")?) {
        (Some(index), Some(id)) => {
            let index = index.parse::<usize>().map_err(|_| "mpd_protocol")?;
            let id = id.parse::<u32>().map_err(|_| "mpd_protocol")?;
            if ids.get(index) != Some(&id) {
                return Err("mpd_protocol");
            };
            Ok(index)
        }
        (None, None) if field(lines, "state")? == Some("stop") => Ok(previous),
        _ => Err("mpd_protocol"),
    }
}
#[cfg(test)]
mod queue_tests {
    use super::*;
    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_owned).collect()
    }
    #[test]
    fn queue_projection_rejects_unknown_entries_and_partial_state() {
        assert_eq!(
            queue_position(
                &lines("playlistlength: 2\nstate: play\nsong: 1\nsongid: 12"),
                &[10, 12],
                0
            ),
            Ok(1)
        );
        assert_eq!(
            queue_position(&lines("playlistlength: 2\nstate: stop"), &[10, 12], 1),
            Ok(1)
        );
        for text in [
            "playlistlength: 1\nstate: play\nsong: 0\nsongid: 10",
            "playlistlength: 2\nstate: play\nsong: 0\nsongid: 12",
            "playlistlength: 2\nstate: pause",
            "playlistlength: 2\nstate: play\nsong: 2\nsongid: 12",
        ] {
            assert_eq!(
                queue_position(&lines(text), &[10, 12], 0),
                Err("mpd_protocol")
            );
        }
    }
}
