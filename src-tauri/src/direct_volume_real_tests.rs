//! Opt-in integration: owned MPD, generated media, ALSA file/null, no real audio.
use super::*;
use crate::audio::AudioEngine;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(Instant::now() < end, "own MPD fixture deadline");
        thread::sleep(Duration::from_millis(10));
    }
}
fn media(dsd: bool) -> (Vec<u8>, Vec<u8>) {
    if !dsd {
        let mut pcm = vec![];
        for i in 0..8827i32 {
            pcm.extend_from_slice(&(((i * 7919) % 60001 - 30000) as i16).to_le_bytes());
            pcm.extend_from_slice(&(((i * 104729) % 50001 - 25000) as i16).to_le_bytes());
        }
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x02\0");
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&176400u32.to_le_bytes());
        wav.extend_from_slice(b"\x04\0\x10\0data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(&pcm);
        return (wav, pcm);
    }
    let size = 8 * 4096usize;
    let left: Vec<u8> = (0..size)
        .map(|i| ((i * 17 + i / 127 + 3) % 256) as u8)
        .collect();
    let right: Vec<u8> = (0..size)
        .map(|i| ((i * 29 + i / 113 + 101) % 256) as u8)
        .collect();
    let reference = left
        .iter()
        .zip(&right)
        .flat_map(|(a, b)| [*a, *b])
        .collect();
    let mut dsf = b"DSD ".to_vec();
    for n in [28, 92 + (size * 2) as u64, 0] {
        dsf.extend_from_slice(&n.to_le_bytes());
    }
    dsf.extend_from_slice(b"fmt ");
    dsf.extend_from_slice(&52u64.to_le_bytes());
    for n in [1u32, 0, 2, 2, 2822400, 8] {
        dsf.extend_from_slice(&n.to_le_bytes());
    }
    dsf.extend_from_slice(&((size * 8) as u64).to_le_bytes());
    for n in [4096u32, 0] {
        dsf.extend_from_slice(&n.to_le_bytes());
    }
    dsf.extend_from_slice(b"data");
    dsf.extend_from_slice(&(12 + (size * 2) as u64).to_le_bytes());
    for i in (0..size).step_by(4096) {
        dsf.extend_from_slice(&left[i..i + 4096]);
        dsf.extend_from_slice(&right[i..i + 4096]);
    }
    (dsf, reference)
}
fn decoded(raw: &[u8], dop: bool, count: usize) -> Vec<u8> {
    if !dop {
        return raw[..count].to_vec();
    }
    let mut result = vec![];
    let mut previous = None;
    for frame in raw[..count * 2].chunks_exact(8) {
        let a = u32::from_le_bytes(frame[..4].try_into().unwrap());
        let b = u32::from_le_bytes(frame[4..].try_into().unwrap());
        let marker = (a >> 16) & 255;
        assert!(marker == 5 || marker == 250);
        assert_eq!((b >> 16) & 255, marker);
        if let Some(previous) = previous {
            assert_ne!(marker, previous);
        }
        previous = Some(marker);
        result.extend_from_slice(&[(a >> 8) as u8, (b >> 8) as u8, a as u8, b as u8]);
    }
    result
}
#[test]
#[ignore = "explicit own namespace only: SONANTE_DIRECT_GAIN_QA=owned-file-null"]
fn real_direct_unity_and_mute_pcm_native_dsd_dop() {
    assert_eq!(
        std::env::var("SONANTE_DIRECT_GAIN_QA").unwrap(),
        "owned-file-null"
    );
    let root = PathBuf::from("/build/app-source/real-proof-v2");
    fs::create_dir(&root).unwrap();
    let role = std::env::var("SONANTE_DIRECT_GAIN_QA_ENGINE_ID").unwrap_or("release".into());
    let (engine, expected) = match role.as_str() {
        "dev" => (
            Path::new("/engine/mpd"),
            "46ea7b1eaa1969f377698155b041589e9475998a3859a4c96f7937565fda7841",
        ),
        "release" => (
            Path::new("/usr/lib/sonante/runtimes/v1/mpd-direct"),
            "7bf2a6c96fad672ee6a056335c1b8054a71107975c637b76f549751b43d0d4b1",
        ),
        _ => panic!("unknown fixture role"),
    };
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(engine).unwrap())),
        expected
    );
    let mut receipts = vec![];
    for (kind, dsd, dop) in [
        ("pcm", false, false),
        ("native_dsd", true, false),
        ("dop", true, true),
    ] {
        for muted in [false, true] {
            let dir = root.join(format!("{kind}-{}", if muted { "mute" } else { "unity" }));
            fs::create_dir(&dir).unwrap();
            fs::create_dir(dir.join("music")).unwrap();
            let (source, reference) = media(dsd);
            let filename = if dsd { "fixture.dsf" } else { "fixture.wav" };
            fs::write(dir.join("music").join(filename), source).unwrap();
            let capture = dir.join("capture.raw");
            let socket = dir.join("socket");
            let socket = socket.to_str().unwrap();
            let asound = dir.join("asound.conf");
            fs::write(&asound,format!("pcm.proof_null {{ type null }}\npcm.proof_output {{ type file slave.pcm \"proof_null\" file \"{}\" format \"raw\" }}\n",capture.display())).unwrap();
            let conf = dir.join("mpd.conf");
            fs::write(
                &conf,
                format!(
                    r#"music_directory "{dir}/music"
db_file "{dir}/db"
bind_to_address "{socket}"
log_file "{dir}/mpd.log"
auto_update "no"
zeroconf_enabled "no"
replaygain "off"
volume_normalization "no"
decoder {{
 plugin "wildmidi"
 enabled "no"
}}
audio_output {{
 type "alsa"
 name "own file null"
 device "proof_output"
 dop "{dop}"
 pcm_eof_flush "yes"
 sonante_direct_strict "yes"
 close_on_pause "no"
 mixer_type "software"
 replay_gain_handler "none"
 always_on "yes"
}}
"#,
                    dir = dir.display(),
                    dop = if dop { "yes" } else { "no" }
                ),
            )
            .unwrap();
            let log = fs::File::create(dir.join("process.log")).unwrap();
            let mut child = Owned(
                Command::new(engine)
                    .arg("--no-daemon")
                    .arg(&conf)
                    .env("ALSA_CONFIG_PATH", &asound)
                    .env("HOME", &dir)
                    .env("XDG_RUNTIME_DIR", &dir)
                    .env("XDG_CONFIG_HOME", &dir)
                    .stdin(Stdio::null())
                    .stdout(log.try_clone().unwrap())
                    .stderr(log)
                    .spawn()
                    .unwrap(),
            );
            wait(|| Path::new(socket).exists() || child.0.try_wait().unwrap().is_some());
            assert!(child.0.try_wait().unwrap().is_none());
            let pid = child.0.id();
            let command = |cmd: &str| AudioEngine::send_owned_command_to_socket(socket, pid, cmd);
            let session = Session::initialize(command).unwrap();
            command("update").unwrap();
            wait(|| {
                !command("status")
                    .unwrap()
                    .iter()
                    .any(|s| s.starts_with("updating_db:"))
            });
            command(&format!("add \"{filename}\"")).unwrap();
            assert_eq!(status(command("status").unwrap()).unwrap().volume, 0);
            session
                .set_mute(false, true, &session.token(), command)
                .unwrap();
            assert_eq!(status(command("status").unwrap()).unwrap().state, "stop");
            if muted {
                session
                    .set_mute(true, false, &session.token(), command)
                    .unwrap();
            }
            // Explicit fixture play, never emitted by the controller.
            command("play").unwrap();
            wait(|| {
                let lines = command("status").unwrap();
                assert!(!lines.iter().any(|s| s.starts_with("error:")), "{lines:?}");
                status(lines).unwrap().state == "stop"
            });
            command("disableoutput 0").unwrap();
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            let raw = fs::read(&capture).unwrap();
            assert!(raw.len() >= reference.len() * if dop { 2 } else { 1 });
            let actual = if !dsd {
                // PcmVolume's software filter expands S16 to S24_P32 even at
                // unity. Compare signed samples and prove the added bits zero.
                assert!(raw.len() >= reference.len() * 2);
                let mut samples = Vec::with_capacity(reference.len());
                for word in raw[..reference.len() * 2].chunks_exact(4) {
                    let value = i32::from_le_bytes(word.try_into().unwrap());
                    assert_eq!(value & 255, 0, "nonzero added PCM bits");
                    assert!((-8388608..=8388607).contains(&value));
                    samples.extend_from_slice(&((value >> 8) as i16).to_le_bytes());
                }
                assert!(
                    raw[reference.len() * 2..].iter().all(|v| *v == 0),
                    "non-silent PCM EOF padding"
                );
                samples
            } else {
                decoded(&raw, dop, reference.len())
            };
            if muted {
                assert!(
                    actual.iter().all(|v| *v == if dsd { 0x69 } else { 0 }),
                    "{kind}: mute was not silence"
                );
            } else {
                assert!(
                    actual == reference,
                    "{kind}: unity changed source samples; first mismatch {:?}",
                    actual.iter().zip(&reference).position(|(a, b)| a != b)
                );
            }
            receipts.push(serde_json::json!({"transport":kind,"muted":muted,"source_bytes":reference.len(),"pcm_container":if dsd {"not PCM"}else{"S16 expanded losslessly to S24_P32; zero added bits"},"capture_bytes":raw.len(),"controller":"production Session; full app or thin fixture socket adapter as recorded by runner","engine_role":role,"engine_sha256":expected,"debug_assertions":cfg!(debug_assertions),"passed":true}));
        }
    }
    fs::write(
        root.join("result.json"),
        serde_json::to_vec_pretty(&receipts).unwrap(),
    )
    .unwrap();
}
