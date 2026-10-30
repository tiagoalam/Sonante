//! Reopen an immutable DB copy in an owned null-only daemon. No media, rescan,
//! queue restoration, play, seek, personal socket, or existing process signal.
use crate::{
    index_coverage::Catalog,
    recovery_fs::{self, Dir, INCOMPLETE},
};
use std::{
    collections::HashSet,
    fs::{self},
    io::Read,
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Probe {
    dir: PathBuf,
    child: Option<Child>,
    diagnostics: Option<std::thread::JoinHandle<DiagnosticSummary>>,
}
// Observations only: stderr is untrusted and may contain paths, tags or tokens.
// Never retain/return it, including in test builds. A matched phrase is not proof
// of database corruption; the failure code describes the observed phase only.
const DIAGNOSTIC_PATTERNS: &[(&str, &[u8])] = &[
    ("socket_bind", b"failed to bind"),
    ("address_in_use", b"address already in use"),
    ("permission_denied", b"permission denied"),
    ("operation_not_permitted", b"operation not permitted"),
    ("path_too_long", b"file name too long"),
    ("missing_file", b"no such file"),
    ("dynamic_loader", b"error while loading shared libraries"),
    ("database_reported", b"database corrupted"),
    ("database_eof", b"unexpected end of file"),
    ("database_line", b"unknown line in db"),
    ("database_format", b"database format mismatch"),
    ("database_tags", b"tag list mismatch"),
    ("output_buffer", b"output buffer"),
    ("invalid_argument", b"invalid argument"),
];
const DIAGNOSTIC_TAIL: usize = 63;
#[derive(Default, Debug)]
struct DiagnosticSummary {
    seen: bool,
    observations: u32,
    complete: bool,
}
#[derive(Clone, Copy)]
enum ValidationFailure {
    StartupExited,
    StartupTimeout,
    CatalogRead,
    Diagnostics,
}
impl ValidationFailure {
    fn code(self) -> &'static str {
        match self {
            Self::StartupExited => "LIBRARY_RECOVERY_DATABASE_VALIDATOR_START_FAILED",
            Self::StartupTimeout => "LIBRARY_RECOVERY_DATABASE_VALIDATOR_START_TIMEOUT",
            Self::CatalogRead => "LIBRARY_RECOVERY_DATABASE_CATALOG_UNCONFIRMED",
            Self::Diagnostics => "LIBRARY_RECOVERY_DATABASE_DIAGNOSTICS_UNCONFIRMED",
        }
    }
}
impl DiagnosticSummary {
    fn ensure_clean(&self) -> Result<(), String> {
        if self.seen || !self.complete {
            Err(self.failure(ValidationFailure::Diagnostics, None))
        } else {
            Ok(())
        }
    }
    fn failure(&self, phase: ValidationFailure, exit: Option<i32>) -> String {
        let observations: Vec<_> = DIAGNOSTIC_PATTERNS
            .iter()
            .enumerate()
            .filter(|(index, _)| self.observations & (1 << index) != 0)
            .map(|(_, (label, _))| *label)
            .collect();
        format!(
            "{}: stderr={}; observations={}; read={}; exit={}",
            phase.code(),
            if self.seen { "present" } else { "absent" },
            if observations.is_empty() {
                "none".into()
            } else {
                observations.join(",")
            },
            if self.complete {
                "complete"
            } else {
                "incomplete"
            },
            exit.map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".into())
        )
    }
}
fn read_diagnostics(mut stream: impl Read) -> DiagnosticSummary {
    let mut summary = DiagnosticSummary::default();
    let mut buffer = [0u8; 4096];
    // Fixed-size streaming window detects phrases split across reads. No raw
    // diagnostics escape this reader; memory use is independent of stderr size.
    let mut window = [0u8; 4096 + DIAGNOSTIC_TAIL];
    let mut tail = 0;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                summary.complete = true;
                return summary;
            }
            Ok(n) => {
                summary.seen = true;
                for (target, byte) in window[tail..tail + n].iter_mut().zip(&buffer[..n]) {
                    *target = byte.to_ascii_lowercase();
                }
                let used = tail + n;
                for (index, (_, pattern)) in DIAGNOSTIC_PATTERNS.iter().enumerate() {
                    if window[..used]
                        .windows(pattern.len())
                        .any(|bytes| bytes == *pattern)
                    {
                        summary.observations |= 1 << index;
                    }
                }
                tail = used.min(DIAGNOSTIC_TAIL);
                window.copy_within(used - tail..used, 0);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return summary,
        }
    }
}
fn finish_diagnostics(probe: &mut Probe) -> Result<DiagnosticSummary, String> {
    // Reap only this owned child before waiting for EOF from its diagnostic pipe.
    reap(probe)?;
    probe
        .diagnostics
        .take()
        .ok_or(INCOMPLETE)?
        .join()
        .map_err(|_| "LIBRARY_RECOVERY_DATABASE_DIAGNOSTICS_UNAVAILABLE".into())
}
fn reap(probe: &mut Probe) -> Result<(), String> {
    if let Some(mut child) = probe.child.take() {
        if child.try_wait().map_err(|_| INCOMPLETE)?.is_none() {
            child.kill().map_err(|_| INCOMPLETE)?;
        }
        child.wait().map_err(|_| INCOMPLETE)?;
    }
    Ok(())
}
impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.diagnostics.take() {
            let _ = reader.join();
        }
        for n in ["mpd.conf", "database", "socket", "pid"] {
            let _ = fs::remove_file(self.dir.join(n));
        }
        let _ = fs::remove_dir(self.dir.join("empty"));
        let _ = fs::remove_dir(&self.dir);
    }
}
pub(crate) fn inspect(
    bytes: &[u8],
    cancelled: &dyn Fn() -> bool,
) -> Result<HashSet<String>, String> {
    recovery_fs::check(cancelled)?;
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .map_err(|_| INCOMPLETE)?;
    let id = recovery_fs::hash(&random);
    let path = PathBuf::from(format!(
        "/tmp/sonante-db-{}-{}",
        unsafe { libc::geteuid() },
        &id[..20]
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|_| INCOMPLETE)?;
    let mut probe = Probe {
        dir: path.clone(),
        child: None,
        diagnostics: None,
    };
    let root = Dir::open(&path)?;
    root.create("empty")?;
    root.write_new("database", bytes)?;
    let quote = |p: PathBuf| -> Result<String, String> {
        let s = p.to_str().ok_or(INCOMPLETE)?;
        if s.contains(['\0', '\r', '\n']) {
            return Err(INCOMPLETE.into());
        }
        Ok(format!(
            "\"{}\"",
            s.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    };
    let config=format!("log_level \"error\"\ndecoder {{\n plugin \"wildmidi\"\n enabled \"no\"\n}}\nmusic_directory {}\ndatabase {{\n plugin \"simple\"\n path {}\n hide_playlist_targets \"no\"\n}}\nbind_to_address {}\npid_file {}\nauto_update \"no\"\nrestore_paused \"no\"\naudio_output {{\n type \"null\"\n name \"Recovery validation\"\n}}\n",quote(path.join("empty"))?,quote(path.join("database"))?,quote(path.join("socket"))?,quote(path.join("pid"))?);
    root.write_new("mpd.conf", config.as_bytes())?;
    let engine = crate::mpd_engine::MpdEngine::resolve()?;
    recovery_fs::check(cancelled)?;
    root.validate()?;
    let sandbox = crate::recovery_sandbox::Sandbox::prepare(&root, engine.program())?;
    let mut command = Command::new(engine.program());
    command
        .arg("--no-daemon")
        .arg(path.join("mpd.conf"))
        .env_remove("LISTEN_FDS")
        .env_remove("LISTEN_PID")
        .env_remove("LISTEN_FDNAMES")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(move || sandbox.enter());
    }
    probe.child = Some(
        command
            .spawn()
            .map_err(|_| "LIBRARY_RECOVERY_DATABASE_VALIDATOR_UNAVAILABLE")?,
    );
    let stderr = probe
        .child
        .as_mut()
        .ok_or(INCOMPLETE)?
        .stderr
        .take()
        .ok_or(INCOMPLETE)?;
    probe.diagnostics = Some(std::thread::spawn(move || read_diagnostics(stderr)));
    let started = Instant::now();
    let socket = path.join("socket");
    loop {
        recovery_fs::check(cancelled)?;
        if let Some(status) = probe
            .child
            .as_mut()
            .ok_or(INCOMPLETE)?
            .try_wait()
            .map_err(|_| INCOMPLETE)?
        {
            let diagnostics = finish_diagnostics(&mut probe)?;
            return Err(diagnostics.failure(ValidationFailure::StartupExited, status.code()));
        }
        if socket.exists() {
            break;
        }
        if started.elapsed() > Duration::from_secs(10) {
            let diagnostics = finish_diagnostics(&mut probe)?;
            return Err(diagnostics.failure(ValidationFailure::StartupTimeout, None));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let catalog = match Catalog::read(socket.to_str().ok_or(INCOMPLETE)?) {
        Ok(catalog) => catalog,
        Err(_) => {
            let diagnostics = finish_diagnostics(&mut probe)?;
            return Err(diagnostics.failure(ValidationFailure::CatalogRead, None));
        }
    };
    recovery_fs::check(cancelled)?;
    root.validate()?;
    let diagnostics = finish_diagnostics(&mut probe)?;
    drop(probe);
    // Preserve the existing fail-closed contract, including unknown stderr and
    // incomplete reads. The diagnostic categories never authorize acceptance.
    diagnostics.ensure_clean()?;
    Ok(catalog.records)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Chunks {
        bytes: std::io::Cursor<Vec<u8>>,
        size: usize,
    }
    impl Read for Chunks {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let size = output.len().min(self.size);
            self.bytes.read(&mut output[..size])
        }
    }
    #[test]
    fn validator_diagnostics_observations_survive_every_read_boundary_without_private_text() {
        for (index, (_, pattern)) in DIAGNOSTIC_PATTERNS.iter().enumerate() {
            assert!(!pattern.is_empty() && pattern.len() <= DIAGNOSTIC_TAIL + 1);
            for size in 1..=pattern.len() + 1 {
                let mut bytes = b"private-path/album-secret X-Plex-Token=private-token ".to_vec();
                bytes.extend(pattern.iter().map(u8::to_ascii_uppercase));
                bytes.extend(b"\xff\x00 private-tag");
                let summary = read_diagnostics(Chunks {
                    bytes: std::io::Cursor::new(bytes),
                    size,
                });
                assert!(summary.seen && summary.complete);
                assert_ne!(summary.observations & (1 << index), 0);
                let safe = summary.failure(ValidationFailure::StartupExited, Some(1));
                for private in [
                    "private-path",
                    "album-secret",
                    "X-Plex",
                    "private-token",
                    "private-tag",
                ] {
                    assert!(!safe.contains(private));
                }
                assert!(safe.len() < 600 && !safe.contains("DATABASE_INVALID"));
            }
        }
    }
    #[test]
    fn validator_diagnostics_phase_is_observed_not_inferred_from_untrusted_message() {
        let summary = read_diagnostics(&b"database corrupted: /private/failed to bind.flac"[..]);
        for (phase, prefix) in [
            (
                ValidationFailure::StartupExited,
                "LIBRARY_RECOVERY_DATABASE_VALIDATOR_START_FAILED:",
            ),
            (
                ValidationFailure::StartupTimeout,
                "LIBRARY_RECOVERY_DATABASE_VALIDATOR_START_TIMEOUT:",
            ),
            (
                ValidationFailure::CatalogRead,
                "LIBRARY_RECOVERY_DATABASE_CATALOG_UNCONFIRMED:",
            ),
            (
                ValidationFailure::Diagnostics,
                "LIBRARY_RECOVERY_DATABASE_DIAGNOSTICS_UNCONFIRMED:",
            ),
        ] {
            let safe = summary.failure(phase, Some(127));
            assert!(safe.starts_with(prefix) && safe.ends_with("exit=127"));
            assert!(safe.contains("socket_bind") && safe.contains("database_reported"));
            assert!(!safe.contains("/private") && !safe.contains("DATABASE_INVALID"));
        }
        let silent = read_diagnostics(&b""[..]);
        assert!(silent
            .failure(ValidationFailure::StartupExited, None)
            .ends_with("stderr=absent; observations=none; read=complete; exit=unknown"));
    }
    #[test]
    fn validator_diagnostics_unknown_and_incomplete_are_still_rejected() {
        let silent = read_diagnostics(&b""[..]);
        assert!(silent.ensure_clean().is_ok());
        let unknown = read_diagnostics(&b"unrecognized private diagnostic"[..]);
        assert_eq!(unknown.observations, 0);
        let error = unknown.ensure_clean().unwrap_err();
        assert!(error.starts_with("LIBRARY_RECOVERY_DATABASE_DIAGNOSTICS_UNCONFIRMED:"));
        assert!(!error.contains("private"));
        struct InterruptedThenError(bool);
        impl Read for InterruptedThenError {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
                } else {
                    Err(std::io::Error::other("private diagnostic reader failure"))
                }
            }
        }
        let incomplete = read_diagnostics(InterruptedThenError(false));
        assert!(!incomplete.seen && !incomplete.complete);
        let error = incomplete.ensure_clean().unwrap_err();
        assert!(error.contains("read=incomplete") && !error.contains("private"));
    }
    #[test]
    fn validator_diagnostics_stream_is_bounded_and_keeps_all_finite_observations() {
        let mut message = Vec::new();
        for (_, pattern) in DIAGNOSTIC_PATTERNS {
            message.extend_from_slice(pattern);
            message.push(b'\n');
        }
        let stream = std::io::repeat(b'x')
            .take(1024 * 1024)
            .chain(std::io::Cursor::new(message));
        let summary = read_diagnostics(stream);
        assert_eq!(summary.observations, (1 << DIAGNOSTIC_PATTERNS.len()) - 1);
        assert!(summary.seen && summary.complete && summary.ensure_clean().is_err());
        assert!(summary.failure(ValidationFailure::Diagnostics, None).len() < 600);
    }
    #[test]
    #[ignore = "owned real MPD/null, synthetic WAV and DB only; no personal runtime"]
    fn real_null_daemon_reopens_immutable_catalog_without_media_or_rescan() {
        let mut random = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut random)
            .unwrap();
        let path = PathBuf::from(format!(
            "/tmp/sonante-seed-{}",
            &recovery_fs::hash(&random)[..20]
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let dir = Dir::open(&path).unwrap();
        let media = dir.create("empty").unwrap();
        let samples = vec![0u8; 88200];
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36u32 + samples.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(44100u32.to_le_bytes());
        wav.extend(88200u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((samples.len() as u32).to_le_bytes());
        wav.extend(samples);
        media.write_new("track.wav", &wav).unwrap();
        let config=format!("log_level \"verbose\"\nmusic_directory \"{}/empty\"\ndb_file \"{}/database\"\nbind_to_address \"{}/socket\"\npid_file \"{}/pid\"\nauto_update \"no\"\naudio_output {{\n type \"null\"\n name \"Seed\"\n}}\n",path.display(),path.display(),path.display(),path.display());
        dir.write_new("mpd.conf", config.as_bytes()).unwrap();
        let engine = crate::mpd_engine::MpdEngine::resolve().unwrap();
        let mut seed = Probe {
            dir: path.clone(),
            diagnostics: None,
            child: Some(
                Command::new(engine.program())
                    .arg("--no-daemon")
                    .arg(path.join("mpd.conf"))
                    .env_remove("LISTEN_FDS")
                    .env_remove("LISTEN_PID")
                    .env_remove("LISTEN_FDNAMES")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            ),
        };
        let deadline = Instant::now();
        while !path.join("socket").exists() {
            assert!(deadline.elapsed() < Duration::from_secs(10));
            assert!(seed.child.as_mut().unwrap().try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(20));
        }
        let socket = path.join("socket");
        let socket = socket.to_str().unwrap();
        let job = crate::audio::AudioEngine::request_mpd_rescan_job(socket).unwrap();
        let started = Instant::now();
        assert!(matches!(
            crate::audio::AudioEngine::wait_for_managed_mpd_update(
                socket,
                Some(job),
                Duration::from_millis(50),
                || started.elapsed() > Duration::from_secs(10)
            ),
            Ok(crate::audio::MpdRefreshCompletion::Completed)
        ));
        let mut before = Catalog::read(socket).unwrap().records;
        let settled = Instant::now();
        while !before.contains("file: track.wav") && settled.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(30));
            before = Catalog::read(socket).unwrap().records;
        }
        assert!(
            before.contains("file: track.wav"),
            "seed catalog: {before:?}"
        );
        let playback = crate::audio::AudioEngine::capture_mpd_playback_snapshot(socket).unwrap();
        assert_eq!(playback.state, crate::audio::PlaybackState::Stopped);
        assert!(playback.uris.is_empty());
        // Reap the owned seed before taking its immutable, durable DB bytes.
        seed.child.as_mut().unwrap().kill().unwrap();
        seed.child.as_mut().unwrap().wait().unwrap();
        seed.child = None;
        let db = fs::read(path.join("database")).unwrap();
        let hash = recovery_fs::hash(&db);
        fs::remove_file(path.join("empty/track.wav")).unwrap();
        let observed = inspect(&db, &|| false).unwrap();
        assert_eq!(observed, before);
        assert_eq!(
            recovery_fs::hash(&fs::read(path.join("database")).unwrap()),
            hash
        );
        assert!(inspect(&db, &|| true).is_err());
        assert!(inspect(b"invalid database", &|| false).is_err());
        drop(seed);
    }
}
