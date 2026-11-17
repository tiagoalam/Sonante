//! Thin Windows IPC for the original A/B component, service and DSP core.
use super::*;
use crate::windows_editions::policy::Request;
use crate::windows_shared::{ab_comparison as core, ab_lifecycle as life};
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};
fn main(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("AB_WINDOW".into())
    }
}
async fn context(controller: Controller, library: Library) -> Result<core::Check, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::windows_instance::validate().map_err(|_| "AB_CHANGED")?;
        let (revision, scope) = controller.ab_context().map_err(str::to_owned)?;
        let check: core::Check = Arc::new(move || {
            controller.ab_revision_current(revision) && library.ab_scope_current(scope, None)
        });
        if !check() {
            return Err("AB_CHANGED".into());
        }
        Ok(check)
    })
    .await
    .map_err(|_| "AB_UNAVAILABLE")?
}
fn enabled(library: &Library, source: &Request) -> Result<(), String> {
    let scope = library.edition_scope().map_err(|_| "AB_CHANGED")?;
    if match source {
        Request::Local { .. } => scope.1.local,
        Request::Plex { .. } => scope.1.plex,
        _ => false,
    } {
        Ok(())
    } else {
        Err("AB_SOURCE".into())
    }
}
fn metadata_slots() -> &'static Arc<tokio::sync::Semaphore> {
    static S: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    S.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
}
#[tauri::command]
pub(crate) async fn ab_tracks(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    library: tauri::State<'_, Library>,
    plex: tauri::State<'_, crate::windows_plex::Controller>,
    source: Request,
) -> Result<Vec<core::Track>, String> {
    main(&window)?;
    enabled(&library, &source)?;
    let slot = metadata_slots()
        .clone()
        .try_acquire_owned()
        .map_err(|_| "AB_BUSY")?;
    let check = context(controller.inner().clone(), library.inner().clone()).await?;
    let lib = library.inner().clone();
    let plex = plex.inner().clone();
    let runtime = tokio::runtime::Handle::current();
    let worker = tauri::async_runtime::spawn_blocking(move || {
        let _slot = slot;
        let result = match source {
            Request::Local { album_id } => lib
                .ab_inventory(&album_id)
                .map(|inv| inv.rows.into_iter().map(|(t, _)| t).collect()),
            Request::Plex {
                server_id,
                library_id,
                album_id,
            } => {
                runtime.block_on(plex.ab_tracks(&server_id, &library_id, &album_id, check.clone()))
            }
            _ => Err("AB_SOURCE".into()),
        };
        if !check() {
            return Err("AB_CHANGED".into());
        }
        result
    });
    tokio::time::timeout(Duration::from_secs(65), worker)
        .await
        .map_err(|_| "AB_TIMEOUT")?
        .map_err(|_| "AB_UNAVAILABLE")?
}
#[tauri::command]
pub(crate) async fn ab_begin(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    library: tauri::State<'_, Library>,
) -> Result<String, String> {
    main(&window)?;
    let policy = context(controller.inner().clone(), library.inner().clone()).await?;
    let token = life::begin(policy)?;
    if let Ok(mut active) = active().lock() {
        active.take();
    }
    Ok(token)
}
#[tauri::command]
pub(crate) async fn ab_load(
    window: tauri::WebviewWindow,
    library: tauri::State<'_, Library>,
    plex: tauri::State<'_, crate::windows_plex::Controller>,
    token: String,
    side: core::Side,
    source: Request,
    track_key: String,
) -> Result<(), String> {
    main(&window)?;
    enabled(&library, &source)?;
    let job = core::job(&token)?;
    let lib = library.inner().clone();
    let plex = plex.inner().clone();
    let runtime = tokio::runtime::Handle::current();
    let local = matches!(source, Request::Local { .. });
    life::run(
        token,
        Duration::from_secs(if local { 6 } else { 65 }),
        move |check| -> Result<(), String> {
            if !check() {
                return Err("AB_CHANGED".into());
            }
            crate::windows_instance::validate().map_err(|_| "AB_CHANGED")?;
            let input = match source {
                Request::Local { album_id } => {
                    lib.ab_local_input(&album_id, &track_key, check.clone())?
                }
                Request::Plex {
                    server_id,
                    library_id,
                    album_id,
                } => runtime.block_on(plex.ab_input(
                    &server_id,
                    &library_id,
                    &album_id,
                    &track_key,
                    &lib.ab_storage_parent()?,
                    check.clone(),
                ))?,
                _ => return Err("AB_SOURCE".into()),
            };
            if !check() {
                return Err("AB_CHANGED".into());
            }
            job.install(side, input)?;
            if !check() {
                return Err("AB_CHANGED".into());
            }
            Ok(())
        },
    )
    .await
}
#[tauri::command]
pub(crate) async fn ab_decode(
    window: tauri::WebviewWindow,
    token: String,
    side: core::Side,
    start: f64,
    seconds: u32,
    sync_context: Option<bool>,
) -> Result<tauri::ipc::Response, core::DecodeFailure> {
    main(&window)?;
    core::parameters(start, seconds)?;
    let job = core::job(&token)?;
    let bytes = life::run(token, Duration::from_secs(40), move |check| {
        if !check() {
            return Err("AB_CHANGED".into());
        }
        crate::windows_instance::validate().map_err(|_| core::DecodeFailure::from("AB_CHANGED"))?;
        let bytes = core::decode_with_context(job, side, start, seconds, sync_context.unwrap_or(false))?;
        if !check() {
            return Err("AB_CHANGED".into());
        }
        Ok::<_, core::DecodeFailure>(bytes)
    })
    .await?;
    Ok(tauri::ipc::Response::new(bytes))
}
#[derive(Clone)]
struct Active {
    token: String,
    intent: String,
    watch: crate::audio::EqTransitionWatch,
    policy: core::Check,
}
fn active() -> &'static Mutex<Option<Active>> {
    static S: OnceLock<Mutex<Option<Active>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}
