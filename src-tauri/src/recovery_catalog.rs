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
    diagnostics: Option<std::thread::JoinHandle<Result<bool, std::io::Error>>>,
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
    probe.diagnostics = Some(std::thread::spawn(move || {
        let mut stream = stderr;
        let mut buffer = [0u8; 4096];
        let mut any = false;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => return Ok::<bool, std::io::Error>(any),
                Ok(n) => {
                    #[cfg(test)]
                    eprintln!(
                        "isolated validator: {}",
                        String::from_utf8_lossy(&buffer[..n])
                    );
                    let _ = n;
                    any = true
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }));
    let started = Instant::now();
    let socket = path.join("socket");
    loop {
        recovery_fs::check(cancelled)?;
        if probe
            .child
            .as_mut()
            .ok_or(INCOMPLETE)?
            .try_wait()
            .map_err(|_| INCOMPLETE)?
            .is_some()
        {
            return Err("LIBRARY_RECOVERY_DATABASE_INVALID".into());
        }
        if socket.exists() {
            break;
        }
        if started.elapsed() > Duration::from_secs(10) {
            return Err("LIBRARY_RECOVERY_DATABASE_VALIDATOR_UNAVAILABLE".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let catalog = Catalog::read(socket.to_str().ok_or(INCOMPLETE)?)?;
    recovery_fs::check(cancelled)?;
    root.validate()?;
    // Child owns only this scratch copy. Drop reaps it before any scratch cleanup.
    reap(&mut probe)?;
    let has_diagnostics = probe
        .diagnostics
        .take()
        .ok_or(INCOMPLETE)?
        .join()
        .map_err(|_| INCOMPLETE)?
        .map_err(|_| INCOMPLETE)?;
    drop(probe);
    if has_diagnostics {
        return Err("LIBRARY_RECOVERY_DATABASE_INVALID".into());
    }
    Ok(catalog.records)
}
#[cfg(test)]
mod tests {
    use super::*;
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
