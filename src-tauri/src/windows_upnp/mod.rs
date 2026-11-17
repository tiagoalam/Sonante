//! Windows UPnP adapter. Original renderer/protocol/UI, same serial playback
//! actor; the network service lifetime is distinct from an admitted media lease.
mod contract;
pub(crate) mod media;
pub(crate) mod network;
pub(crate) mod stopped;
use crate::{
    upnp::{self, Backend, Control, Preferences, Snapshot, Status},
    windows_mpd::{action::Action, library::Library, session::Controller},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use tauri::{AppHandle, Manager};
pub(crate) type Guard = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) enum Request {
    Prepare(crate::windows_mpd::library::Selection),
    Action { id: Option<String>, action: Action },
}
#[derive(Default)]
pub(crate) struct Lifecycle {
    stopped: AtomicBool,
    epoch: AtomicU64,
    active: AtomicBool,
}
fn allowed(app: &AppHandle) -> bool {
    !app.state::<Lifecycle>().stopped.load(Ordering::Acquire)
        && app.state::<Controller>().ready()
        && app
            .state::<Library>()
            .observed_network_scope()
            .is_some_and(|s| s.0)
}
fn guard(app: &AppHandle, epoch: u64) -> Guard {
    let app = app.clone();
    Arc::new(move || {
        let s = app.state::<Lifecycle>();
        s.active.load(Ordering::Acquire)
            && s.epoch.load(Ordering::Acquire) == epoch
            && allowed(&app)
    })
}
struct Adapter {
    app: AppHandle,
    current: Guard,
}
impl Backend for Adapter {
    fn snapshot(&self) -> Result<Snapshot, String> {
        if !(self.current)() {
            return Err("UPNP_SESSION_CHANGED".into());
        }
        let value = self
            .app
            .state::<Controller>()
            .status()
            .map_err(str::to_owned)?
            .upnp_snapshot();
        if !(self.current)() {
            return Err("UPNP_SESSION_CHANGED".into());
        }
        Ok(value)
    }
    fn apply(&self, control: Control) -> Result<(), String> {
        if !(self.current)() {
            return Err("UPNP_SESSION_CHANGED".into());
        }
        self.app
            .state::<Controller>()
            .upnp_control(control, self.current.clone())
            .map(|_| ())
            .map_err(str::to_owned)
    }
}
fn path() -> std::path::PathBuf {
    crate::persistence::sonante_config_dir().join("upnp-receiver.json")
}
fn load() -> Result<Option<Preferences>, String> {
    let Some(bytes) = crate::windows_remote::storage::read(&path(), 4096)? else {
        return Ok(None);
    };
    let p: Preferences = serde_json::from_slice(&bytes).map_err(|_| "UPNP_PREFERENCES_INVALID")?;
    if p.name.trim().is_empty()
        || p.name.chars().count() > 64
        || p.name.chars().any(char::is_control)
        || p.id.len() != 36
        || !p.id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err("UPNP_PREFERENCES_INVALID".into());
    }
    Ok(Some(p))
}
fn save(p: &Preferences) -> Result<(), String> {
    let _ = load()?;
    crate::windows_remote::storage::write(
        &path(),
        &serde_json::to_vec(p).map_err(|_| "UPNP_PREFERENCES_INVALID")?,
    )
    .map_err(|_| "UPNP_PREFERENCES_SAVE_FAILED".into())
}
// Caller owns State.operation. Epoch is revoked before waiting on a worker;
// commands already waiting on actor admission cannot revive a closed service.
fn configure(app: &AppHandle, prefs: Preferences) -> Result<Preferences, String> {
    let name = prefs.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err("UPNP_NAME_INVALID".into());
    }
    let life = app.state::<Lifecycle>();
    life.active.store(false, Ordering::Release);
    let epoch = life.epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    let state = app.state::<upnp::State>();
    state.shutdown();
    if life.stopped.load(Ordering::Acquire) {
        return Err("UPNP_SHUTTING_DOWN".into());
    }
    if prefs.enabled && !allowed(app) {
        return Err("UPNP_FULL_REQUIRED".into());
    }
    let backend = Arc::new(Adapter {
        app: app.clone(),
        current: guard(app, epoch),
    });
    let result = state.configure(prefs, backend);
    if result.as_ref().is_ok_and(|p| p.enabled) && allowed(app) {
        life.active.store(true, Ordering::Release);
    }
    if life.stopped.load(Ordering::Acquire) {
        life.active.store(false, Ordering::Release);
        state.shutdown();
        return Err("UPNP_SHUTTING_DOWN".into());
    }
    result
}
pub(crate) fn publish(
    app: &AppHandle,
    result: &Result<crate::windows_mpd::session::Snapshot, &'static str>,
) {
    if let (Some(state), Ok(value)) = (app.try_state::<upnp::State>(), result) {
        if let Ok(mut current) = state.snapshot.lock() {
            *current = value.upnp_snapshot();
        }
    }
}
pub(crate) fn preferences_changed(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || restore(&app));
}
fn restore(app: &AppHandle) {
    let result = (|| -> Result<(), String> {
        let state = app.state::<upnp::State>();
        let _serial = state.operation.lock().map_err(|_| "UPNP_BUSY")?;
        if !allowed(app) {
            let life = app.state::<Lifecycle>();
            life.active.store(false, Ordering::Release);
            life.epoch.fetch_add(1, Ordering::AcqRel);
            state.shutdown();
            return Ok(());
        }
        if let Some(prefs) = load()? {
            let status = state.status()?;
            let old = state.preferences()?;
            // Unrelated config saves must not tear down active controllers,
            // subscriptions or discovery while the requested policy is unchanged.
            if old.enabled == prefs.enabled
                && old.name == prefs.name
                && old.id == prefs.id
                && status.error.is_none()
                && status.enabled == prefs.enabled
            {
                return Ok(());
            }
            configure(app, prefs)?;
        }
        Ok(())
    })();
    if result.is_err() {
        eprintln!("Windows UPnP: restore_unavailable");
    }
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
pub(crate) fn begin_shutdown(app: &AppHandle) {
    let s = app.state::<Lifecycle>();
    s.stopped.store(true, Ordering::Release);
    s.active.store(false, Ordering::Release);
    s.epoch.fetch_add(1, Ordering::AcqRel);
}
pub(crate) fn shutdown(app: &AppHandle) {
    let state = app.state::<upnp::State>();
    let _serial = state
        .operation
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    state.shutdown();
}
fn main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("UPNP_UNAVAILABLE".into())
    }
}
#[tauri::command]
pub(crate) fn get_upnp_receiver(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Status, String> {
    main(&window)?;
    app.state::<upnp::State>().status()
}
#[tauri::command]
pub(crate) async fn set_upnp_receiver(
    window: tauri::WebviewWindow,
    app: AppHandle,
    enabled: bool,
    name: String,
) -> Result<Status, String> {
    main(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<upnp::State>();
        let _serial = state.operation.lock().map_err(|_| "UPNP_BUSY")?;
        if app.state::<Lifecycle>().stopped.load(Ordering::Acquire) {
            return Err("UPNP_SHUTTING_DOWN".into());
        }
        if enabled && !allowed(&app) {
            return Err("UPNP_FULL_REQUIRED".into());
        }
        if let Err(error) = load() {
            if !enabled {
                let life = app.state::<Lifecycle>();
                life.active.store(false, Ordering::Release);
                life.epoch.fetch_add(1, Ordering::AcqRel);
                state.shutdown();
            }
            return Err(error);
        }
        let previous = state.preferences()?;
        let next = match configure(
            &app,
            Preferences {
                enabled,
                name,
                id: String::new(),
            },
        ) {
            Ok(p) => p,
            Err(error) => {
                let _ = configure(&app, previous);
                return Err(error);
            }
        };
        if let Err(error) = save(&next) {
            let _ = configure(&app, previous);
            return Err(error);
        }
        state.status()
    })
    .await
    .map_err(|_| "UPNP_WORKER_UNAVAILABLE")?
}
