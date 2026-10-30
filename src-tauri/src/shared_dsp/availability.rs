//! A read-only compatibility check, independent of runtime hash availability.
//! Unknown observations retain the existing PipeWire graph validation; they are
//! never presented as evidence that the system uses native PulseAudio.
use std::{
    ffi::OsString,
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const MAX_INFO: u64 = 16 * 1024;
const TIMEOUT: Duration = Duration::from_millis(750);
pub(crate) const PULSE_AUDIO_UNSUPPORTED: &str = "EQ_PULSEAUDIO_UNSUPPORTED";

pub(crate) fn pulse_audio_in_use() -> bool {
    let runtime = std::env::var_os("PULSE_RUNTIME_PATH").map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|p| PathBuf::from(p).join("pulse")))
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}/pulse", unsafe { libc::geteuid() })));
    let server = std::env::var_os("PULSE_SERVER").filter(|v| !v.is_empty())
        .unwrap_or_else(|| { let mut v = OsString::from("unix:"); v.push(runtime.join("native")); v });
    read_info(probe_command(server)).is_some_and(|bytes| native_pulse(&bytes))
}

pub(crate) fn require_supported() -> Result<(), String> {
    require_supported_with(pulse_audio_in_use)
}
fn require_supported_with(probe: impl FnOnce() -> bool) -> Result<(), String> {
    if probe() { Err(PULSE_AUDIO_UNSUPPORTED.into()) } else { Ok(()) }
}

fn probe_command(server: OsString) -> Command {
    // An explicit server prevents libpulse autospawn. No daemon is started by
    // this query, and no volume, stream, module or system default is changed.
    let mut command = Command::new("/usr/bin/pactl");
    command.arg("--server").arg(server).arg("info").env("LC_ALL", "C");
    command
}

fn read_info(mut command: Command) -> Option<Vec<u8>> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped())
        .stderr(Stdio::null()).spawn().ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill(); let _ = child.wait(); return None;
    };
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(MAX_INFO + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + TIMEOUT;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => { let _ = child.kill(); let _ = child.wait(); break false; }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    (success && bytes.len() <= MAX_INFO as usize).then_some(bytes)
}

fn native_pulse(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    let mut names = text.lines().filter_map(|line| line.strip_prefix("Server Name:"));
    let Some(name) = names.next() else { return false };
    // PipeWire's PulseAudio protocol server normally identifies itself as
    // "PulseAudio (on PipeWire <version>)". That must not be blocked.
    names.next().is_none() && name.trim().eq_ignore_ascii_case("pulseaudio")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_pulse_is_distinct_from_pipewire_compatibility_and_unknown_results() {
        for bytes in [b"Server Name: pulseaudio\n".as_slice(), b"User Name: fixture\nServer Name: PulseAudio\r\n"] {
            assert!(native_pulse(bytes));
        }
        for bytes in [b"Server Name: PulseAudio (on PipeWire 1.4.2)\n".as_slice(),
            b"Server Name: PipeWire\n", b"Server Name: other\n", b"Server Name:\n", b"", b"bad \xff",
            b"Server Name: pulseaudio\nServer Name: PipeWire\n"] {
            assert!(!native_pulse(bytes));
        }
    }
    #[test]
    fn only_confirmed_pulse_refuses_activation_and_query_is_explicit_read_only() {
        assert_eq!(require_supported_with(|| true), Err(PULSE_AUDIO_UNSUPPORTED.into()));
        assert_eq!(require_supported_with(|| false), Ok(()));
        let command = probe_command(OsString::from("unix:/fixture/pulse/native"));
        assert_eq!(command.get_program(), "/usr/bin/pactl");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--server", "unix:/fixture/pulse/native", "info"]);
        assert!(command.get_envs().any(|(k,v)| k == "LC_ALL" && v == Some(std::ffi::OsStr::new("C"))));
    }
}
