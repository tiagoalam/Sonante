//! Application adapter: UPnP cannot bypass playback/configuration admission.
use crate::{
    audio::{CurrentMedia, MediaLocator, PlaybackStatus, TrackMetadata},
    upnp::{self, Backend, Control, Media, Preferences, Snapshot, Status},
    AudioState, ConfigState, ConfigTransactionState,
};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager};
static CURRENT: OnceLock<Mutex<Option<Media>>> = OnceLock::new();
fn media() -> &'static Mutex<Option<Media>> {
    CURRENT.get_or_init(|| Mutex::new(None))
}
pub fn resolve(id: &str) -> Result<String, String> {
    media()
        .lock()
        .map_err(|_| "UPNP_BUSY")?
        .as_ref()
        .filter(|m| m.id == id)
        .map(|m| m.uri.clone())
        .ok_or_else(|| "UPNP_SESSION_EXPIRED".into())
}
fn snapshot(status: &PlaybackStatus) -> Snapshot {
    Snapshot {
        session: match &status.current_media {
            Some(CurrentMedia::Upnp { session_id, .. }) if status.snapshot_consistent => {
                Some(session_id.clone())
            }
            _ => None,
        },
        state: status.state.clone(),
        elapsed: status.elapsed,
        duration: status.duration,
        volume: status.volume.value.min(100),
        muted: status.volume.muted,
        volume_writable: status.volume.available && status.volume.writable,
        volume_available: status.volume.available,
        error: status.playback_error.is_some() || !status.snapshot_consistent,
        play_mode: status
            .options
            .as_ref()
            .map(|options| {
                if options.random {
                    "SHUFFLE"
                } else if options.repeat {
                    if options.single != crate::playback_options::SingleMode::Off {
                        "REPEAT_ONE"
                    } else {
                        "REPEAT_ALL"
                    }
                } else {
                    "NORMAL"
                }
            })
            .unwrap_or("NORMAL")
            .into(),
    }
}
pub fn publish(app: &AppHandle, status: &PlaybackStatus, trusted_volume: bool) {
    if let Some(state) = app.try_state::<upnp::State>() {
        if let Ok(mut current) = state.snapshot.lock() {
            let mut next = snapshot(status);
            if !trusted_volume {
                next.volume = current.volume;
                next.muted = current.muted;
                next.volume_writable = current.volume_writable;
                next.volume_available = current.volume_available;
            }
            *current = next;
        }
    }
}
pub(crate) fn require_session(audio: &crate::audio::AudioEngine, id: &str) -> Result<(), String> {
    audio.ensure_queue_coherent()?;
    let queue = audio.get_queue();
    if queue.len() == 1
        && matches!(&queue[0].media_locator,Some(MediaLocator::Upnp{session_id}) if session_id==id)
    {
        Ok(())
    } else {
        Err("UPNP_SESSION_CHANGED".into())
    }
}

