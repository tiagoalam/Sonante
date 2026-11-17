//! Windows application adapter for the original paired LAN remote.
mod contract;
pub(crate) mod model;
mod network;
mod work;
#[cfg(windows)]
mod private_file;
pub(crate) mod storage;
use crate::{
    remote::{self, Backend, Command, Guard, QueuePage},
    windows_mpd::{
        library::Library,
        session::{Controller, Snapshot},
    },
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Manager};
#[derive(Default)]
pub(crate) struct Lifecycle {
    stopped: AtomicBool,
}
fn allowed(app: &AppHandle) -> bool {
    !app.state::<Lifecycle>().stopped.load(Ordering::Acquire)
        && app.state::<Controller>().ready()
        && app.state::<Library>().observed_network_scope() == Some((true, true))
}
fn path() -> std::path::PathBuf {
    crate::persistence::sonante_config_dir().join("remote-control.json")
}
fn require_main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("REMOTE_UNAVAILABLE".into())
    }
}
fn status(app: &AppHandle) -> Result<remote::Status, String> {
    app.state::<remote::State>().status(network::addresses())
}
async fn blocking<T:Send+'static>(write:bool,run:impl FnOnce()->Result<T,String>+Send+'static)->Result<T,String>{
    static READ:std::sync::OnceLock<work::Lane>=std::sync::OnceLock::new();
    static WRITE:std::sync::OnceLock<work::Lane>=std::sync::OnceLock::new();
    let lane=if write{WRITE.get_or_init(||work::Lane::new(1))}else{READ.get_or_init(||work::Lane::new(4))};
    let physical=lane.reserve().ok_or("REMOTE_BUSY")?;
    tauri::async_runtime::spawn_blocking(move||{let _physical=physical;run()}).await.map_err(|_|"REMOTE_UNAVAILABLE")?
}
async fn read_status(app:AppHandle)->Result<remote::Status,String>{
    blocking(false,move||{
        if app.state::<Lifecycle>().stopped.load(Ordering::Acquire){return Err("REMOTE_BUSY".into());}
        status(&app)
    }).await
}
struct Adapter(AppHandle);
impl Backend for Adapter {
    fn active(&self) -> bool {
        allowed(&self.0)
    }
    fn credential_path(&self) -> Option<std::path::PathBuf> {
        Some(crate::persistence::sonante_config_dir().join("remote-clients.json"))
    }
    fn queue(&self, offset: usize) -> Result<QueuePage, String> {
        if !self.active() {
            return Err("REMOTE_UNAVAILABLE".into());
        }
        self.0
            .state::<Controller>()
            .status()
            .map_err(str::to_owned)?
            .remote_queue(offset)
    }
    fn control(&self, command: Command, current: Guard) -> Result<(), String> {
        if !self.active() || !current() {
            return Err("REMOTE_CHANGED".into());
        }
        self.0
            .state::<Controller>()
            .remote_control(command, current)
            .map(|_| ())
            .map_err(str::to_owned)
    }
    fn artwork(&self, wanted: &str, current: Guard) -> Result<Option<(String, Vec<u8>)>, String> {
        if !self.active() || !current() {
            return Err("REMOTE_CHANGED".into());
        }
        let controller = self.0.state::<Controller>();
        let before = controller.status().map_err(str::to_owned)?;
        let track = before.remote_track(wanted).ok_or("REMOTE_CHANGED")?;
        let image = if let Some(radio) = &track.radio {
            radio.thumb.as_deref().and_then(image)
        } else if let Some(reference) = &track.plex_image {
            let result = tauri::async_runtime::block_on(
                self.0
                    .state::<crate::windows_plex::Controller>()
                    .image(reference.clone()),
            )?;
            if result.len() > 2 * 1024 * 1024 {
                None
            } else {
                mime(&result).map(|m| (m.into(), result))
            }
        } else {
            match &track.remote_source {
                Some(crate::windows_mpd::queue_v3::Source::Navidrome { song_id, .. }) => {
                    let nav = self.0.state::<crate::windows_navidrome::Controller>();
                    let c = track.remote_connection_id.clone().ok_or("REMOTE_CHANGED")?;
                    let id = tauri::async_runtime::block_on(
                        nav.song_cover_id(c.clone(), song_id.clone()),
                    )?;
                    match id {
                        Some(id) => image(&tauri::async_runtime::block_on(nav.cover(c, id))?),
                        None => None,
                    }
                }
                None => match &track.catalog_album_id {
                    Some(id) => self
                        .0
                        .state::<Library>()
                        .cover(id)
                        .map_err(str::to_owned)?
                        .as_deref()
                        .and_then(image),
                    None => None,
                },
                _ => None,
            }
        };
        if !self.active()
            || !current()
            || controller
                .status()
                .map_err(str::to_owned)?
                .remote_track(wanted)
                .is_none()
        {
            return Err("REMOTE_CHANGED".into());
        }
        Ok(image)
    }
    fn queue_artwork(&self, _wanted: &str, _current: Guard) -> Result<Option<(String, Vec<u8>)>, String> {
        Ok(None)
    }
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

pub(crate) fn publish(app: &AppHandle, value: &Result<Snapshot, &'static str>) {
    if let Some(state) = app.try_state::<remote::State>() {
        if !state.enabled.load(Ordering::Acquire) {
            return;
        }
        if let Ok(value) = value {
            state.publish(value.remote_frame().snapshot);
        }
    }
}
// No joining worker under the actor's audio/config locks. Admission checks the
// current library mode/beta independently; closing ports happens off the actor.
pub(crate) fn preferences_changed(app: &AppHandle, disable_grants: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || restore_policy(&app, disable_grants));
}
fn load_preferences() -> Result<Option<remote::Preferences>, String> {
    let Some(bytes) = storage::read(&path(), 4096)? else {
        return Ok(None);
    };
    let prefs: remote::Preferences =
        serde_json::from_slice(&bytes).map_err(|_| "REMOTE_PREFERENCES")?;
    if prefs.port < 1024 {
        return Err("REMOTE_PREFERENCES".into());
    }
    Ok(Some(prefs))
}
fn save_preferences(prefs: &remote::Preferences) -> Result<(), String> {
    let _ = load_preferences()?;
    storage::write(
        &path(),
        &serde_json::to_vec(prefs).map_err(|_| "REMOTE_PREFERENCES")?,
    )
}
pub(crate) fn restore(app: &AppHandle) {
    restore_policy(app, false)
}
fn restore_policy(app: &AppHandle, disable_grants: bool) {
    let result = (|| -> Result<(), String> {
        let state = app.state::<remote::State>();
        let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
        if app.state::<Lifecycle>().stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        let scope = app
            .state::<Library>()
            .network_scope()
            .map_err(str::to_owned)?;
        if !scope.0 && !(disable_grants && !scope.1) {
            state.suspend_under_operation();
            return Ok(());
        }
        let mut prefs = match load_preferences()? {
            Some(prefs) => prefs,
            None if disable_grants && !scope.1 => state.preferences()?,
            None => return Ok(()),
        };
        if !scope.1 {
            prefs.enabled = false;
        }
        state.configure(prefs.clone(), Arc::new(Adapter(app.clone())))?;
        if !scope.1 {
            save_preferences(&prefs)?;
        }
        Ok(())
    })();
    if result.is_err() {
        eprintln!("Windows remote: restore_unavailable");
    }
}
pub(crate) fn begin_shutdown(app: &AppHandle) {
    app.state::<Lifecycle>()
        .stopped
        .store(true, Ordering::Release);
}
pub(crate) fn shutdown(app: &AppHandle) {
    app.state::<remote::State>().shutdown();
}
#[tauri::command]
pub(crate) async fn get_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    read_status(app).await
}
#[tauri::command]
pub(crate) async fn set_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
    enabled: bool,
    port: u16,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    let worker=app.clone();
    blocking(true,move || {
        let app=worker;
        let state = app.state::<remote::State>();
        let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
        if app.state::<Lifecycle>().stopped.load(Ordering::Acquire) {
            return Err("REMOTE_BUSY".into());
        }
        if enabled && !allowed(&app) {
            return Err("REMOTE_UNAVAILABLE".into());
        }
        if let Err(error) = load_preferences() {
            // Explicit OFF still closes the listener if its durable preference
            // record was damaged. Never rewrite that damaged record or claim a
            // successful durable save.
            if !enabled {
                let _ = state.configure(
                    remote::Preferences {
                        enabled: false,
                        port,
                    },
                    Arc::new(Adapter(app.clone())),
                );
                state.suspend_under_operation();
            }
            return Err(error);
        }
        let previous = state.preferences()?;
        let next = remote::Preferences { enabled, port };
        let backend = Arc::new(Adapter(app.clone()));
        if let Err(error) = state.configure(next.clone(), backend.clone()) {
            let _ = state.configure(previous, backend);
            return Err(error);
        }
        if app.state::<Lifecycle>().stopped.load(Ordering::Acquire) {
            state.suspend_under_operation();
            return Err("REMOTE_BUSY".into());
        }
        if save_preferences(&next).is_err() {
            let _ = state.configure(previous, backend);
            return Err("REMOTE_PREFERENCES".into());
        }
        Ok(())
    })
    .await?;
    read_status(app).await
}
#[tauri::command]
pub(crate) async fn pair_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    let worker=app.clone();
    blocking(true,move||{
    let app=worker;
    let state = app.state::<remote::State>();
    let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
    if !allowed(&app) {
        return Err("REMOTE_UNAVAILABLE".into());
    }
    state
        .shared()
        .ok_or("REMOTE_DISABLED")?
        .auth
        .lock()
        .map_err(|_| "REMOTE_BUSY")?
        .begin()?;
    Ok(())
    }).await?;
    read_status(app).await
}
#[tauri::command]
pub(crate) async fn revoke_remote_control(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<remote::Status, String> {
    require_main(&window)?;
    let worker=app.clone();
    blocking(true,move||{
    let app=worker;
    if app.state::<Lifecycle>().stopped.load(Ordering::Acquire){return Err("REMOTE_BUSY".into());}
    let state = app.state::<remote::State>();
    let _serial = state.operation.lock().map_err(|_| "REMOTE_BUSY")?;
    if let Some(shared) = state.shared() {
        shared.revoke()?;
    }
    Ok(())
    }).await?;
    read_status(app).await
}
#[tauri::command]
pub(crate) async fn remote_control_qr(
    window: tauri::WebviewWindow,
    app: AppHandle,
    url: String,
) -> Result<String, String> {
    require_main(&window)?;
    blocking(false,move||{
    if !allowed(&app) {
        return Err("REMOTE_UNAVAILABLE".into());
    }
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
    }).await
}

pub(crate) fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Initialization can legitimately outlast a fixed three-second timer.
        // Wait without occupying a blocking worker or starting an audio engine.
        while !app.state::<Controller>().ready() {
            if app.state::<Lifecycle>().stopped.load(Ordering::Acquire) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if app.state::<Lifecycle>().stopped.load(Ordering::Acquire) {
            return;
        }
        let _ = tauri::async_runtime::spawn_blocking(move || restore(&app)).await;
    });
}

#[cfg(test)]
mod tests;
