//! Main-window, Shared-only sample preparation. User files are read through an
//! admitted retained FD; no queue replacement, route switch or automatic resume.
use crate::ab_comparison::{self as ab, Check, Input, Side, Track};
use crate::album_file_sizes::Request as Source;
use crate::*;
use std::{
    fs::{self, File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
fn main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("AB_UNAVAILABLE".into())
    }
}
fn shared(app: &AppHandle) -> Result<(), String> {
    crate::require_experimental_features(app).map_err(|_| "AB_CHANGED")?;
    let config = app.state::<ConfigState>();
    let config = config.0.lock().map_err(|_| "AB_UNAVAILABLE")?;
    if config.first_run || !matches!(config.audio_output_type.as_str(), "pipewire" | "shared") {
        return Err("AB_SHARED_ONLY".into());
    }
    Ok(())
}
// Source configuration is bound as well as the connection manager generation.
// Replacing the Plex client cannot keep an old prepared account alive.
fn binding(config: &AppConfig, source: &Source) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    if !config.experimental_features || config.first_run { return Err("AB_CHANGED".into()); }
    let values = match source {
        Source::Local { .. } => serde_json::to_vec(&config.local_folders),
        Source::Plex { server_id, .. } => {
            if config.plex_server_id.as_deref() != Some(server_id.as_str())
                || config.plex_token.is_empty()
            {
                return Err("AB_CHANGED".into());
            }
            serde_json::to_vec(&(&config.plex_server_id, &config.plex_url, &config.plex_token))
        }
        _ => return Err("AB_SOURCE".into()),
    }
    .map_err(|_| "AB_UNAVAILABLE")?;
    Ok(format!("{:x}", Sha256::digest(values)))
}
fn io_slots() -> &'static tokio::sync::Semaphore {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    &SLOTS
}
fn enabled(source: &Source) -> bool {
    match source {
        Source::Local { .. } => music_sources::local(),
        Source::Plex { .. } => music_sources::plex(),
        Source::Navidrome { .. } => false,
    }
}
fn local_tracks(app: &AppHandle, album: &str) -> Result<Vec<audio::LocalItem>, String> {
    let (tracks, _) = with_local_source_context(app, |before, recheck, _| {
        if before.observed_conflict {
            return Err("AB_CHANGED".into());
        }
        let socket = app
            .state::<AudioState>()
            .0
            .try_lock()
            .map_err(|_| "AB_BUSY")?
            .local_album_socket_path();
        if Path::new(&socket) != MpdSupervisor::socket_path() {
            return Err("AB_CHANGED".into());
        }
        let groups = AudioEngine::get_local_album_tracks_at(&socket, album)?.ok_or("AB_TRACKS")?;
        let tracks = groups
            .into_iter()
            .flat_map(|g| g.tracks)
            .filter(|t| t.item_type == "file")
            .collect::<Vec<_>>();
        let after = recheck()?;
        if after.observed_conflict
            || before.sources != after.sources
            || tracks.is_empty()
            || tracks.len() > 500
        {
            return Err("AB_CHANGED".into());
        }
        Ok(tracks)
    })?;
    Ok(tracks)
}
fn local_descriptor(item: &audio::LocalItem, i: usize) -> Result<Track, String> {
    ab::track(
        &item.path,
        item.title.as_deref().unwrap_or(&item.name),
        item.duration,
        i + 1,
    )
}
fn client(app: &AppHandle) -> Result<plex::PlexClient, String> {
    app.state::<PlexState>()
        .0
        .lock()
        .map(|c| c.clone())
        .map_err(|_| "AB_UNAVAILABLE".into())
}
#[tauri::command]
pub(crate) async fn ab_tracks(
    window: tauri::WebviewWindow,
    app: AppHandle,
    source: Source,
) -> Result<Vec<Track>, String> {
    main(&window)?;
    shared(&app)?;
    let _slot = io_slots().try_acquire().map_err(|_| "AB_BUSY")?;
    if !enabled(&source) {
        return Err("AB_SOURCE".into());
    }
    match source {
        Source::Local { album_id } => tauri::async_runtime::spawn_blocking(move || {
            local_tracks(&app, &album_id)?
                .iter()
                .enumerate()
                .map(|(i, t)| local_descriptor(t, i))
                .collect()
        })
        .await
        .map_err(|_| "AB_UNAVAILABLE".to_string())?,
        Source::Plex {
            server_id,
            library_id,
            album_id,
        } => {
            client(&app)?
                .ab_tracks(&server_id, &library_id, &album_id)
                .await
        }
        _ => Err("AB_SOURCE".into()),
    }
}
#[tauri::command]
pub(crate) fn ab_begin(window: tauri::WebviewWindow, app: AppHandle) -> Result<String, String> {
    main(&window)?;
    shared(&app)?;
    ab::begin()
}
#[derive(Clone, PartialEq)]
struct Stamp {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
fn stamp(meta: std::fs::Metadata) -> Option<Stamp> {
    meta.is_file().then(|| Stamp {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime: (meta.mtime(), meta.mtime_nsec()),
        ctime: (meta.ctime(), meta.ctime_nsec()),
    })
}
fn open_local(root: PathBuf, path: PathBuf) -> Result<(File, Check), String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|_| "AB_SOURCE")?;
    let before = stamp(file.metadata().map_err(|_| "AB_SOURCE")?)
        .filter(|s| s.len > 0)
        .ok_or("AB_SOURCE")?;
    let retained = file.try_clone().map_err(|_| "AB_SOURCE")?;
    let check: Check = Arc::new(move || {
        let actual = fs::read_link(format!("/proc/self/fd/{}", retained.as_raw_fd())).ok();
        actual.as_ref() == Some(&path)
            && path.starts_with(&root)
            && retained.metadata().ok().and_then(stamp).as_ref() == Some(&before)
            && fs::metadata(&path).ok().and_then(stamp).as_ref() == Some(&before)
    });
    if !check() {
        return Err("AB_CHANGED".into());
    }
    Ok((file, check))
}
fn local_input(app: &AppHandle, album: &str, key: &str) -> Result<Input, String> {
    let tracks = local_tracks(app, album)?;
    let item = tracks
        .into_iter()
        .enumerate()
        .find_map(|(i, t)| {
            local_descriptor(&t, i)
                .ok()
                .filter(|t| t.key == key)
                .map(|_| t)
        })
        .ok_or("AB_CHANGED")?;
    let ((file, validate, folders), _) =
        with_local_source_context(app, |before, recheck, context| {
            if before.observed_conflict
                || !local_media::is_catalog_track_uri(&item.path)
                || local_media::is_virtual_cue_track_uri(&item.path)
            {
                return Err("AB_SOURCE".into());
            }
            let resolved =
                local_media::resolve_local_path(context.library, &item.path, context.configured)
                    .map_err(|_| "AB_SOURCE")?;
            let (file, validate) = open_local(resolved.root, resolved.physical_path)?;
            let after = recheck()?;
            if after.observed_conflict || before.sources != after.sources || !validate() {
                return Err("AB_CHANGED".into());
            }
            Ok((file, validate, context.configured.to_vec()))
        })?;
    let proof_app = app.clone();
    let file_check = validate;
    let validate: Check = Arc::new(move || {
        file_check()
            && proof_app
                .state::<profile_lease::State>()
                .lease()
                .is_ok_and(|lease| lease.validate().is_ok())
    });
    let policy_app = app.clone();
    let alive: Check = Arc::new(move || {
        music_sources::local()
            && policy_app.state::<ConfigState>().0.lock().is_ok_and(|c| {
                !c.first_run
                    && matches!(c.audio_output_type.as_str(), "shared" | "pipewire")
                    && c.local_folders == folders
            })
    });
    Ok(Input {
        media: Box::new(file),
        alive,
        validate,
        identity: format!("local:{key}"),
    })
}
#[tauri::command]
pub(crate) async fn ab_load(
    window: tauri::WebviewWindow,
    app: AppHandle,
    token: String,
    side: Side,
    source: Source,
    track_key: String,
) -> Result<(), String> {
    main(&window)?;
    shared(&app)?;
    if !enabled(&source)
        || track_key.len() != 64
        || !track_key.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("AB_SOURCE".into());
    }
    let _slot = io_slots().try_acquire().map_err(|_| "AB_BUSY")?;
    let captured_source = source.clone();
    let expected_binding = {
        let state = app.state::<ConfigState>();
        let config = state.0.lock().map_err(|_| "AB_UNAVAILABLE")?;
        binding(&config, &captured_source)?
    };
    let job = ab::job(&token)?;
    let valid_job = job.clone();
    let current: Check = Arc::new(move || valid_job.current());
    let work = async {
        Ok::<Input, String>(match source {
            Source::Local { album_id } => {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    local_input(&app, &album_id, &track_key)
                })
                .await
                .map_err(|_| "AB_UNAVAILABLE".to_string())??
            }
            Source::Plex {
                server_id,
                library_id,
                album_id,
            } => {
                client(&app)?
                    .ab_input(&server_id, &library_id, &album_id, &track_key, current)
                    .await?
            }
            _ => return Err("AB_SOURCE".into()),
        })
    };
    tokio::pin!(work);
    let mut input = loop {
        tokio::select! {result=&mut work=>break result?,_=tokio::time::sleep(std::time::Duration::from_millis(100))=>{if !job.current(){return Err("AB_CHANGED".into());}}}
    };
    shared(&app)?;
    let policy_app = app.clone();
    let previous = input.alive.clone();
    input.alive = Arc::new(move || {
        previous()
            && policy_app
                .state::<ConfigState>()
                .0
                .lock()
                .is_ok_and(|config| {
                    binding(&config, &captured_source)
                        .is_ok_and(|actual| actual == expected_binding)
                })
    });
    job.install(side, input)
}
#[tauri::command]
pub(crate) async fn ab_decode(
    window: tauri::WebviewWindow,
    app: AppHandle,
    token: String,
    side: Side,
    start: f64,
    seconds: u32,
) -> Result<tauri::ipc::Response, ab::DecodeFailure> {
    main(&window)?;
    shared(&app)?;
    ab::parameters(start, seconds)?;
    let job = ab::job(&token)?;
    let local = job.local_input(side)?;
    let decode_app = app.clone();
    let bytes = tauri::async_runtime::spawn_blocking(
        move || -> Result<Result<Vec<u8>, ab::DecodeFailure>, String> {
            if !local {
                return Ok(ab::decode(job, side, start, seconds));
            }
            // Keep the existing profile/transaction/source fence through EVERY
            // payload read. No Audio mutex or frontend read-queue slot is held by
            // this worker. Shared transport uses its established separate gate.
            let (bytes, _) = with_local_source_context(&decode_app, |before, recheck, _| {
                if before.observed_conflict {
                    return Err("AB_CHANGED".into());
                }
                let bytes = ab::decode(job, side, start, seconds);
                let after = recheck()?;
                if after.observed_conflict || before.sources != after.sources {
                    return Err("AB_CHANGED".into());
                }
                Ok(bytes)
            })?;
            Ok(bytes)
        },
    )
    .await
    .map_err(|_| "AB_UNAVAILABLE".to_string())??;
    let bytes = bytes?;
    shared(&app)?;
    ab::job(&token)?;
    Ok(tauri::ipc::Response::new(bytes))
}
#[derive(Clone)]
struct Active {
    token: String,
    intent: String,
    watch: audio::EqTransitionWatch,
}
fn active() -> &'static Mutex<Option<Active>> {
    static ACTIVE: OnceLock<Mutex<Option<Active>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}