#[tauri::command]
pub(crate) async fn ab_start(
    window: tauri::WebviewWindow,
    controller: tauri::State<'_, Controller>,
    token: String,
    intent: String,
) -> Result<(), String> {
    main(&window)?;
    let policy = life::policy(&token)?;
    let job = core::job(&token)?;
    if !job.valid() {
        return Err("AB_CHANGED".into());
    }
    let controller = controller.inner().clone();
    let admitted = intent.clone();
    let check = policy.clone();
    let watch = tauri::async_runtime::spawn_blocking(move || {
        let (revision, _) = controller.ab_context().map_err(str::to_owned)?;
        if !check() {
            return Err("AB_CHANGED".into());
        }
        controller
            .pause_for_ab(&admitted, revision)
            .map_err(str::to_owned)
    })
    .await
    .map_err(|_| "AB_UNAVAILABLE")??;
    if !watch.current() || !job.valid() || !policy() {
        return Err("AB_CHANGED".into());
    }
    *active().lock().map_err(|_| "AB_UNAVAILABLE")? = Some(Active {
        token,
        intent,
        watch,
        policy,
    });
    Ok(())
}
#[tauri::command]
pub(crate) fn ab_alive(window: tauri::WebviewWindow, token: String, intent: String) -> bool {
    if main(&window).is_err() {
        return false;
    }
    let session = active().try_lock().ok().and_then(|s| {
        s.as_ref()
            .filter(|s| s.token == token && s.intent == intent)
            .cloned()
    });
    session.is_some_and(|s| {
        s.watch.current() && (s.policy)() && core::job(&token).is_ok_and(|j| j.valid())
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
    life::cancel(&token);
    if let Ok(mut active) = active().lock() {
        if active.as_ref().is_some_and(|s| s.token == token) {
            active.take();
        }
    }
}
pub(super) fn shutdown() {
    life::shutdown();
    if let Ok(mut active) = active().lock() {
        active.take();
    }
}
