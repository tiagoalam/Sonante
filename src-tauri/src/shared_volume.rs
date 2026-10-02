use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_SINK: &str = "@DEFAULT_AUDIO_SINK@";
const WPCTL_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_STDOUT_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SharedVolumeBackend {
    PipeWire,
    MpdSoftware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PipeWireVolume {
    pub value: u32,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WpctlError {
    Unavailable,
    Timeout,
    Failed,
    MalformedOutput,
}

impl SharedVolumeBackend {
    pub(crate) fn detect(is_shared: bool, configured_as_pipewire: bool) -> Self {
        Self::detect_with(is_shared, configured_as_pipewire, read_pipewire_volume)
    }

    fn detect_with<F>(is_shared: bool, configured_as_pipewire: bool, probe: F) -> Self
    where
        F: FnOnce() -> Result<PipeWireVolume, String>,
    {
        if is_shared && configured_as_pipewire && probe().is_ok() {
            Self::PipeWire
        } else {
            Self::MpdSoftware
        }
    }
}

pub(crate) fn read_pipewire_volume() -> Result<PipeWireVolume, String> {
    let output = run_wpctl(&["get-volume", DEFAULT_SINK])
        .map_err(|_| "Volume compartilhado temporariamente indisponível.".to_string())?;
    parse_volume(&output)
        .map_err(|_| "Resposta inválida do controle de volume compartilhado.".to_string())
}

pub(crate) fn set_pipewire_volume(value: u32) -> Result<(), String> {
    let value = normalize_volume(value);
    let argument = format!("{}%", value);
    run_wpctl(&["set-volume", DEFAULT_SINK, &argument])
        .map_err(|_| "Não foi possível alterar o volume compartilhado.".to_string())?;

    if value > 0 {
        run_wpctl(&["set-mute", DEFAULT_SINK, "0"])
            .map_err(|_| "Não foi possível remover o mute do volume compartilhado.".to_string())?;
    }

    Ok(())
}

pub(crate) fn normalize_volume(value: u32) -> u32 {
    value.min(100)
}

fn run_wpctl(args: &[&str]) -> Result<Vec<u8>, WpctlError> {
    run_command("wpctl", args, WPCTL_TIMEOUT)
}

fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, WpctlError> {
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| WpctlError::Unavailable)?;

    let stdout = child.stdout.take().ok_or(WpctlError::Failed)?;
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take(MAX_STDOUT_BYTES + 1)
            .read_to_end(&mut output)
            .map(|_| output)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(WpctlError::Timeout);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(WpctlError::Failed);
            }
        }
    };

    let output = reader
        .join()
        .map_err(|_| WpctlError::Failed)?
        .map_err(|_| WpctlError::Failed)?;
    if !status.success() {
        return Err(WpctlError::Failed);
    }
    if output.len() > MAX_STDOUT_BYTES as usize {
        return Err(WpctlError::MalformedOutput);
    }
    Ok(output)
}

fn parse_volume(output: &[u8]) -> Result<PipeWireVolume, WpctlError> {
    let output = std::str::from_utf8(output).map_err(|_| WpctlError::MalformedOutput)?;
    let line = output
        .strip_suffix("\r\n")
        .or_else(|| output.strip_suffix('\n'))
        .unwrap_or(output);
    if line.contains(['\n', '\r']) {
        return Err(WpctlError::MalformedOutput);
    }

    let fields = line
        .strip_prefix("Volume: ")
        .ok_or(WpctlError::MalformedOutput)?
        .split(' ')
        .collect::<Vec<_>>();
    let (raw_value, muted) = match fields.as_slice() {
        [value] => (*value, false),
        [value, "[MUTED]"] => (*value, true),
        _ => return Err(WpctlError::MalformedOutput),
    };
    let mut decimal_parts = raw_value.split('.');
    let whole = decimal_parts.next().unwrap_or_default();
    let fraction = decimal_parts.next().unwrap_or_default();
    if !matches!(whole, "0" | "1")
        || fraction.is_empty()
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || decimal_parts.next().is_some()
    {
        return Err(WpctlError::MalformedOutput);
    }
    let value = raw_value
        .parse::<f64>()
        .map_err(|_| WpctlError::MalformedOutput)?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(WpctlError::MalformedOutput);
    }

    Ok(PipeWireVolume {
        value: (value * 100.0).round() as u32,
        muted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_regular_volume() {
        assert_eq!(
            parse_volume(b"Volume: 0.50\n").unwrap(),
            PipeWireVolume {
                value: 50,
                muted: false,
            }
        );
    }

    #[test]
    fn parses_muted_volume() {
        assert_eq!(
            parse_volume(b"Volume: 0.37 [MUTED]\n").unwrap(),
            PipeWireVolume {
                value: 37,
                muted: true,
            }
        );
    }

    #[test]
    fn rejects_malformed_or_out_of_range_output() {
        assert!(parse_volume(b"volume=0.50\n").is_err());
        assert!(parse_volume(b"Volume: 1.25\n").is_err());
        assert!(parse_volume(b"Volume: NaN\n").is_err());
        assert!(parse_volume(b"Volume: 5e-1\n").is_err());
    }

    #[test]
    fn clamps_ui_volume_to_supported_range() {
        assert_eq!(normalize_volume(0), 0);
        assert_eq!(normalize_volume(64), 64);
        assert_eq!(normalize_volume(101), 100);
    }

    #[test]
    fn detection_requires_shared_pipewire_configuration_and_valid_probe() {
        let valid = || {
            Ok(PipeWireVolume {
                value: 50,
                muted: false,
            })
        };
        assert_eq!(
            SharedVolumeBackend::detect_with(true, true, valid),
            SharedVolumeBackend::PipeWire
        );
        assert_eq!(
            SharedVolumeBackend::detect_with(true, false, valid),
            SharedVolumeBackend::MpdSoftware
        );
        assert_eq!(
            SharedVolumeBackend::detect_with(false, true, valid),
            SharedVolumeBackend::MpdSoftware
        );
        assert_eq!(
            SharedVolumeBackend::detect_with(true, true, || Err("indisponível".to_string())),
            SharedVolumeBackend::MpdSoftware
        );
    }

    #[test]
    fn subprocess_timeout_terminates_and_reaps_child() {
        assert_eq!(
            run_command("sleep", &["1"], Duration::from_millis(20)).unwrap_err(),
            WpctlError::Timeout
        );
    }
}