struct Adapter(AppHandle);
impl Adapter {
    fn unavailable(&self, error: &str) {
        if let Some(state) = self.0.try_state::<upnp::State>() {
            if let Ok(mut snapshot) = state.snapshot.lock() {
                snapshot.error = true;
                snapshot.volume_writable = false;
                snapshot.volume_available = false;
            }
        }
        eprintln!(
            "[UPnP] status_unavailable {}",
            crate::diagnostics::error_summary(error)
        );
    }
    fn status(&self) -> Result<PlaybackStatus, String> {
        let result =
            tauri::async_runtime::block_on(crate::get_playback_status(self.0.clone(), None));
        if let Err(error) = &result {
            self.unavailable(error);
        }
        result
    }
}
impl Adapter {
    fn control(&self, command: Control) -> Result<(), String> {
        let (expected, command) = match command {
            Control::Session { id, action } => (Some(id), *action),
            command => (None, command),
        };
        let check = |audio: &crate::audio::AudioEngine| match &expected {
            Some(id) => require_session(audio, id),
            None => Ok(()),
        };
        let app = &self.0;
        let transactions = app.state::<ConfigTransactionState>();
        if transactions.shutdown_requested() || transactions.shutdown_started() {
            return Err("UPNP_SHUTTING_DOWN".into());
        }
        transactions.operations.reject_playback()?;
        if expected.is_some() {
            check(
                &*app
                    .state::<AudioState>()
                    .0
                    .lock()
                    .map_err(|_| "UPNP_BUSY")?,
            )?;
        }
        match command {
            Control::Volume(value) => {
                crate::set_volume(value, app.state(), app.state(), app.state())?
            }
            Control::Mute(muted) => {
                let status = self.status()?;
                let token = status
                    .volume
                    .shared_token
                    .ok_or("UPNP_VOLUME_UNAVAILABLE")?;
                crate::set_shared_mute(muted, token, app.state(), app.state(), app.state())?;
            }
            command => {
                let shared = {
                    let config = app.state::<ConfigState>();
                    let config = config.0.lock().map_err(|_| "UPNP_BUSY")?;
                    crate::is_shared_output(&config)
                };
                let state = app.state::<AudioState>();
                state.note_transport_intervention();
                let token = crate::admit_or_validate_playback_checked(
                    &state.0,
                    &transactions,
                    None,
                    &check,
                )?;
                crate::finish_playback_intent(&state.0, &transactions, &token, |audio| {
                    check(audio)?;
                    transactions.operations.reject_playback()?;
                    match command {
                        Control::Prepare(ref item) => {
                            let track = TrackMetadata {
                                title: item.title.clone(),
                                artist: item.artist.clone(),
                                album: item.album.clone(),
                                thumb: item.artwork.clone(),
                                plex_image: None,
                                media_locator: Some(MediaLocator::Upnp {
                                    session_id: item.id.clone(),
                                }),
                                uri: String::new(),
                                duration: None,
                            };
                            audio.prepare_upnp(track, &item.uri)?;
                            *media().lock().map_err(|_| "UPNP_BUSY")? = Some(item.clone());
                            Ok(())
                        }
                        Control::Clear => {
                            if audio.get_queue().iter().any(|track| {
                                matches!(track.media_locator, Some(MediaLocator::Upnp { .. }))
                            }) {
                                audio.clear_queue()?;
                            }
                            *media().lock().map_err(|_| "UPNP_BUSY")? = None;
                            Ok(())
                        }
                        Control::Play => audio.play(shared),
                        Control::Pause => audio.pause(),
                        Control::Stop => audio.stop_upnp(),
                        Control::Seek(seconds) => audio.seek(seconds, shared),
                        _ => Err("UPNP_CONTROL_INVALID".into()),
                    }
                })?;
            }
        }
        // Accepted controls remain accepted if an informational read is late.
        if let Ok(status) = self.status() {
            publish(app, &status, true);
        }
        Ok(())
    }
}
impl Backend for Adapter {
    fn snapshot(&self) -> Result<Snapshot, String> {
        // Position/transport reads do not enumerate hardware or invoke wpctl.
        let state = self.0.state::<AudioState>();
        let result = state
            .0
            .lock()
            .map_err(|_| "UPNP_BUSY".to_string())
            .and_then(|audio| audio.get_mpris_status());
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                self.unavailable(&error);
                return Err(error);
            }
        };
        publish(&self.0, &status, false);
        let receiver = self.0.state::<upnp::State>();
        let current = receiver.snapshot.lock().map_err(|_| "UPNP_BUSY")?.clone();
        Ok(current)
    }
    fn volume_snapshot(&self) -> Result<Snapshot, String> {
        let status = self.status()?;
        publish(&self.0, &status, true);
        Ok(snapshot(&status))
    }
    fn apply(&self, command: Control) -> Result<(), String> {
        let result = self.control(command);
        if let Err(error) = &result {
            eprintln!(
                "[UPnP] control_refused {}",
                crate::diagnostics::error_summary(error)
            );
        }
        result
    }
}
fn path() -> std::path::PathBuf {
    crate::persistence::sonante_config_dir().join("upnp-receiver.json")
}
fn load() -> Result<Option<Preferences>, String> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let path = path();
    if !path
        .try_exists()
        .map_err(|_| "UPNP_PREFERENCES_UNAVAILABLE")?
    {
        return Ok(None);
    }
    crate::persistence::prepare_private_file_for_load(&path, "upnp-receiver.json")
        .map_err(|_| "UPNP_PREFERENCES_UNAVAILABLE")?;
    let mut bytes = Vec::new();
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "UPNP_PREFERENCES_UNAVAILABLE")?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "UPNP_PREFERENCES_UNAVAILABLE")?;
    if bytes.len() > 4096 {
        return Err("UPNP_PREFERENCES_INVALID".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "UPNP_PREFERENCES_INVALID".into())
}
pub fn restore(app: &AppHandle) {
    match load() {
        Ok(Some(prefs)) => {
            if let Err(error) = app
                .state::<upnp::State>()
                .configure(prefs, Arc::new(Adapter(app.clone())))
            {
                eprintln!("[UPnP] receiver_start=unavailable category={error}");
            }
        }
        Ok(None) => {}
        Err(error) => eprintln!("[UPnP] preferences=unavailable category={error}"),
    }
}
#[tauri::command]
pub fn get_upnp_receiver(app: AppHandle) -> Result<Status, String> {
    app.state::<upnp::State>().status()
}
#[tauri::command]
pub async fn set_upnp_receiver(
    app: AppHandle,
    enabled: bool,
    name: String,
) -> Result<Status, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let transactions = app.state::<ConfigTransactionState>();
        if transactions.shutdown_requested() || transactions.shutdown_started() {
            return Err("UPNP_SHUTTING_DOWN".into());
        }
        let state = app.state::<upnp::State>();
        let _serial = state.operation.lock().map_err(|_| "UPNP_BUSY")?;
        let previous = state.preferences()?;
        let prefs = state.configure(
            Preferences {
                enabled,
                name,
                id: String::new(),
            },
            Arc::new(Adapter(app.clone())),
        )?;
        if transactions.shutdown_requested() || transactions.shutdown_started() {
            state.shutdown();
            return Err("UPNP_SHUTTING_DOWN".into());
        }
        let bytes = serde_json::to_vec(&prefs).map_err(|_| "UPNP_PREFERENCES_INVALID")?;
        if crate::persistence::atomic_write_private(&path(), &bytes, "upnp-receiver.json").is_err()
        {
            if let Err(error) = state.configure(previous, Arc::new(Adapter(app.clone()))) {
                eprintln!("[UPnP] preferences_rollback=unavailable category={error}");
            }
            return Err("UPNP_PREFERENCES_SAVE_FAILED".into());
        }
        state.status()
    })
    .await
    .map_err(|_| "UPNP_WORKER_UNAVAILABLE")?
}