#[tauri::command]
pub(crate) fn ab_start(
    window: tauri::WebviewWindow,
    app: AppHandle,
    token: String,
    intent: String,
) -> Result<(), String> {
    main(&window)?;
    shared(&app)?;
    let job = ab::job(&token)?;
    if !job.valid() {
        return Err("AB_CHANGED".into());
    }
    let transactions = app.state::<ConfigTransactionState>();
    let audio = app.state::<AudioState>();
    let mut audio = audio.0.lock().map_err(|_| "AB_UNAVAILABLE")?;
    if initial_playback_context(&transactions, Some(&intent))?.is_some() {
        return Err("AB_BUSY".into());
    }
    let watch = audio.pause_for_ab(&intent)?;
    drop(audio);
    if !watch.current() || !job.valid() {
        return Err("AB_CHANGED".into());
    }
    *active().lock().map_err(|_| "AB_UNAVAILABLE")? = Some(Active {
        token,
        intent,
        watch,
    });
    Ok(())
}
#[tauri::command]
pub(crate) fn ab_alive(
    window: tauri::WebviewWindow,
    app: AppHandle,
    token: String,
    intent: String,
) -> bool {
    if main(&window).is_err() || shared(&app).is_err() || !ab::job(&token).is_ok_and(|j| j.valid())
    {
        return false;
    }
    let session = active().lock().ok().and_then(|s| {
        s.as_ref()
            .filter(|s| s.token == token && s.intent == intent)
            .cloned()
    });
    session.is_some_and(|s| {
        s.watch.current()
            && app
                .state::<AudioState>()
                .0
                .lock()
                .is_ok_and(|audio| audio.validate_playback_intent(&s.intent).is_ok())
    })
}
#[tauri::command]
pub(crate) fn ab_end(window: tauri::WebviewWindow, token: String, intent: String) {
    if main(&window).is_err() {
        return;
    }
    if let Ok(mut active) = active().lock() {
        if active
            .as_ref()
            .is_some_and(|s| s.token == token && s.intent == intent)
        {
            active.take();
        }
    }
}
#[tauri::command]
pub(crate) fn ab_cancel(window: tauri::WebviewWindow, token: String) {
    if main(&window).is_err() {
        return;
    }
    ab::cancel(&token);
    if let Ok(mut active) = active().lock() {
        if active.as_ref().is_some_and(|s| s.token == token) {
            active.take();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn ab_prepared_source_binding_requires_the_accepted_experimental_preference() {
        let mut config = AppConfig { first_run: false, local_folders: vec!["/synthetic".into()], ..AppConfig::default() };
        let source = Source::Local { album_id: "synthetic-album".into() };
        assert_eq!(binding(&config, &source).unwrap_err(), "AB_CHANGED");
        config.experimental_features = true;
        let accepted = binding(&config, &source).unwrap();
        config.experimental_features = false;
        assert_eq!(binding(&config, &source).unwrap_err(), "AB_CHANGED");
        config.experimental_features = true;
        assert_eq!(binding(&config, &source).unwrap(), accepted);
        config.first_run = true;
        assert_eq!(binding(&config, &source).unwrap_err(), "AB_CHANGED");
    }
    #[test]
    fn retained_local_input_detects_replacement_and_symlink_escape() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("fixture.wav");
        fs::write(&path, [1u8; 64]).unwrap();
        let (_file, validate) = open_local(root.clone(), path.clone()).unwrap();
        assert!(validate());
        fs::rename(&path, root.join("old.wav")).unwrap();
        fs::write(&path, [1u8; 64]).unwrap();
        assert!(!validate());
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("outside.wav"), [1u8; 64]).unwrap();
        let link = root.join("link.wav");
        symlink(outside.path().join("outside.wav"), &link).unwrap();
        assert!(open_local(root, link).is_err());
    }
}
