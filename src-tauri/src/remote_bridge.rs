//! LAN commands share native playback admission, queue revisions and volume policy.
mod network;
use crate::{
    audio::{CurrentMedia, PlaybackStatus, VolumeBackend},
    remote::{self, Backend, Command, Guard, QueuePage, QueueTrack, Snapshot},
    *,
};
use std::{collections::HashMap, sync::{Arc, Mutex}};
use tauri::{AppHandle, Manager};
#[derive(Clone)]
struct Media {
    key: String,
    current: Option<CurrentMedia>,
    thumb: Option<String>,
    plex: Option<plex::PlexImageRef>,
}
#[derive(Default)]
pub(crate) struct MediaState {
    current: Mutex<Option<Media>>,
    art: Mutex<Option<(String, String, Vec<u8>)>>,
    queue_art: Mutex<QueueArtworkIndex>,
}
#[derive(Clone)]
struct QueueArtworkRef { revision: String, index: usize, identity: String }
#[derive(Default)]
struct QueueArtworkIndex { revision: String, entries: HashMap<String, QueueArtworkRef> }
fn clean(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}
fn key(media: &Option<CurrentMedia>) -> String {
    remote::digest(&serde_json::to_vec(media).unwrap_or_default())
}
fn queue_track_identity(track: &audio::TrackMetadata) -> String {
    remote::digest(&serde_json::to_vec(&(track.media_locator.as_ref(), track.uri.as_str())).unwrap_or_default())
}
fn queue_wire_revision(audio_revision: &str) -> String {
    format!("q1:{}", remote::digest(audio_revision.as_bytes()))
}
fn queue_artwork_key(revision: &str, index: usize, track: &audio::TrackMetadata) -> String {
    remote::digest(&serde_json::to_vec(&(revision, index, queue_track_identity(track))).unwrap_or_default())
}
fn queue_track_has_artwork(track: &audio::TrackMetadata) -> bool {
    if track.thumb.as_deref().is_some_and(|value| image(value).is_some()) || track.plex_image.is_some() { return true; }
    match &track.media_locator {
        Some(audio::MediaLocator::Local { uri }) => !uri.is_empty(),
        Some(audio::MediaLocator::Navidrome { connection_id, song_id, .. }) => !connection_id.is_empty() && !song_id.is_empty(),
        Some(audio::MediaLocator::Plex { .. } | audio::MediaLocator::Upnp { .. } | audio::MediaLocator::Radio { .. }) => false,
        None => !track.uri.is_empty() && !track.uri.starts_with("http://") && !track.uri.starts_with("https://"),
    }
}
fn project_queue_page(value: audio::QueueEditSnapshot, offset: usize, artwork: &mut QueueArtworkIndex) -> Result<QueuePage, String> {
    let total = value.tracks.len();
    if offset > total { return Err("REMOTE_CHANGED".into()); }
    let audio_revision = value.revision;
    let revision = queue_wire_revision(&audio_revision);
    if artwork.revision != audio_revision {
        artwork.revision.clone_from(&audio_revision);
        artwork.entries.clear();
    }
    let tracks = value.tracks.into_iter().enumerate().skip(offset).take(60).map(|(index, track)| {
        let art_key = queue_track_has_artwork(&track).then(|| queue_artwork_key(&revision, index, &track));
        if let Some(art_key) = &art_key {
            artwork.entries.insert(art_key.clone(), QueueArtworkRef {
                revision: audio_revision.clone(), index, identity: queue_track_identity(&track),
            });
        }
        QueueTrack {
            index,
            title: clean(&track.title, 512),
            artist: clean(&track.artist, 512),
            album: clean(&track.album, 512),
            duration: track.duration.filter(|value| value.is_finite() && *value > 0.),
            artwork: art_key,
        }
    }).collect();
    Ok(QueuePage { revision, total, offset, tracks })
}
fn index(media: &Option<CurrentMedia>) -> Option<usize> {
    match media {
        Some(
            CurrentMedia::Local { queue_index, .. }
            | CurrentMedia::Plex { queue_index, .. }
            | CurrentMedia::Navidrome { queue_index, .. }
            | CurrentMedia::Radio { queue_index, .. }
            | CurrentMedia::Upnp { queue_index, .. },
        ) => Some(*queue_index),
        _ => None,
    }
}
fn public(status: &PlaybackStatus) -> Snapshot {
    Snapshot {
        schema: 1,
        available: status.snapshot_consistent && status.playback_error.is_none(),
        state: match status.state.as_str() {
            "play" => "play",
            "pause" => "pause",
            _ => "stop",
        }
        .into(),
        elapsed: if status.elapsed.is_finite() {
            status.elapsed.max(0.)
        } else {
            0.
        },
        duration: if status.duration.is_finite() {
            status.duration.max(0.)
        } else {
            0.
        },
        media: key(&status.current_media),
        title: clean(&status.title, 512),
        artist: clean(&status.artist, 512),
        album: clean(&status.album, 512),
        source: match &status.current_media {
            Some(CurrentMedia::Local { .. }) => "Local",
            Some(CurrentMedia::Plex { .. }) => "Plex",
            Some(CurrentMedia::Navidrome { .. }) => "Navidrome",
            Some(CurrentMedia::Radio { .. }) => "Radio",
            Some(CurrentMedia::Upnp { .. }) => "UPnP",
            _ => "",
        }
        .into(),
        audio_format: clean(&status.audio_format, 100),
        index: index(&status.current_media),
        volume: status.volume.value.min(100),
        muted: status.volume.muted,
        volume_writable: status.volume.available
            && status.volume.writable
            && !matches!(status.volume.backend, VolumeBackend::DirectEquipment),
    }
}
pub(crate) fn publish(app: &AppHandle, status: &PlaybackStatus) {
    if crate::null_output_test_mode() {
        return;
    }
    let Some(state) = app.try_state::<remote::State>() else {
        return;
    };
    if !state.enabled.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let frame = public(status);
    if let Some(media) = app.try_state::<MediaState>() {
        if let Ok(mut stored) = media.current.lock() {
            *stored = Some(Media {
                key: frame.media.clone(),
                current: status.current_media.clone(),
                thumb: status.thumb.clone(),
                plex: status.plex_image.clone(),
            });
        }
    }
    state.publish(frame);
}
fn require_main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("REMOTE_UNAVAILABLE".into())
    }
}
fn configured(app: &AppHandle) -> Result<(), String> {
    require_network_services_enabled()?;
    let state = app.state::<ConfigState>();
    let config = state.0.lock().map_err(|_| "REMOTE_BUSY")?;
    if !config.experimental_features_enabled() {
        Err("REMOTE_UNAVAILABLE".into())
    } else {
        Ok(())
    }
}
fn require_network_services_enabled() -> Result<(), String> {
    network_services_enabled(crate::null_output_test_mode())
}
fn network_services_enabled(null_output_test: bool) -> Result<(), String> {
    if null_output_test {
        Err("NULL_OUTPUT_TEST_SERVICES_DISABLED".into())
    } else {
        Ok(())
    }
}
pub(crate) fn media_matches(audio: &audio::AudioEngine, expected: &str) -> Result<(), String> {
    let status = audio.get_telemetry_status()?;
    if !status.snapshot_consistent || key(&status.current_media) != expected {
        return Err("REMOTE_CHANGED".into());
    }
    Ok(())
}
fn mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}
fn image(value: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine;
    if value.len() > 3 * 1024 * 1024 {
        return None;
    }
    let (prefix, data) = value.split_once(',')?;
    if !matches!(
        prefix,
        "data:image/png;base64" | "data:image/jpeg;base64" | "data:image/webp;base64"
    ) {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    if bytes.len() > 2 * 1024 * 1024 {
        return None;
    }
    Some((mime(&bytes)?.into(), bytes))
}
struct Adapter(AppHandle);
impl Backend for Adapter {
    fn credential_path(&self) -> Option<std::path::PathBuf> {
        Some(crate::persistence::sonante_config_dir().join("remote-clients.json"))
    }
    fn active(&self) -> bool {
        configured(&self.0).is_ok()
            && self
                .0
                .try_state::<ConfigTransactionState>()
                .is_some_and(|v| !v.shutdown_requested() && !v.shutdown_started())
    }
    fn queue(&self, offset: usize) -> Result<QueuePage, String> {
        configured(&self.0)?;
        let value = tauri::async_runtime::block_on(crate::get_queue_edit_snapshot(self.0.clone()))?;
        let media = self.0.state::<MediaState>();
        let mut artwork = media.queue_art.lock().map_err(|_| "REMOTE_BUSY")?;
        project_queue_page(value, offset, &mut artwork)
    }
    fn control(&self, command: Command, current: Guard) -> Result<(), String> {
        if !current() || !command.valid() {
            return Err("REMOTE_CHANGED".into());
        }
        configured(&self.0)?;
        let app = &self.0;
        let transactions = app.state::<ConfigTransactionState>();
        if transactions.shutdown_requested() || transactions.shutdown_started() {
            return Err("REMOTE_BUSY".into());
        }
        transactions.operations.reject_playback()?;
        let audio = app.state::<AudioState>();
        audio.note_transport_intervention();
        match command {
            Command::Volume { value } => {
                if !current() {
                    return Err("REMOTE_CHANGED".into());
                }
                // Share the desktop policy and dispatch, with revocation checked
                // after waiting for the output/audio locks and before the write.
                let _permit = crate::begin_volume_transaction(&transactions)?;
                let supervisor = app.state::<SupervisorState>();
                let supervisor = supervisor.0.lock().map_err(|_| "REMOTE_BUSY")?;
                crate::require_gradual_volume(supervisor.volume_backend())?;
                let backend = supervisor.shared_volume_backend();
                drop(supervisor);
                crate::set_volume_with_backend(
                    backend,
                    value,
                    |value| {
                        if !current() {
                            return Err("REMOTE_CHANGED".into());
                        }
                        shared_volume::set_pipewire_volume(value)
                    },
                    |value| {
                        let engine = audio.0.lock().map_err(|_| "REMOTE_BUSY")?;
                        if !current() {
                            return Err("REMOTE_CHANGED".into());
                        }
                        engine.set_volume(value)
                    },
                )?;
            }
            command => {
                let shared = {
                    let state = app.state::<ConfigState>();
                    let config = state.0.lock().map_err(|_| "REMOTE_BUSY")?;
                    crate::is_shared_output(&config)
                };
                let expected = match &command {
                    Command::Play { media }
                    | Command::Pause { media }
                    | Command::Next { media }
                    | Command::Previous { media }
                    | Command::Seek { media, .. } => Some(media.clone()),
                    _ => None,
                };
                let check = |engine: &audio::AudioEngine| {
                    if !current() {
                        return Err("REMOTE_CHANGED".into());
                    }
                    if let Some(expected) = &expected {
                        media_matches(engine, expected)?;
                    }
                    Ok(())
                };
                let token = crate::admit_or_validate_playback_checked(
                    &audio.0,
                    &transactions,
                    None,
                    &check,
                )?;
                crate::finish_playback_intent(&audio.0, &transactions, &token, |engine| {
                    check(engine)?;
                    match command {
                        Command::Play { .. } => engine.play(shared),
                        Command::Pause { .. } => engine.pause(),
                        Command::Next { .. } => engine.next(shared),
                        Command::Previous { .. } => engine.previous(shared),
                        Command::Seek { seconds, .. } => engine.seek(seconds, shared),
                        Command::QueuePlay { index, revision } => {
                            let fresh = engine.queue_edit_snapshot()?;
                            if queue_wire_revision(&fresh.revision) != revision || index >= fresh.tracks.len() {
                                return Err("QUEUE_EDIT_CHANGED".into());
                            }
                            if !current() {
                                return Err("REMOTE_CHANGED".into());
                            }
                            engine.play_index(index, shared)
                        }
                        _ => Err("REMOTE_INVALID".into()),
                    }
                })?;
            }
        }
        // A single informational refresh after an accepted action; polling still
        // belongs to the existing desktop telemetry loop. Failure cannot undo ACK.
        let _ = tauri::async_runtime::block_on(crate::get_playback_status(app.clone(), None));
        Ok(())
    }
    fn artwork(&self, wanted: &str, current: Guard) -> Result<Option<(String, Vec<u8>)>, String> {
        if !current() {
            return Err("REMOTE_CHANGED".into());
        }
        let cache = self.0.state::<MediaState>();
        let media = cache
            .current
            .lock()
            .map_err(|_| "REMOTE_BUSY")?
            .as_ref()
            .filter(|v| v.key == wanted)
            .cloned()
            .ok_or("REMOTE_CHANGED")?;
        if let Some((_, kind, data)) = cache
            .art
            .lock()
            .map_err(|_| "REMOTE_BUSY")?
            .as_ref()
            .filter(|v| v.0 == wanted)
        {
            return Ok(Some((kind.clone(), data.clone())));
        }
        let result = if let Some(value) = media.thumb.as_deref().and_then(image) {
            Some(value)
        } else if let Some(reference) = media.plex {
            if !music_sources::plex() {
                return Err("REMOTE_SOURCE_DISABLED".into());
            }
            let client = self
                .0
                .state::<PlexState>()
                .0
                .lock()
                .map_err(|_| "REMOTE_BUSY")?
                .clone();
            let bytes = tauri::async_runtime::block_on(client.get_image(&reference))?;
            if bytes.len() > 2 * 1024 * 1024 {
                return Ok(None);
            }
            mime(&bytes).map(|m| (m.into(), bytes))
        } else {
            match media.current {
                Some(CurrentMedia::Local { uri, .. }) => tauri::async_runtime::block_on(
                    crate::get_local_cover(uri, self.0.clone(), None),
                )?
                .as_deref()
                .and_then(image),
                Some(CurrentMedia::Navidrome {
                    connection_id,
                    song_id,
                    ..
                }) => {
                    let id = tauri::async_runtime::block_on(navidrome::navidrome_song_cover_id(
                        connection_id.clone(),
                        song_id,
                    ))?;
                    match id {
                        Some(id) => image(&tauri::async_runtime::block_on(
                            navidrome::navidrome_cover(connection_id, id),
                        )?),
                        None => None,
                    }
                }
                _ => None,
            }
        };
        if !current()
            || cache
                .current
                .lock()
                .map_err(|_| "REMOTE_BUSY")?
                .as_ref()
                .is_none_or(|v| v.key != wanted)
        {
            return Err("REMOTE_CHANGED".into());
        }
        if let Some((kind, data)) = &result {
            *cache.art.lock().map_err(|_| "REMOTE_BUSY")? =
                Some((wanted.into(), kind.clone(), data.clone()));
        }
        Ok(result)
    }
    fn queue_artwork(&self, wanted: &str, current: Guard) -> Result<Option<(String, Vec<u8>)>, String> {
        if !current() { return Err("REMOTE_CHANGED".into()); }
        let entry = self.0.state::<MediaState>().queue_art.lock().map_err(|_| "REMOTE_BUSY")?
            .entries.get(wanted).cloned().ok_or("REMOTE_CHANGED")?;
        let fresh = tauri::async_runtime::block_on(crate::get_queue_edit_snapshot(self.0.clone()))?;
        if !current() || fresh.revision != entry.revision { return Err("REMOTE_CHANGED".into()); }
        let Some(track) = fresh.tracks.get(entry.index) else { return Err("REMOTE_CHANGED".into()); };
        if queue_track_identity(track) != entry.identity || queue_artwork_key(&queue_wire_revision(&fresh.revision), entry.index, track) != wanted {
            return Err("REMOTE_CHANGED".into());
        }
        let result = if let Some(value) = track.thumb.as_deref().and_then(image) {
            Some(value)
        } else if let Some(reference) = track.plex_image.as_ref() {
            if !music_sources::plex() { return Ok(None); }
            let client = self.0.state::<PlexState>().0.lock().map_err(|_| "REMOTE_BUSY")?.clone();
            let bytes = tauri::async_runtime::block_on(client.get_image(reference))?;
            if bytes.len() > 2 * 1024 * 1024 { None } else { mime(&bytes).map(|kind| (kind.into(), bytes)) }
        } else {
            match track.media_locator.as_ref() {
                Some(audio::MediaLocator::Local { uri }) if uri.starts_with("sqlite-track://") || uri.starts_with("sqlite-cue://") => {
                    let locator = uri.clone();
                    let library = self.0.state::<SqliteLibraryState>().inner().clone();
                    let cover = tauri::async_runtime::block_on(tauri::async_runtime::spawn_blocking(move || library.album_cover_for_track_locator(&locator)))
                        .map_err(|_| "REMOTE_UNAVAILABLE")??;
                    cover.as_deref().and_then(image)
                }
                Some(audio::MediaLocator::Local { uri }) => tauri::async_runtime::block_on(
                    crate::get_local_cover(uri.clone(), self.0.clone(), None),
                )?.as_deref().and_then(image),
                Some(audio::MediaLocator::Navidrome { connection_id, song_id, .. }) => {
                    let id = tauri::async_runtime::block_on(navidrome::navidrome_song_cover_id(connection_id.clone(), song_id.clone()))?;
                    match id {
                        Some(id) => image(&tauri::async_runtime::block_on(navidrome::navidrome_cover(connection_id.clone(), id))?),
                        None => None,
                    }
                }
                _ if track.media_locator.is_none() && !track.uri.is_empty()
                    && !track.uri.starts_with("http://") && !track.uri.starts_with("https://") => tauri::async_runtime::block_on(
                        crate::get_local_cover(track.uri.clone(), self.0.clone(), None),
                    )?.as_deref().and_then(image),
                _ => None,
            }
        };
        if !current() { return Err("REMOTE_CHANGED".into()); }
        Ok(result.filter(|(_, bytes)| bytes.len() <= 2 * 1024 * 1024))
    }
}
fn path() -> std::path::PathBuf {
    crate::persistence::sonante_config_dir().join("remote-control.json")
}
fn addresses() -> Vec<std::net::Ipv4Addr> {
    network::addresses()
}

fn status(app: &AppHandle) -> Result<remote::Status, String> {
    app.state::<remote::State>().status(addresses())
}
// Config publication never joins a worker while holding config/audio locks.
// API admission reads the accepted flag immediately; cleanup then closes the port.
pub(crate) fn beta_preference_changed(app: &AppHandle, previous: bool, next: bool) {
    if crate::null_output_test_mode() {
        return;
    }
    if !previous || next {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| -> Result<(), String> {
            let state = app.state::<remote::State>();
            let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
            // A newer accepted preference and explicit network enable supersede cleanup.
            if configured(&app).is_ok() {
                return Ok(());
            }
            let prefs = remote::Preferences {
                enabled: false,
                port: state.preferences()?.port,
            };
            state.configure(prefs.clone(), Arc::new(Adapter(app.clone())))?;
            crate::persistence::atomic_write_private(
                &path(),
                &serde_json::to_vec(&prefs).map_err(|_| "REMOTE_PREFERENCES")?,
                "remote-control.json",
            )
            .map_err(|_| "REMOTE_PREFERENCES".to_string())
        })();
        if result.is_err() {
            eprintln!("[Remote] disable=unavailable");
        }
    });
}
pub(crate) fn usage_mode_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || restore(&app));
}
pub(crate) fn restore(app: &AppHandle) {
    if crate::null_output_test_mode() {
        app.state::<remote::State>().shutdown();
        return;
    }
    let transactions = app.state::<ConfigTransactionState>();
    if transactions.shutdown_requested() || transactions.shutdown_started() {
        return;
    }
    // Repair a shutdown/crash between saving beta=false and asynchronous network
    // cleanup: disabled beta must not restore an old durable grant on re-enable.
    let result = (|| -> Result<(), String> {
        let state = app.state::<remote::State>();
        let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
        if transactions.shutdown_requested() || transactions.shutdown_started() {
            return Ok(());
        }
        if !app.state::<crate::ConfigState>().0.lock().map_err(|_| "REMOTE_BUSY")?.full_mode() {
            state.suspend_under_operation();
            return Ok(());
        }
        let beta_enabled = configured(app).is_ok();
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        if !path().try_exists().map_err(|_| "REMOTE_PREFERENCES")? {
            return Ok(());
        }
        crate::persistence::prepare_private_file_for_load(&path(), "remote-control.json")
            .map_err(|_| "REMOTE_PREFERENCES")?;
        let mut bytes = Vec::new();
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path())
            .map_err(|_| "REMOTE_PREFERENCES")?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| "REMOTE_PREFERENCES")?;
        if bytes.len() > 4096 {
            return Err("REMOTE_PREFERENCES".into());
        }
        let mut prefs = serde_json::from_slice::<remote::Preferences>(&bytes)
            .map_err(|_| "REMOTE_PREFERENCES")?;
        if !beta_enabled {
            prefs.enabled = false;
        }
        state.configure(prefs.clone(), Arc::new(Adapter(app.clone())))?;
        if !beta_enabled {
            crate::persistence::atomic_write_private(
                &path(),
                &serde_json::to_vec(&prefs).map_err(|_| "REMOTE_PREFERENCES")?,
                "remote-control.json",
            )
            .map_err(|_| "REMOTE_PREFERENCES")?;
        }
        Ok(())
    })();
    if result.is_err() {
        eprintln!("[Remote] restore=unavailable");
    }
}
#[tauri::command]
pub(crate) fn get_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    require_network_services_enabled()?;
    status(&app)
}
#[tauri::command]
pub(crate) async fn set_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
    enabled: bool,
    port: u16,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    require_network_services_enabled()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<remote::State>();
        let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
        if enabled {
            configured(&app)?;
        }
        let transactions = app.state::<ConfigTransactionState>();
        if transactions.shutdown_requested() || transactions.shutdown_started() {
            return Err("REMOTE_BUSY".into());
        }
        let previous = state.preferences()?;
        let next = remote::Preferences { enabled, port };
        let backend = Arc::new(Adapter(app.clone()));
        if let Err(error) = state.configure(next.clone(), backend.clone()) {
            let _ = state.configure(previous, backend);
            return Err(error);
        }
        if transactions.shutdown_requested() {
            let _ = state.configure(
                remote::Preferences {
                    enabled: false,
                    port,
                },
                backend,
            );
            return Err("REMOTE_BUSY".into());
        }
        if crate::persistence::atomic_write_private(
            &path(),
            &serde_json::to_vec(&next).map_err(|_| "REMOTE_PREFERENCES")?,
            "remote-control.json",
        )
        .is_err()
        {
            let _ = state.configure(previous, backend);
            return Err("REMOTE_PREFERENCES".into());
        }
        status(&app)
    })
    .await
    .map_err(|_| "REMOTE_UNAVAILABLE")?
}
#[tauri::command]
pub(crate) fn pair_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    require_network_services_enabled()?;
    let state = app.state::<remote::State>();
    let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
    configured(&app)?;
    let data = state.shared().ok_or("REMOTE_DISABLED")?;
    data.auth.lock().map_err(|_| "REMOTE_BUSY")?.begin()?;
    status(&app)
}
#[tauri::command]
pub(crate) fn revoke_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    require_network_services_enabled()?;
    let state = app.state::<remote::State>();
    let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
    if let Some(data) = state.shared() {
        data.revoke()?;
    }
    status(&app)
}
#[tauri::command]
pub(crate) fn remote_control_qr(
    window: tauri::WebviewWindow,
    app: AppHandle,
    url: String,
) -> Result<String, String> {
    require_main(&window)?;
    require_network_services_enabled()?;
    configured(&app)?;
    let state = status(&app)?;
    if !state.enabled || !state.urls.contains(&url) {
        return Err("REMOTE_CHANGED".into());
    }
    let code = state.code.ok_or("REMOTE_PAIRING_CLOSED")?;
    let qr = qrcode::QrCode::new(format!("{url}/#code={code}").as_bytes())
        .map_err(|_| "REMOTE_INVALID")?;
    Ok(qr
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status() -> PlaybackStatus {
        serde_json::from_value(serde_json::json!({"snapshot_consistent":true,"state":"play","elapsed":12.5,"duration":180.,"audio_format":"48000:24:2","current_media":{"kind":"plex","server_id":"private-server","part_key":"/library/parts/1/file.flac?X-Plex-Token=private","queue_index":2},"title":"Song","artist":"Artist","album":"Album","thumb":"http://private/cover?token=private","plex_image":null,"volume":{"value":34,"muted":false,"writable":true,"available":true,"backend":"pipewire"},"is_updating":false})).unwrap()
    }
    #[test]
    fn null_output_profile_refuses_remote_service_access_without_starting_a_listener() {
        assert_eq!(network_services_enabled(true).unwrap_err(), "NULL_OUTPUT_TEST_SERVICES_DISABLED");
        assert!(network_services_enabled(false).is_ok());
        let state = remote::State::isolated_profile();
        assert!(!state.enabled.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(state.status(Vec::new()).unwrap().port, remote::ISOLATED_PROFILE_DEFAULT_PORT);
    }
    #[test]
    fn remote_projection_contains_metadata_and_an_opaque_media_key_without_source_paths_or_tokens()
    {
        let mut value = status();
        let first = public(&value);
        let text = serde_json::to_string(&first).unwrap();
        assert!(!text.contains("private"));
        assert!(!text.contains("part_key"));
        assert!(!text.contains("thumb"));
        assert_eq!(first.index, Some(2));
        assert!(first.volume_writable);
        if let Some(CurrentMedia::Plex { queue_index, .. }) = &mut value.current_media {
            *queue_index = 3;
        }
        assert_ne!(public(&value).media, first.media);
        value.snapshot_consistent = false;
        assert!(!public(&value).available);
        value.volume.backend = VolumeBackend::DirectEquipment;
        assert!(!public(&value).volume_writable);
    }
    #[test]
    fn remote_artwork_accepts_only_bounded_raster_payloads_never_a_url_or_svg() {
        use base64::Engine;
        assert!(image("http://private/cover?token=private").is_none());
        let svg = format!(
            "data:image/svg+xml;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"<svg/>")
        );
        assert!(image(&svg).is_none());
        let png = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfixture")
        );
        assert_eq!(image(&png).unwrap().0, "image/png");
        assert!(image("data:image/png;base64,PGh0bWw+").is_none());
    }
    #[test]
    fn linux_queue_revision_remains_an_opaque_remote_q1_token_for_the_underlying_mpd_q2_snapshot() {
        let mpd_revision = format!("q2:{}", "a".repeat(64));
        let wire = queue_wire_revision(&mpd_revision);
        assert!(wire.starts_with("q1:"));
        assert_eq!(wire.len(), 67);
        assert!(!wire.contains(&mpd_revision));
        assert_eq!(wire, queue_wire_revision(&mpd_revision));
        assert_ne!(wire, queue_wire_revision(&format!("q2:{}", "b".repeat(64))));
    }
    #[test]
    fn remote_queue_artwork_keys_are_opaque_and_scoped_to_revision_and_position() {
        let track = audio::TrackMetadata {
            title: "Fixture".into(), artist: "Artist".into(), album: "Album".into(),
            thumb: None, plex_image: None,
            media_locator: Some(audio::MediaLocator::Local { uri: "sqlite-track://7/YQ".into() }),
            uri: "/private/music/a.flac".into(), duration: Some(10.),
        };
        let revision = format!("q1:{}", "a".repeat(64));
        let first = queue_artwork_key(&revision, 0, &track);
        assert_eq!(first.len(), 64);
        assert!(!first.contains("sqlite") && !first.contains("private"));
        assert_eq!(first, queue_artwork_key(&revision, 0, &track));
        assert_ne!(first, queue_artwork_key(&revision, 1, &track));
        assert_ne!(first, queue_artwork_key(&format!("q1:{}", "b".repeat(64)), 0, &track));
        assert!(queue_track_has_artwork(&track));
    }
    #[test]
    fn remote_queue_projection_keeps_twelve_rows_and_art_keys_for_same_album_with_partial_art_metadata() {
        let tracks = (0..12).map(|index| {
            let media_locator = match index {
                0..=5 => audio::MediaLocator::Local { uri: format!("sqlite-track://7/{}", index + 1) },
                6..=8 => audio::MediaLocator::Plex { server_id: "fixture-server".into(), part_key: format!("/fixture/{index}"), rating_key: None, file_path: None },
                _ => audio::MediaLocator::Navidrome { connection_id: "fixture-connection".into(), song_id: format!("song-{index}"), account_id: None },
            };
            audio::TrackMetadata {
                title: format!("Track {index}"), artist: "Fixture artist".into(), album: "Fixture album".into(),
                thumb: (index % 2 == 0).then(|| "invalid-or-omitted-fixture-thumb".into()),
                plex_image: (6..=8).contains(&index).then(|| PlexImageRef { server_id: "fixture-server".into(), path: format!("/library/metadata/{index}/thumb") }),
                media_locator: Some(media_locator), uri: format!("/private/audio/{index}.flac"), duration: Some(90.),
            }
        }).collect();
        let snapshot = audio::QueueEditSnapshot { tracks, revision: format!("q2:{}", "c".repeat(64)) };
        let mut index = QueueArtworkIndex::default();
        let page = project_queue_page(snapshot, 0, &mut index).unwrap();
        assert_eq!(page.total, 12);
        assert_eq!(page.tracks.len(), 12);
        assert_eq!(page.revision, queue_wire_revision(&format!("q2:{}", "c".repeat(64))));
        assert!(page.tracks.iter().all(|track| track.artwork.as_ref().is_some_and(|key| key.len() == 64)));
        let keys = page.tracks.iter().filter_map(|track| track.artwork.as_ref()).collect::<std::collections::HashSet<_>>();
        assert_eq!(keys.len(), 12, "duplicate queue items keep distinct positions");
        let payload = serde_json::to_string(&page).unwrap();
        assert!(!payload.contains("sqlite-track://") && !payload.contains("private/audio") && !payload.contains("fixture-server"));
        assert_eq!(index.entries.len(), 12);
    }
}
