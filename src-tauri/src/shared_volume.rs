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


// Address the inspected node, not a possibly changed default, for a mute gesture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target { pub id: String, pub serial: String }
impl Target { pub(crate) fn token(&self) -> String { format!("{}:{}",self.id,self.serial) } }
fn parse_target(output: &[u8]) -> Result<Target,String> {
    let text=std::str::from_utf8(output).map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED")?;
    let id=text.lines().next().and_then(|line|line.trim().strip_prefix("id "))
        .and_then(|line|line.split(',').next()).filter(|id|!id.is_empty()&&id.bytes().all(|b|b.is_ascii_digit())).ok_or("SHARED_MUTE_CONTEXT_CHANGED")?;
    let serial=text.lines().filter_map(|line|line.trim().trim_start_matches("* ").split_once(" = "))
        .find_map(|(key,value)|(key=="object.serial").then_some(value.trim_matches('"')))
        .filter(|v|!v.is_empty()&&v.bytes().all(|b|b.is_ascii_digit())).ok_or("SHARED_MUTE_CONTEXT_CHANGED")?;
    Ok(Target{id:id.into(),serial:serial.into()})
}
pub(crate) fn read_target() -> Result<Target,String> {
    parse_target(&run_wpctl(&["inspect",DEFAULT_SINK]).map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED")?)
}
pub(crate) fn read_target_volume(target:&Target)->Result<PipeWireVolume,String> {
    parse_volume(&run_wpctl(&["get-volume",&target.id]).map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED")?)
        .map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED".into())
}
pub(crate) fn apply_mute_action(target:&Target,action:crate::shared_mute::Action)->Result<(),String> {
    apply_mute_action_with(target,action,run_wpctl)
}
fn apply_mute_action_with(target:&Target,action:crate::shared_mute::Action,mut run:impl FnMut(&[&str])->Result<Vec<u8>,WpctlError>)->Result<(),String> {
    let confirmed=parse_target(&run(&["inspect",&target.id]).map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED")?)?;
    if confirmed!=*target {return Err("SHARED_MUTE_CONTEXT_CHANGED".into());}
    use crate::shared_mute::Action;
    match action {
        Action::None=>Ok(()),
        Action::Mute(muted)=>run(&["set-mute",&target.id,if muted {"1"}else{"0"}]).map(|_|()).map_err(|_|"SHARED_MUTE_FAILED".into()),
        Action::Volume(value)=> {
            run(&["set-volume",&target.id,&format!("{}%",value)]).map_err(|_|"SHARED_MUTE_FAILED")?;
            if value>0 {run(&["set-mute",&target.id,"0"]).map_err(|_|"SHARED_MUTE_FAILED")?;}
            Ok(())
        }
    }
}

pub(crate) fn normalize_volume(value: u32) -> u32 {
    value.min(100)
}

fn run_wpctl(args: &[&str]) -> Result<Vec<u8>, WpctlError> {
    crate::initial_plex_playback::check_effect().map_err(|_| WpctlError::Failed)?;
    if args.first()==Some(&"inspect") {run_command_limit("wpctl",args,WPCTL_TIMEOUT,64*1024)}
    else {run_command("wpctl",args,WPCTL_TIMEOUT)}
}

fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, WpctlError> {
    run_command_limit(program,args,timeout,MAX_STDOUT_BYTES)
}
fn run_command_limit(program: &str, args: &[&str], timeout: Duration, max_stdout_bytes:u64) -> Result<Vec<u8>, WpctlError> {
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
            .take(max_stdout_bytes + 1)
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
    if output.len() > max_stdout_bytes as usize {
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
    fn native_mute_addresses_the_confirmed_node_without_changing_volume_or_using_default() {
        let target=Target{id:"52".into(),serial:"829".into()};
        let mut calls=Vec::new();
        apply_mute_action_with(&target,crate::shared_mute::Action::Mute(false),|args|{
            calls.push(args.iter().map(|s|s.to_string()).collect::<Vec<_>>());
            Ok(if args[0]=="inspect" {b"id 52, type Node\nobject.serial = \"829\"\n".to_vec()}else{vec![]})
        }).unwrap();
        assert_eq!(calls,vec![vec!["inspect","52"],vec!["set-mute","52","0"]]);
    }
    #[test]
    fn replaced_node_or_failed_native_mute_never_falls_back_to_set_volume() {
        let target=Target{id:"52".into(),serial:"829".into()};
        let mut calls=0;
        assert!(apply_mute_action_with(&target,crate::shared_mute::Action::Mute(false),|args|{
            calls+=1;assert_eq!(args[0],"inspect");Ok(b"id 52, type Node\nobject.serial = \"999\"\n".to_vec())
        }).is_err());assert_eq!(calls,1);
        assert!(apply_mute_action_with(&target,crate::shared_mute::Action::Mute(false),|args|{
            assert_ne!(args[0],"set-volume");if args[0]=="inspect" {Ok(b"id 52, type Node\nobject.serial = \"829\"\n".to_vec())}else{Err(WpctlError::Failed)}
        }).is_err());
    }
    #[test]
    fn inspect_can_read_bounded_node_properties_larger_than_a_volume_response() {
        let text=format!("id 52, type Node\nobject.serial = \"829\"\n{}","p".repeat(6000));
        let output=run_command_limit("printf",&["%s",&text],WPCTL_TIMEOUT,64*1024).unwrap();
        assert_eq!(parse_target(&output).unwrap().token(),"52:829");
        assert_eq!(run_command("printf",&["%s",&text],WPCTL_TIMEOUT),Err(WpctlError::MalformedOutput));
    }
    #[test]
    fn mute_target_requires_node_id_and_serial() {
        let t=parse_target(b"id 52, type PipeWire:Interface:Node\n    object.serial = \"829\"\n").unwrap();
        assert_eq!(t.token(),"52:829");
        assert!(parse_target(b"id 52, type PipeWire:Interface:Node\n").is_err());
        assert!(parse_target(b"id default, type Node\nobject.serial = \"829\"\n").is_err());
    }
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
