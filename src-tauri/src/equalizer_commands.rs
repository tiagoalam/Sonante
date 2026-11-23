//! UI/persistence and lifecycle orchestration; a saved EQ request is restored
//! only through the existing confirmed Shared-route lifecycle.
use crate::equalizer::{EquipmentCategory, EqPreset, EqualizerConfig, EqualizerStore, PresetRef};
use crate::shared_dsp;
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Serialize)]
pub(crate) struct Catalog {
    built_ins: Vec<EqPreset>,
    builtin_groups: Vec<crate::equalizer::BuiltinGroup>,
    config: EqualizerConfig,
    revision: String,
    runtime: shared_dsp::Snapshot,
    shared: bool,
    runtime_available: bool,
    runtime_diagnostic: Option<&'static str>,
    pulse_audio: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyResult {
    runtime: shared_dsp::Snapshot,
    audio_route: Option<crate::audio_route::AudioRouteProjection>,
}

struct ConfirmedRouteTransition {
    generation: u64,
    requested: crate::audio_route::AudioRoute,
    legacy_issues: Vec<crate::audio_route::LegacyRouteIssue>,
}

fn camilla_runtime_diagnostic(error: shared_dsp::Error) -> &'static str {
    match error {
        shared_dsp::Error::RuntimeMissing => "EQ_CAMILLA_RUNTIME_MISSING",
        shared_dsp::Error::RuntimeUntrusted => "EQ_CAMILLA_RUNTIME_UNTRUSTED",
        _ => "EQ_CAMILLA_RUNTIME_UNAVAILABLE",
    }
}

fn mpd_eq_runtime_diagnostic(error: &str) -> &'static str {
    if error == "EQ_MPD_RUNTIME_MISSING" {
        "EQ_MPD_RUNTIME_MISSING"
    } else {
        "EQ_MPD_RUNTIME_UNAVAILABLE"
    }
}

fn runtime_diagnostic() -> Option<&'static str> {
    if let Err(error) = shared_dsp::binary_path() {
        return Some(camilla_runtime_diagnostic(error));
    }
    crate::mpd_engine::MpdEngine::resolve_eq()
        .err()
        .map(|error| mpd_eq_runtime_diagnostic(&error))
}

fn route_failure_details(
    stage: &'static str,
    error: &str,
    output_changed: bool,
) -> serde_json::Value {
    serde_json::json!({
        "stage": stage,
        "code": shared_dsp::diagnostics::safe_error_code(error),
        "output_changed": output_changed,
    })
}

fn revision(config: &EqualizerConfig) -> Result<String, String> {
    let bytes = serde_json::to_vec(config).map_err(|_| "EQ_STORE_FAILED")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn load() -> Result<(EqualizerConfig, String), String> {
    let config = EqualizerStore::default()
        .load()
        .map_err(|_| "EQ_STORE_UNAVAILABLE_PRESERVED")?;
    let version = revision(&config)?;
    Ok((config, version))
}
fn shared_config(app: &AppHandle) -> Result<AppConfig, String> {
    let config = app
        .state::<ConfigState>()
        .0
        .lock()
        .map_err(|_| "EQ_CONFIG_UNAVAILABLE")?
        .clone();
    if !crate::is_shared_output(&config) || config.first_run {
        return Err("EQ_SHARED_ONLY".into());
    }
    Ok(config)
}

fn startup_restore_allowed(
    desired: bool,
    shared_configured: bool,
    owned_mpd_running: bool,
    confirmed_shared_route: bool,
    queue_ready: bool,
    null_output_test: bool,
    eq_pipe_present: bool,
    runtime_idle: bool,
    shutdown: bool,
) -> bool {
    desired
        && shared_configured
        && owned_mpd_running
        && confirmed_shared_route
        && queue_ready
        && !null_output_test
        && !eq_pipe_present
        && runtime_idle
        && !shutdown
}

fn startup_restore_confirmed(snapshot: &shared_dsp::Snapshot) -> bool {
    snapshot.verified
        && snapshot.in_path == Some(true)
        && matches!(snapshot.phase, shared_dsp::Phase::Active | shared_dsp::Phase::Ready)
        && (snapshot.applied.is_some() || snapshot.comparison.is_some())
}

#[derive(Clone)]
enum PreferenceUpdate {
    SetEnabled(bool),
    RequireEnabledRevision(String),
}

fn update_preference(store: &EqualizerStore, update: &PreferenceUpdate) -> Result<(), String> {
    match update {
        PreferenceUpdate::SetEnabled(enabled) => persist_enabled_preference(store, *enabled),
        PreferenceUpdate::RequireEnabledRevision(expected) => {
            let (config, actual) = load_from_store(store)?;
            if config.enabled && &actual == expected {
                Ok(())
            } else {
                Err("EQ_STARTUP_RESTORE_SUPERSEDED".into())
            }
        }
    }
}

fn validate_preference_before_route(
    store: &EqualizerStore,
    update: &PreferenceUpdate,
) -> Result<(), String> {
    match update {
        PreferenceUpdate::SetEnabled(_) => Ok(()),
        PreferenceUpdate::RequireEnabledRevision(_) => update_preference(store, update),
    }
}

fn persist_preference_after_route(
    store: &EqualizerStore,
    update: &PreferenceUpdate,
) -> Result<(), String> {
    match update {
        PreferenceUpdate::SetEnabled(_) => update_preference(store, update),
        PreferenceUpdate::RequireEnabledRevision(_) => Ok(()),
    }
}

fn load_from_store(store: &EqualizerStore) -> Result<(EqualizerConfig, String), String> {
    let config = store
        .load()
        .map_err(|_| "EQ_STORE_UNAVAILABLE_PRESERVED")?;
    let revision = revision(&config)?;
    Ok((config, revision))
}

fn runtime_snapshot(app: &AppHandle) -> shared_dsp::Snapshot {
    let mut s = app.state::<shared_dsp::State>().snapshot();
    s.in_path = app
        .state::<SupervisorState>()
        .0
        .try_lock()
        .ok()
        .map(|v| v.shared_eq_pipe().is_some());
    if s.in_path == Some(true) && s.phase == shared_dsp::Phase::Off {
        s.phase = if s.error.is_some() {
            shared_dsp::Phase::Error
        } else {
            shared_dsp::Phase::Stopping
        };
    }
    s
}

#[tauri::command]
pub(crate) async fn get_equalizer(app: AppHandle) -> Result<Catalog, String> {
    let result = tauri::async_runtime::spawn_blocking(|| {
        load().map(|loaded| {
            // Availability is not activation. Hash resources and query server
            // compatibility without starting audio engines or holding locks.
            let diagnostic = runtime_diagnostic();
            let available = diagnostic.is_none();
            let pulse_audio = shared_dsp::availability::pulse_audio_in_use();
            (loaded, available, diagnostic, pulse_audio)
        })
    })
        .await
        .map_err(|_| "EQ_STORE_UNAVAILABLE")??;
    let ((config, revision), runtime_available, runtime_diagnostic, pulse_audio) = result;
    let shared = app
        .state::<ConfigState>()
        .0
        .lock()
        .map(|c| crate::is_shared_output(&c))
        .unwrap_or(false);
    Ok(Catalog {
        built_ins: crate::equalizer::catalog_built_ins(&config),
        builtin_groups: crate::equalizer::builtin_groups(&config),
        config,
        revision,
        runtime: runtime_snapshot(&app),
        shared,
        runtime_available,
        runtime_diagnostic,
        pulse_audio,
    })
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Edit {
    Import { document: String, name: String },
    ImportAutoEq { document: String, name: String },
    LinkEquipment { id: Option<String>, name: String, reference: PresetRef },
    AddEquipment { id: Option<String>, name: String, category: EquipmentCategory, manufacturer: Option<String>, opra_vendor_id: Option<String>, opra_product_id: Option<String> },
    AddEquipmentPreset { id: String, reference: PresetRef, preferred: bool },
    RemoveEquipmentPreset { id: String, reference: PresetRef },
    SetPreferredEquipmentPreset { id: String, reference: Option<PresetRef> },
    UnlinkEquipment { id: String },
    Create {
        name: String,
        source: Option<PresetRef>,
        preset: Option<EqPreset>,
    },
    Rename {
        id: String,
        name: String,
    },
    Update {
        id: String,
        preset: EqPreset,
    },
    Delete {
        reference: PresetRef,
    },
    Select {
        reference: PresetRef,
    },
}
#[tauri::command]
pub(crate) async fn edit_equalizer(
    expected_revision: String,
    edit: Edit,
    app: AppHandle,
) -> Result<Catalog, String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let transactions = app2.state::<ConfigTransactionState>();
        let _guard = transactions.begin()?;
        if matches!(&edit,Edit::Select{..}|Edit::Import{..}|Edit::Create{..}|Edit::Update{..}|Edit::Delete{..}) {
            app2.state::<AudioState>().0.lock().map_err(|_|"EQ_AUDIO_UNAVAILABLE")?.note_eq_intervention();
        }
        edit_store(&EqualizerStore::default(), &expected_revision, edit)
    })
    .await
    .map_err(|_| "EQ_STORE_WRITE_UNCONFIRMED")??;
    get_equalizer(app).await
}

#[derive(Serialize)]
pub(crate) struct ImportPreview { document: String, preset: EqPreset }
#[tauri::command]
pub(crate) async fn pick_equalizer_preset() -> Result<Option<ImportPreview>,String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(path)=rfd::FileDialog::new().add_filter("Sonante EQ JSON", &["json"]).pick_file() else {return Ok(None)};
        let document=crate::equalizer_portable::read_document(&path)?;
        let preset=crate::equalizer_portable::inspect(&document)?;
        Ok(Some(ImportPreview{document,preset}))
    }).await.map_err(|_|"EQ_IMPORT_INVALID")?
}

#[derive(Serialize)]
pub(crate) struct AutoEqImportPreview {
    document: String,
    preset: EqPreset,
    source_filter_count: usize,
    source_name: String,
}

#[tauri::command]
pub(crate) async fn pick_autoeq_preset() -> Result<Option<AutoEqImportPreview>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Parametric EQ text", &["txt"])
            .pick_file()
        else { return Ok(None) };
        let document = crate::equalizer_portable::read_document(&path)
            .map_err(|_| "EQ_AUTOEQ_IMPORT_INVALID")?;
        let imported = crate::equalizer_autoeq::parse(&document)?;
        // Return only a sanitized basename; never expose or persist the selected path.
        let source_name = path.file_name().and_then(|v| v.to_str()).unwrap_or("ParametricEQ.txt")
            .chars().filter(|c| !c.is_control()).take(120).collect::<String>();
        Ok(Some(AutoEqImportPreview {
            document,
            preset: imported.preset,
            source_filter_count: imported.source_filter_count,
            source_name,
        }))
    }).await.map_err(|_| "EQ_AUTOEQ_IMPORT_INVALID")?
}
#[tauri::command]
pub(crate) async fn export_equalizer_preset(preset: EqPreset) -> Result<bool,String> {
    tauri::async_runtime::spawn_blocking(move || {
        let document=crate::equalizer_portable::export(&preset)?;
        let Some(path)=rfd::FileDialog::new().add_filter("Sonante EQ JSON", &["json"])
            .set_file_name("preset.sonante-eq.json").save_file() else {return Ok(false)};
        crate::equalizer_portable::write_new_document(&path,&document)?;Ok(true)
    }).await.map_err(|_|"EQ_EXPORT_FAILED")?
}

fn edit_store(store: &EqualizerStore, expected_revision: &str, edit: Edit) -> Result<(), String> {
    let mut config = store.load().map_err(|_| "EQ_STORE_UNAVAILABLE_PRESERVED")?;
    let current = revision(&config)?;
    if current != expected_revision {
        return Err("EQ_STORE_CHANGED".into());
    }
    let result = match edit {
        Edit::Import { document, name } => {
            let mut value = crate::equalizer_portable::inspect(&document)?;
            let created=config.create_user_preset(&name,None).map_err(|_|"EQ_PRESET_INVALID")?;
            value.id=created.id.clone();value.name=created.name;
            config.update_user_preset(&created.id,value).map_err(|_|"EQ_PRESET_INVALID")?;
            config.set_active_preset(PresetRef::User{id:created.id})
        }
        Edit::ImportAutoEq { document, name } => {
            let mut value = crate::equalizer_autoeq::parse(&document)?.preset;
            let created = config.create_user_preset(&name, None).map_err(|_| "EQ_PRESET_INVALID")?;
            value.id = created.id.clone();
            value.name = created.name;
            // Saving an imported copy must not select it or alter activation state.
            config.update_user_preset(&created.id, value)
        }
        Edit::LinkEquipment { id, name, reference } => config.link_equipment(id,&name,reference),
        Edit::AddEquipment { id, name, category, manufacturer, opra_vendor_id, opra_product_id } => config.add_equipment(id, &name, category, manufacturer, opra_vendor_id, opra_product_id).map(|_| ()),
        Edit::AddEquipmentPreset { id, reference, preferred } => config.add_equipment_preset(&id, reference, preferred),
        Edit::RemoveEquipmentPreset { id, reference } => config.remove_equipment_preset(&id, &reference),
        Edit::SetPreferredEquipmentPreset { id, reference } => config.set_preferred_equipment_preset(&id, reference),
        Edit::UnlinkEquipment { id } => config.unlink_equipment(&id),
        Edit::Create {
            name,
            source,
            preset,
        } => {
            let created = config
                .create_user_preset(&name, source.as_ref())
                .map_err(|_| "EQ_PRESET_INVALID")?;
            if let Some(mut value) = preset {
                value.id = created.id.clone();
                value.name = created.name;
                config
                    .update_user_preset(&created.id, value)
                    .map_err(|_| "EQ_PRESET_INVALID")?;
            }
            config.set_active_preset(PresetRef::User { id: created.id })
        }
        Edit::Rename { id, name } => config.rename_user_preset(&id, &name),
        Edit::Update { id, preset } => config.update_user_preset(&id, preset),
        Edit::Delete { reference } => config.delete_preset(&reference),
        Edit::Select { reference } => config.set_active_preset(reference),
    };
    for p in &config.user_presets {
        crate::equalizer::validate_v1(p).map_err(|_| "EQ_PRESET_INVALID")?;
    }
    result.map_err(|_| "EQ_PRESET_INVALID")?;
    store
        .save(&config)
        .map_err(|_| "EQ_STORE_WRITE_UNCONFIRMED".to_string())
}

fn persist_enabled_preference(store: &EqualizerStore, enabled: bool) -> Result<(), String> {
    let mut config = store.load().map_err(|_| "EQ_STORE_UNAVAILABLE_PRESERVED")?;
    config.set_enabled(enabled).map_err(|_| "EQ_PRESET_INVALID")?;
    store
        .save(&config)
        .map_err(|_| "EQ_STORE_WRITE_UNCONFIRMED".to_string())
}

fn failure_callback(app: &AppHandle) -> Arc<dyn Fn() + Send + Sync> {
    let app = app.clone();
    Arc::new(move || {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let error = app
                .state::<shared_dsp::State>()
                .snapshot()
                .error
                .unwrap_or(shared_dsp::Error::ControlFailed);
            shared_dsp::diagnostics::event("dsp.failure_callback", "started", Some(0), serde_json::json!({"error": error}));
            let tx = app.state::<ConfigTransactionState>();
            loop {
                if tx.shutdown_requested() {
                    shared_dsp::diagnostics::event("dsp.failure_callback", "cancelled", None, serde_json::json!({"reason": "shutdown"}));
                    return;
                }
                match tx.operations.try_begin_background() {
                    Err(config_save_operation::BackgroundAdmission::Busy) => {
                        let _ = tauri::async_runtime::spawn_blocking(|| {
                            std::thread::sleep(Duration::from_secs(1))
                        })
                        .await;
                    }
                    Err(config_save_operation::BackgroundAdmission::Blocked) => {
                        shared_dsp::diagnostics::event("dsp.failure_callback", "blocked", None, serde_json::json!({"reason": "background_admission_blocked"}));
                        return;
                    }
                    Ok(mut guard) => {
                        if let Ok(config) = shared_config(&app) {
                            let requested = match crate::configured_audio_route(
                                &config,
                                crate::audio_route::AudioProcessing::Off,
                            ) {
                                Ok(route) => route,
                                Err(_) => {
                                    shared_dsp::diagnostics::event("dsp.failure_bypass", "error", None, serde_json::json!({"code": "route_invalid"}));
                                    app.state::<shared_dsp::State>().record_failure(error);
                                    return;
                                }
                            };
                            let generation = match guard.begin_audio_transition(
                                requested,
                                config_save_operation::AudioTransitionKind::Equalizer,
                            ) {
                                Ok(generation) => generation,
                                Err(_) => {
                                    shared_dsp::diagnostics::event("dsp.failure_bypass", "error", None, serde_json::json!({"code": "route_admission_failed"}));
                                    app.state::<shared_dsp::State>().record_failure(error);
                                    return;
                                }
                            };
                            let route = tx.operations.audio_transition_handle(generation);
                            let result = set_route_admitted(&app, None, false, route.clone()).await;
                            if result.is_ok() { route.confirm(); } else { route.fail(); }
                            if result.is_err() {
                                shared_dsp::diagnostics::event("dsp.failure_bypass", "error", None, serde_json::json!({"code": result.as_ref().err().map(|error| shared_dsp::diagnostics::safe_error_code(error))}));
                                eprintln!("[Equalizer] fallback=unconfirmed");
                            } else {
                                shared_dsp::diagnostics::event("dsp.failure_bypass", "ok", None, serde_json::json!({}));
                            }
                        } else {
                            shared_dsp::diagnostics::event("dsp.failure_bypass", "error", None, serde_json::json!({"code": "profile_config_unavailable"}));
                        }
                        app.state::<shared_dsp::State>().record_failure(error);
                        return;
                    }
                }
            }
        });
    })
}

#[tauri::command]
pub(crate) async fn apply_equalizer(
    enabled: bool,
    preset: Option<EqPreset>,
    bypass_expected: Option<EqPreset>,
    app: AppHandle,
) -> Result<ApplyResult, String> {
    let mut attempt = shared_dsp::diagnostics::Attempt::begin();
    let initial = app.state::<shared_dsp::State>().snapshot();
    let selected_mode = app
        .state::<ConfigState>()
        .0
        .lock()
        .map(|config| {
            if crate::is_shared_output(&config) {
                "shared"
            } else if config.audio_output_type == "alsa" {
                "direct"
            } else {
                "other"
            }
        })
        .unwrap_or("configuration_unavailable");
    attempt.event(
        "tauri.command",
        "started",
        Some(0),
        serde_json::json!({
            "enabled": enabled,
            "preset_present": preset.is_some(),
            "selected_output_mode": selected_mode,
            "initial_phase": initial.phase,
            "initial_dsp_verified": initial.verified,
            "initial_preset_present": initial.applied.is_some(),
            "initial_error": initial.error,
        }),
    );
    if shared_dsp::diagnostics::is_enabled() && enabled {
        let validation_started = std::time::Instant::now();
        let diagnostic = runtime_diagnostic();
        attempt.event(
            "runtime.validation",
            if diagnostic.is_none() { "ok" } else { "error" },
            Some(validation_started.elapsed().as_millis()),
            serde_json::json!({
                "code": diagnostic.map(shared_dsp::diagnostics::safe_error_code),
                "process_readiness_confirmed": false,
            }),
        );
    }
    let result = apply_equalizer_inner(
        enabled,
        preset,
        bypass_expected,
        PreferenceUpdate::SetEnabled(enabled),
        app,
    )
    .await;
    let code = result
        .as_ref()
        .err()
        .map(|error| shared_dsp::diagnostics::safe_error_code(error))
        .unwrap_or("confirmed");
    attempt.finish(if result.is_ok() { "ok" } else { "error" }, code);
    result
}

async fn apply_equalizer_inner(
    enabled: bool,
    preset: Option<EqPreset>,
    bypass_expected: Option<EqPreset>,
    preference: PreferenceUpdate,
    app: AppHandle,
) -> Result<ApplyResult, String> {
    if enabled {
        // Recheck the live server; a catalog observation is not authorization
        // to activate after the system audio server changes. Bypass stays usable.
        let availability_started = std::time::Instant::now();
        let availability = tauri::async_runtime::spawn_blocking(shared_dsp::availability::require_supported)
            .await;
        match availability {
            Ok(Ok(())) => shared_dsp::diagnostics::event("audio.server_admission", "ok", Some(availability_started.elapsed().as_millis()), serde_json::json!({"supported_server": true})),
            Ok(Err(error)) => {
                shared_dsp::diagnostics::event("audio.server_admission", "error", Some(availability_started.elapsed().as_millis()), serde_json::json!({"code": shared_dsp::diagnostics::safe_error_code(&error)}));
                return Err(error);
            }
            Err(_) => {
                shared_dsp::diagnostics::event("audio.server_admission", "error", Some(availability_started.elapsed().as_millis()), serde_json::json!({"code": "availability_worker_failed"}));
                return Err("EQ_AVAILABILITY_UNCONFIRMED".into());
            }
        }
    }
    app.state::<AudioState>().0.lock().map_err(|_|"EQ_AUDIO_UNAVAILABLE")?.note_eq_intervention();
    if enabled {
        let preflight_started = std::time::Instant::now();
        shared_dsp::diagnostics::event("activation.preflight", "started", None, serde_json::json!({"stage": "shared_route_and_preset"}));
        if let Err(error) = shared_config(&app) {
            shared_dsp::diagnostics::event("activation.preflight", "error", Some(preflight_started.elapsed().as_millis()), serde_json::json!({"stage": "shared_route_and_preset", "code": shared_dsp::diagnostics::safe_error_code(&error)}));
            return Err(error);
        }
        let preset = preset.as_ref().ok_or("EQ_PRESET_REQUIRED")?;
        if crate::equalizer::validate_v1(&preset).is_err() {
            shared_dsp::diagnostics::event("activation.preflight", "error", Some(preflight_started.elapsed().as_millis()), serde_json::json!({"stage": "preset_validation", "code": "preset_invalid"}));
            return Err("EQ_PRESET_INVALID".into());
        }
        if shared_dsp::safety::assess(&preset, 48000).is_err() {
            shared_dsp::diagnostics::event("activation.preflight", "error", Some(preflight_started.elapsed().as_millis()), serde_json::json!({"stage": "headroom_validation", "code": "headroom_rejected"}));
            return Err("EQ_UNSAFE_HEADROOM".into());
        }
        shared_dsp::diagnostics::event("activation.preflight", "ok", Some(preflight_started.elapsed().as_millis()), serde_json::json!({"stage": "shared_route_and_preset"}));
    } else {
        let current = app.state::<shared_dsp::State>().snapshot();
        confirm_level_return(&current, bypass_expected.as_ref())?;
    }

    let confirmed_transition = if enabled {
        let preset = preset.ok_or("EQ_PRESET_REQUIRED")?;
        let state=app.state::<shared_dsp::State>().snapshot();
        if (state.applied.is_some()||state.comparison.is_some())
            && app
                .state::<SupervisorState>()
                .0
                .lock()
                .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
                .shared_eq_pipe()
                .is_some()
        {
            let worker = app.clone();
            let preference = preference.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let tx = worker.state::<ConfigTransactionState>();
                let _guard = tx.begin()?;
                update_preference(&EqualizerStore::default(), &preference)?;
                shared_config(&worker)?;
                worker
                    .state::<shared_dsp::State>()
                    .apply(preset)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| "EQ_APPLY_FAILED")??;
            None
        } else {
            Some(set_route(&app, Some(preset), None, preference).await?)
        }
    } else {
        Some(set_route(&app, None, bypass_expected, preference).await?)
    };
    let runtime = runtime_snapshot(&app);
    let audio_route = confirmed_transition
        .and_then(|transition| confirmed_route_projection(&app, transition, &runtime));
    Ok(ApplyResult { runtime, audio_route })
}

/// One-shot startup restoration, invoked only after the already-owned MPD and
/// its cached queue have completed startup restoration. It never starts a
/// playback request or creates another supervisor.
pub(crate) async fn restore_desired_on_startup(app: AppHandle) {
    let loaded = tauri::async_runtime::spawn_blocking(load).await;
    let (config, saved_revision) = match loaded {
        Ok(Ok(value)) => value,
        Ok(Err(code)) => {
            shared_dsp::diagnostics::event("eq.startup_restore", "unavailable", None, serde_json::json!({"code": code}));
            return;
        }
        Err(_) => {
            shared_dsp::diagnostics::event("eq.startup_restore", "unavailable", None, serde_json::json!({"code": "EQ_STORE_UNAVAILABLE"}));
            return;
        }
    };
    if !config.enabled {
        shared_dsp::diagnostics::event("eq.startup_restore", "disabled", None, serde_json::json!({}));
        return;
    }

    let shared_configured = app.state::<ConfigState>().0.lock()
        .map(|value| !value.first_run && crate::is_shared_output(&value))
        .unwrap_or(false);
    if !shared_configured {
        // Direct is a supported user choice. Keep the desired preference for a
        // later Shared session, but do not touch its device or claim EQ active.
        shared_dsp::diagnostics::event("eq.startup_restore", "deferred", None, serde_json::json!({"reason": "shared_route_required"}));
        return;
    }

    let (owned_mpd_running, confirmed_shared_route, null_output_test, eq_pipe_present) = {
        let supervisor_state = app.state::<SupervisorState>();
        let mut supervisor = match supervisor_state.0.lock() {
            Ok(value) => value,
            Err(_) => {
                app.state::<shared_dsp::State>().record_failure(shared_dsp::Error::ControlFailed);
                return;
            }
        };
        let running = matches!(supervisor.observe_health(), Ok(MpdProcessObservation::Running))
            && supervisor.observed_process_pid().is_some();
        let route = supervisor.audio_route_observation();
        let shared = route.applied.as_ref().is_some_and(|route| {
            route.access == crate::audio_route::AudioAccess::Shared
                && route.processing == crate::audio_route::AudioProcessing::Off
        });
        (running, shared, route.null_output_test, route.eq_pipe_configured)
    };
    let queue_ready = app.state::<AudioState>().0.lock()
        .map(|audio| audio.ensure_startup_queue_ready().is_ok())
        .unwrap_or(false);
    let current = runtime_snapshot(&app);
    let runtime_active = startup_restore_confirmed(&current);
    let runtime_idle = current.phase == shared_dsp::Phase::Off
        && current.applied.is_none()
        && current.comparison.is_none();
    let shutdown = app.state::<ConfigTransactionState>().shutdown_requested();
    if !startup_restore_allowed(
        config.enabled,
        shared_configured,
        owned_mpd_running,
        confirmed_shared_route,
        queue_ready,
        null_output_test,
        eq_pipe_present || runtime_active,
        runtime_idle,
        shutdown,
    ) {
        shared_dsp::diagnostics::event(
            "eq.startup_restore",
            "deferred",
            None,
            serde_json::json!({
                "owned_mpd_running": owned_mpd_running,
                "confirmed_shared_route": confirmed_shared_route,
                "queue_ready": queue_ready,
                "null_output_test": null_output_test,
                "existing_eq_pipe": eq_pipe_present,
                "runtime_idle": runtime_idle,
                "shutdown": shutdown,
            }),
        );
        if !shutdown && (!owned_mpd_running || !confirmed_shared_route || !queue_ready || eq_pipe_present) {
            app.state::<shared_dsp::State>().record_failure(shared_dsp::Error::ControlFailed);
        }
        return;
    }

    let preset = match config.preset(&config.active_preset) {
        Ok(value) => value,
        Err(_) => {
            app.state::<shared_dsp::State>().record_failure(shared_dsp::Error::ApplyFailed);
            return;
        }
    };
    if crate::equalizer::validate_v1(&preset).is_err()
        || shared_dsp::safety::assess(&preset, 48000).is_err()
    {
        app.state::<shared_dsp::State>().record_failure(shared_dsp::Error::ApplyFailed);
        return;
    }
    if !app.state::<shared_dsp::State>().begin_restore() {
        return;
    }

    shared_dsp::diagnostics::event("eq.startup_restore", "started", None, serde_json::json!({"preset_present": true}));
    match apply_equalizer_inner(
        true,
        Some(preset),
        None,
        PreferenceUpdate::RequireEnabledRevision(saved_revision),
        app.clone(),
    )
    .await
    {
        Ok(result) if startup_restore_confirmed(&result.runtime) => {
            shared_dsp::diagnostics::event("eq.startup_restore", "confirmed", None, serde_json::json!({"revision": result.runtime.revision}));
        }
        Ok(_) => {
            shared_dsp::diagnostics::event("eq.startup_restore", "unconfirmed", None, serde_json::json!({"code": "EQ_RUNTIME_UNCONFIRMED"}));
            app.state::<shared_dsp::State>().record_failure(shared_dsp::Error::ControlFailed);
        }
        Err(error) if error == "EQ_STARTUP_RESTORE_SUPERSEDED" => {
            shared_dsp::diagnostics::event("eq.startup_restore", "cancelled", None, serde_json::json!({"reason": "preference_changed"}));
            app.state::<shared_dsp::State>().cancel_restore();
        }
        Err(error) => {
            shared_dsp::diagnostics::event("eq.startup_restore", "error", None, serde_json::json!({"code": shared_dsp::diagnostics::safe_error_code(&error)}));
            app.state::<shared_dsp::State>().record_failure(shared_dsp::Error::ControlFailed);
        }
    }
}

/// Live DSP configuration only; never calls a queue, playback or output-switch method.
#[tauri::command]
pub(crate) async fn compare_equalizer(action:shared_dsp::comparison::Action,token:Option<String>,expected:Option<EqPreset>,app:AppHandle)->Result<shared_dsp::Snapshot,String>{
    let worker=app.clone();
    tauri::async_runtime::spawn_blocking(move||{
        // Finish is a token/watch/generation-guarded cleanup in the DSP actor. It
        // can queue behind an apply; it never defeats a newer intent or lifecycle.
        if action==shared_dsp::comparison::Action::Finish {
            return worker.state::<shared_dsp::State>().compare(action,token,expected,None).map_err(|e|e.to_string());
        }
        shared_dsp::availability::require_supported()?;
        let tx=worker.state::<ConfigTransactionState>();let _guard=tx.begin()?;
        shared_config(&worker)?;
        if worker.state::<SupervisorState>().0.lock().map_err(|_|"EQ_SUPERVISOR_UNAVAILABLE")?.shared_eq_pipe().is_none(){return Err("EQ_COMPARISON_UNAVAILABLE".into())}
        let watch=worker.state::<AudioState>().0.lock().map_err(|_|"EQ_AUDIO_UNAVAILABLE")?.watch_eq_transition();
        worker.state::<shared_dsp::State>().compare(action,token,expected,Some(watch)).map_err(|e|e.to_string())
    }).await.map_err(|_|"EQ_COMPARISON_UNCONFIRMED")??;
    Ok(runtime_snapshot(&app))
}

fn confirm_level_return(
    current: &shared_dsp::Snapshot,
    expected: Option<&EqPreset>,
) -> Result<(), String> {
    // Shared protection always introduces at least the resampler reserve.
    // Removing it must be an explicit acknowledgement bound to THIS profile,
    // not a stale/global boolean. Automatic error fallback stays paused.
    if current
        .applied
        .as_ref()
        .or_else(||current.comparison.as_ref().map(|c|&c.source))
        .is_some_and(|p| Some(p) != expected)
    {
        return Err("EQ_BYPASS_LEVEL_RETURN_CONFIRMATION_REQUIRED".into());
    }
    Ok(())
}

async fn set_route(
    app: &AppHandle,
    preset: Option<EqPreset>,
    bypass_expected: Option<EqPreset>,
    preference: PreferenceUpdate,
) -> Result<ConfirmedRouteTransition, String> {
    let transactions = app.state::<ConfigTransactionState>();
    let current = app.state::<shared_dsp::State>().snapshot();
    if preset.is_none() {
        confirm_level_return(&current, bypass_expected.as_ref())?;
    }
    let config = shared_config(app)?;
    let processing = if preset.is_some() {
        crate::audio_route::AudioProcessing::Equalizer
    } else {
        crate::audio_route::AudioProcessing::Off
    };
    let requested_mode = if preset.is_some() { "shared_eq" } else { "shared" };
    let requested = crate::configured_audio_route(&config, processing)?;
    let confirmed_route = requested.clone();
    let legacy_issues = crate::audio_route::interpret_legacy_route(
        &config.audio_output_type,
        &config.alsa_device,
    ).issues;
    // Serialize the preference update/check with the route operation. Startup
    // restoration uses a revision check here so a late restore cannot override
    // a newer explicit disable.
    let store = EqualizerStore::default();
    shared_dsp::diagnostics::event("route.transition_admission", "started", None, serde_json::json!({"requested_mode": requested_mode}));
    let transition = transactions.begin_audio_transition(
        requested,
        crate::config_save_operation::AudioTransitionKind::Equalizer,
        || validate_preference_before_route(&store, &preference),
    );
    let (_route_transaction, route) = match transition {
        Ok(value) => {
            shared_dsp::diagnostics::event("route.transition_admission", "ok", None, serde_json::json!({"requested_mode": requested_mode}));
            value
        }
        Err(error) => {
            shared_dsp::diagnostics::event("route.transition_admission", "error", None, serde_json::json!({"requested_mode": requested_mode, "code": shared_dsp::diagnostics::safe_error_code(&error)}));
            return Err(error);
        }
    };
    let generation = route.generation();
    // A failed DSP cleanup is not a healthy transition eligible for autoplay.
    let may_resume = preset.is_some() || (current.verified && (current.applied.is_some()||current.comparison.is_some()));
    let result = set_route_admitted(app, preset, may_resume, route.clone()).await;
    if let Err(error) = result {
        // Preflight failures are known failures. Once output mutation begins,
        // switch_output marks the operation uncertain and resolves it only
        // after a verified rollback or a successful transition.
        route.fail();
        shared_dsp::diagnostics::event("route.confirmation", "failed_or_uncertain", None, serde_json::json!({"requested_mode": requested_mode}));
        return Err(error);
    }
    route.confirm();
    shared_dsp::diagnostics::event("route.confirmation", "confirmed", None, serde_json::json!({"requested_mode": requested_mode}));
    match persist_preference_after_route(&store, &preference) {
        Ok(()) => {
            let enabled = match &preference {
                PreferenceUpdate::SetEnabled(value) => Some(*value),
                _ => None,
            };
            shared_dsp::diagnostics::event(
                "preference.persist",
                "confirmed",
                None,
                serde_json::json!({"enabled": enabled}),
            );
            Ok(ConfirmedRouteTransition {
                generation,
                requested: confirmed_route,
                legacy_issues,
            })
        }
        Err(error) => {
            shared_dsp::diagnostics::event(
                "preference.persist",
                "error",
                None,
                serde_json::json!({"code": shared_dsp::diagnostics::safe_error_code(&error)}),
            );
            Err(error)
        }
    }
}

fn confirmed_route_projection(
    app: &AppHandle,
    transition: ConfirmedRouteTransition,
    runtime: &shared_dsp::Snapshot,
) -> Option<crate::audio_route::AudioRouteProjection> {
    let current = app
        .state::<ConfigTransactionState>()
        .audio_transition_snapshot()?;
    if current.generation != transition.generation
        || current.state != crate::audio_route::RouteOperationState::Stable
        || current.requested != transition.requested
    {
        return None;
    }
    let observation = app
        .state::<SupervisorState>()
        .0
        .try_lock()
        .ok()?
        .audio_route_observation();
    confirmed_route_projection_from_observation(
        transition,
        runtime,
        observation,
        crate::next_audio_route_snapshot_generation(),
    )
}

fn confirmed_route_projection_from_observation(
    transition: ConfirmedRouteTransition,
    runtime: &shared_dsp::Snapshot,
    observation: crate::supervisor::AudioRouteObservation,
    snapshot_generation: u64,
) -> Option<crate::audio_route::AudioRouteProjection> {
    use crate::audio_route::{
        AudioAccess, AudioProcessing, AudioRouteSnapshot, RouteEvidence, RouteOperationState,
    };

    let route = transition.requested;
    if route.access != AudioAccess::Shared {
        // A successful DSP bypass does not prove physical Direct PCM ownership.
        return None;
    }
    let mut applied = route.clone();
    applied.processing = AudioProcessing::Off;
    if observation.null_output_test
        || observation.operation != RouteOperationState::Stable
        || observation.applied.as_ref() != Some(&applied)
        || observation.eq_pipe_configured != (route.processing == AudioProcessing::Equalizer)
    {
        return None;
    }
    let mut evidence = vec![
        RouteEvidence::PersistedConfiguration,
        RouteEvidence::SupervisorOutputApplied,
        RouteEvidence::MpdProtocolAvailable,
    ];
    if route.processing == AudioProcessing::Equalizer {
        if !runtime.verified
            || runtime.in_path != Some(true)
            || !matches!(runtime.phase, shared_dsp::Phase::Ready | shared_dsp::Phase::Active)
        {
            return None;
        }
        evidence.push(RouteEvidence::DspGraphVerified);
    }
    Some(crate::audio_route::AudioRouteProjection {
        generation: snapshot_generation,
        operation_id: Some(format!("route:{}", transition.generation)),
        operation: RouteOperationState::Stable,
        route: AudioRouteSnapshot::confirmed(route.clone(), route.clone()).ok(),
        applied: Some(applied),
        evidence,
        diagnostic: None,
        pcm_acquired: None,
        exclusive_access_confirmed: None,
        legacy_issues: transition.legacy_issues,
    })
}
async fn set_route_admitted(
    app: &AppHandle,
    preset: Option<EqPreset>,
    resume_requested: bool,
    route: config_save_operation::AudioTransitionHandle,
) -> Result<(), String> {
    let preflight_started = std::time::Instant::now();
    let watch = if resume_requested {
        Some(
            app.state::<AudioState>()
                .0
                .lock()
                .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                .watch_eq_transition(),
        )
    } else {
        None
    };
    let transactions = app.state::<ConfigTransactionState>();
    if transactions.shutdown_requested() {
        return Err("EQ_CANCELLED".into());
    }
    let config = shared_config(app)?;
    shared_dsp::diagnostics::event(
        "audio.route_selected",
        "info",
        Some(preflight_started.elapsed().as_millis()),
        serde_json::json!({
            "route": match config.audio_output_type.as_str() { "pipewire" => "pipewire_shared", "shared" => "shared", "alsa" => "alsa_direct", _ => "other" },
            "eq_route_supported": crate::is_shared_output(&config),
        }),
    );
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    let current_pipe = app
        .state::<SupervisorState>()
        .0
        .lock()
        .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
        .shared_eq_pipe();
    if preset.is_none() && current_pipe.is_none() {
        let worker = app.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            worker
                .state::<shared_dsp::State>()
                .stop()
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "EQ_STOP_FAILED")?;
    }
    shared_dsp::diagnostics::event(
        "route.preflight",
        "ok",
        Some(preflight_started.elapsed().as_millis()),
        serde_json::json!({"enabling": preset.is_some(), "existing_eq_pipe": current_pipe.is_some()}),
    );
    let plex = app
        .state::<PlexState>()
        .0
        .lock()
        .map_err(|_| "EQ_QUEUE_UNAVAILABLE")?
        .clone();
    let sqlite_library = app.state::<SqliteLibraryState>().clone();
    let queue_started = std::time::Instant::now();
    let resolved = match resolve_device_switch_queue(&app.state::<AudioState>().0, &plex, Some(&sqlite_library), &config.local_folders).await {
        Ok(q) => {
            shared_dsp::diagnostics::event("route.queue_resolved", "ok", Some(queue_started.elapsed().as_millis()), serde_json::json!({"resolved": true}));
            q
        },
        Err(e) => {
            shared_dsp::diagnostics::event("route.queue_resolved", "error", Some(queue_started.elapsed().as_millis()), serde_json::json!({"code": shared_dsp::diagnostics::safe_error_code(&e)}));
            return Err(e);
        }
    };
    let worker = app.clone();
    let switch_started = std::time::Instant::now();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = switch_output(&worker, &config, &resolved, preset, watch, &route);
        if result.is_ok() {
            route.confirm();
        } else {
            route.fail();
        }
        result
    })
    .await
    .map_err(|_| "EQ_SWITCH_FAILED")?;
    shared_dsp::diagnostics::event(
        "route.switch",
        if result.is_ok() { "ok" } else { "error" },
        Some(switch_started.elapsed().as_millis()),
        serde_json::json!({"code": if result.is_ok() { "confirmed" } else { "switch_failed" }}),
    );
    let final_state = app.state::<shared_dsp::State>().snapshot();
    shared_dsp::diagnostics::event(
        "dsp.final_state",
        if result.is_ok() && final_state.verified { "confirmed" } else { "unconfirmed" },
        Some(switch_started.elapsed().as_millis()),
        serde_json::json!({
            "phase": final_state.phase,
            "verified": final_state.verified,
            "preset_present": final_state.applied.is_some(),
            "error": final_state.error,
        }),
    );
    result
}

fn prepare_eq_after_playback_suspension<T>(
    snapshot: Option<audio::DeviceSwitchSnapshot>,
    prepare: impl FnOnce() -> Result<T, String>,
    restore_paused: impl FnOnce(Option<audio::DeviceSwitchSnapshot>) -> Result<(), String>,
) -> Result<T, (String, Result<(), String>)> {
    match prepare() {
        Ok(prepared) => Ok(prepared),
        Err(cause) => Err((cause, restore_paused(snapshot))),
    }
}

fn switch_output(
    app: &AppHandle,
    config: &AppConfig,
    queue: &ResolvedDeviceSwitchQueue,
    preset: Option<EqPreset>,
    watch: Option<audio::EqTransitionWatch>,
    route: &config_save_operation::AudioTransitionHandle,
) -> Result<(), String> {
    let switch_started = std::time::Instant::now();
    let tx = app.state::<ConfigTransactionState>();
    let dsp = app.state::<shared_dsp::State>();
    let is_enabling = preset.is_some();
    let failure_stage = Cell::new("transition_admission");
    let failure_logged = Cell::new(false);
    let result = (|| {
        failure_stage.set("restart_admission");
        let _restart = tx.try_begin_restart().ok_or("EQ_CANCELLED")?;
        failure_stage.set("shared_route_validation");
        shared_config(app)?;
        failure_stage.set("transition_superseded_check");
        if watch.as_ref().is_some_and(|w| !w.current()) {
            return Err("EQ_SUPERSEDED".into());
        }
        let audio = app.state::<AudioState>();
        let supervisor = app.state::<SupervisorState>();
        audio
            .0
            .lock()
            .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
            .invalidate_playback_intents();
        let preparation = {
            failure_stage.set("playback_suspension");
            audio
                .0
                .lock()
                .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                .prepare_reserved_device_switch(queue)
        };
        let snapshot = match preparation {
            Ok(s) => {
                shared_dsp::diagnostics::event("playback.suspended", "ok", Some(switch_started.elapsed().as_millis()), serde_json::json!({}));
                s
            }
            Err(error) => {
                shared_dsp::diagnostics::event("playback.suspended", "error", Some(switch_started.elapsed().as_millis()), serde_json::json!({"code": shared_dsp::diagnostics::safe_error_code(&error.cause)}));
                if error.snapshot.is_some() {
                    return Err(error_with_rollback(
                        error.cause,
                        restore_prepared_playback(error.snapshot, Some(queue), &audio),
                    ));
                }
                return Err(error.cause);
            }
        };
        let restore_paused = |saved| -> Result<(), String> {
            let outcome = audio
                .0
                .lock()
                .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                .restore_eq_switch(saved, queue)?;
            if outcome != audio::DeviceSwitchRestoreOutcome::Complete {
                return Err("EQ_POSITION_NOT_RESTORED".into());
            }
            Ok(())
        };
        let restore_failed_transition_paused = |saved| -> Result<(), String> {
            let outcome = audio
                .0
                .lock()
                .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                .restore_for_route_save(saved, queue)?;
            if outcome != audio::DeviceSwitchRestoreOutcome::Complete {
                return Err("EQ_POSITION_NOT_RESTORED".into());
            }
            Ok(())
        };
        let analyzer_state = app.state::<AnalyzerState>();
        let suspended = {
            failure_stage.set("analyzer_suspension");
            let mut analyzer = analyzer_state
                .0
                .lock()
                .map_err(|_| "EQ_ANALYZER_UNAVAILABLE");
            analyzer
                .as_mut()
                .map(|a| a.suspend_for_output_switch(app))
                .map_err(|e| *e)
        };
        let (capture, _) = match suspended {
            Ok(value) => value,
            Err(_) => {
                return Err(error_with_rollback(
                    "EQ_ANALYZER_UNAVAILABLE".into(),
                        restore_failed_transition_paused(snapshot),
                ))
            }
        };
        shared_dsp::diagnostics::event("analyzer.suspended", "ok", Some(switch_started.elapsed().as_millis()), serde_json::json!({}));
        let mut resume = AnalyzerSwitchResume::new(|capture| {
            app.state::<AnalyzerState>()
                .0
                .lock()
                .map_err(|_| "EQ_ANALYZER_UNAVAILABLE".to_string())?
                .resume_after_output_switch(capture, app, || tx.shutdown_requested())
        });
        resume.capture = capture;

        // The PipeWire/ALSA admission must observe a quiescent Sonante stream.
        // Starting Camilla while the previous MPD output is still playing can
        // expose an incomplete `/proc/asound/.../streamN` report and falsely
        // reject the selected AUX stereo sink. The queue has already been
        // captured and stopped above; prepare only after that point.
        let prepared = if let Some(preset) = preset {
            failure_stage.set("dsp_prepare");
            let prepare_started = std::time::Instant::now();
            shared_dsp::diagnostics::event(
                "route.dsp_prepare",
                "started",
                Some(switch_started.elapsed().as_millis()),
                serde_json::json!({"playback_suspended": true}),
            );
            let worker = app.clone();
            let callback = failure_callback(app);
            match prepare_eq_after_playback_suspension(
                snapshot,
                || {
                    worker
                        .state::<shared_dsp::State>()
                        .prepare(preset, callback)
                        .map_err(|error| error.to_string())
                },
                &restore_failed_transition_paused,
            ) {
                Ok(prepared) => {
                    shared_dsp::diagnostics::event(
                        "route.dsp_prepare",
                        "ok",
                        Some(prepare_started.elapsed().as_millis()),
                        serde_json::json!({"generation": prepared.generation}),
                    );
                    Some(prepared)
                }
                Err((cause, rollback)) => {
                    shared_dsp::diagnostics::event(
                        "route.dsp_prepare",
                        "error",
                        Some(prepare_started.elapsed().as_millis()),
                        serde_json::json!({
                            "code": shared_dsp::diagnostics::safe_error_code(&cause),
                            "queue_restored_paused": rollback.is_ok(),
                        }),
                    );
                    return Err(error_with_rollback(cause, resume.after_rollback(rollback)));
                }
            }
        } else {
            None
        };
        let pipe = prepared.as_ref().map(|p| p.pipe.clone());
        let changed = Cell::new(false);
        let applied = (|| {
            failure_stage.set("transition_cancellation_check");
            if tx.shutdown_requested() {
                return Err("EQ_CANCELLED".into());
            }
            failure_stage.set("dsp_prepared_validation");
            if prepared.as_ref().is_some_and(|p| !dsp.verify_prepared(p)) {
                shared_dsp::diagnostics::event(
                    "dsp.prepared_validation",
                    "error",
                    Some(switch_started.elapsed().as_millis()),
                    serde_json::json!({"code": "prepared_generation_unconfirmed"}),
                );
                return Err("EQ_CANCELLED".into());
            }
            if prepared.is_some() {
                shared_dsp::diagnostics::event(
                    "dsp.prepared_validation",
                    "ok",
                    Some(switch_started.elapsed().as_millis()),
                    serde_json::json!({"generation_bound": true}),
                );
            }
            // Mark before the first process/output mutation (including DSP
            // stop); successful rollback can later refine this to Failed.
            route.mark_uncertain();
            if pipe.is_none() {
                failure_stage.set("dsp_stop");
                dsp.stop().map_err(|e| e.to_string())?;
            }
            changed.set(true); // start may stop the old process before failing.
            failure_stage.set("mpd_eq_output_start");
            let mpd_started = std::time::Instant::now();
            let output_result = supervisor
                .0
                .lock()
                .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
                .start_shared_eq_output(config, pipe);
            if let Err(error) = output_result {
                shared_dsp::diagnostics::event("mpd.eq_output_start", "error", Some(mpd_started.elapsed().as_millis()), serde_json::json!({"code": shared_dsp::diagnostics::safe_error_code(&error), "enabling": is_enabling}));
                return Err(error);
            }
            shared_dsp::diagnostics::event("mpd.eq_output_start", "ok", Some(mpd_started.elapsed().as_millis()), serde_json::json!({"enabling": is_enabling}));
            if tx.shutdown_requested() {
                return Err("EQ_CANCELLED".into());
            }
            failure_stage.set("playback_position_restore");
            let restore_started = std::time::Instant::now();
            restore_paused(snapshot)?;
            shared_dsp::diagnostics::event("playback.position_restored", "ok", Some(restore_started.elapsed().as_millis()), serde_json::json!({}));
            if is_enabling {
                failure_stage.set("dsp_commit");
                dsp.commit().map_err(|e| e.to_string())?;
                shared_dsp::diagnostics::event("dsp.route_committed", "ok", Some(switch_started.elapsed().as_millis()), serde_json::json!({}));
            }
            if let Some(watch) = &watch {
                failure_stage.set("playback_resume");
                if tx.shutdown_requested() {
                    return Err("EQ_CANCELLED".into());
                }
                let result = audio
                    .0
                    .lock()
                    .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                    .resume_eq_switch(snapshot, queue, watch);
                match result {
                    Ok(outcome) => {
                        dsp.record_transition(outcome);
                        shared_dsp::diagnostics::event("playback.resumed", "ok", Some(switch_started.elapsed().as_millis()), serde_json::json!({"transition": outcome}));
                    },
                    Err(e) => {
                        dsp.record_transition("unconfirmed");
                        shared_dsp::diagnostics::event("playback.resumed", "error", Some(switch_started.elapsed().as_millis()), serde_json::json!({"code": "transition_unconfirmed"}));
                        return Err(e);
                    }
                }
            }
            Ok(())
        })();
        if let Err(original) = applied {
            failure_logged.set(true);
            shared_dsp::diagnostics::event(
                "route.apply_stage",
                "error",
                Some(switch_started.elapsed().as_millis()),
                route_failure_details(failure_stage.get(), &original, changed.get()),
            );
            if tx.shutdown_requested() {
                return Err(format!("{original};EQ_CANCELLED"));
            }
            let rollback = (|| {
                shared_dsp::diagnostics::event("route.rollback", "started", Some(switch_started.elapsed().as_millis()), serde_json::json!({"changed_output": changed.get()}));
                if changed.get() {
                    dsp.stop().map_err(|e| e.to_string())?;
                    supervisor
                        .0
                        .lock()
                        .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
                        .start_shared_eq_output(config, None)?;
                }
                restore_failed_transition_paused(snapshot)
            })();
            if changed.get() {
                if rollback.is_ok() {
                    route.fail_after_rollback();
                } else {
                    route.mark_uncertain();
                }
            }
            shared_dsp::diagnostics::event(
                "route.rollback",
                if rollback.is_ok() { "ok" } else { "error" },
                Some(switch_started.elapsed().as_millis()),
                serde_json::json!({"code": rollback.as_ref().err().map(|error| shared_dsp::diagnostics::safe_error_code(error))}),
            );
            return Err(error_with_rollback(
                original,
                resume.after_rollback(rollback),
            ));
        }
        resume.confirmed = true;
        Ok(())
    })();
    if let Err(error) = &result {
        if !failure_logged.get() {
            shared_dsp::diagnostics::event(
                "route.pre_apply_failure",
                "error",
                Some(switch_started.elapsed().as_millis()),
                route_failure_details(failure_stage.get(), error, false),
            );
        }
    }
    // A dependency prepared before a refused queue/suspension cannot stay
    // published as applied. Do not stop an existing DSP on a preflight refusal.
    if result.is_err() && is_enabling {
        let stop_result = dsp.stop();
        shared_dsp::diagnostics::event("dsp.cleanup_after_error", if stop_result.is_ok() { "ok" } else { "error" }, Some(switch_started.elapsed().as_millis()), serde_json::json!({"code": if stop_result.is_ok() { "stopped" } else { "stop_failed" }}));
    }
    shared_dsp::diagnostics::event("audio.switch_result", if result.is_ok() { "ok" } else { "error" }, Some(switch_started.elapsed().as_millis()), serde_json::json!({"code": result.as_ref().err().map(|error| shared_dsp::diagnostics::safe_error_code(error))}));
    result
}

/// Called only AFTER the existing output-switch playback snapshot/suspension,
/// BEFORE opening another device. The coordinator already owns admission.
pub(crate) fn stop_for_output_switch(app: &AppHandle) -> Result<(), String> {
    let supervisor = app.state::<SupervisorState>();
    let needs_stop = supervisor
        .0
        .lock()
        .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
        .shared_eq_pipe()
        .is_some();
    if needs_stop {
        app.state::<shared_dsp::State>()
            .stop()
            .map_err(|e| e.to_string())?;
        supervisor
            .0
            .lock()
            .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
            .clear_shared_eq_pipe();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn my_headphones_edits_persist_offline_without_changing_active_preset_or_eq_state() {
        let dir = std::env::temp_dir().join(format!("sonante-my-headphones-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let store = EqualizerStore::new(dir.join("eq.json"));
        let mut initial = EqualizerConfig::default();
        let saved = initial.create_user_preset("Travel", None).unwrap();
        initial.set_enabled(true).unwrap();
        let active = initial.active_preset.clone();
        store.save(&initial).unwrap();

        let current = store.load().unwrap();
        edit_store(&store, &revision(&current).unwrap(), Edit::AddEquipment {
            id: None, name: "Edition XS".into(), category: EquipmentCategory::Headphones, manufacturer: Some("HIFIMAN".into()),
            opra_vendor_id: Some("hifiman".into()), opra_product_id: Some("hifiman_edition_xs".into()),
        }).unwrap();
        let current = store.load().unwrap();
        let equipment = current.equipment_profiles[0].clone();
        assert_eq!(equipment.category, EquipmentCategory::Headphones);
        assert!(equipment.presets.is_empty());
        assert_eq!(equipment.opra_product_id.as_deref(), Some("hifiman_edition_xs"));
        assert!(current.enabled);
        assert_eq!(current.active_preset, active);
        assert_eq!(current.user_presets, vec![saved.clone()]);

        let reference = PresetRef::User { id: saved.id.clone() };
        edit_store(&store, &revision(&current).unwrap(), Edit::AddEquipmentPreset { id: equipment.id.clone(), reference: reference.clone(), preferred: true }).unwrap();
        let current = store.load().unwrap();
        assert_eq!(current.equipment_profiles[0].presets, vec![reference.clone()]);
        assert_eq!(current.equipment_profiles[0].preferred_preset, Some(reference.clone()));
        edit_store(&store, &revision(&current).unwrap(), Edit::UnlinkEquipment { id: equipment.id }).unwrap();
        let current = store.load().unwrap();
        assert!(current.equipment_profiles.is_empty());
        assert_eq!(current.user_presets, vec![saved]);
        assert!(current.enabled);
        assert_eq!(current.active_preset, active);
        std::fs::remove_dir_all(dir).unwrap();
    }
    use std::sync::{Arc, Mutex};

    fn route_confirmation_fixture(processing: crate::audio_route::AudioProcessing) -> ConfirmedRouteTransition {
        ConfirmedRouteTransition {
            generation: 17,
            requested: crate::audio_route::AudioRoute {
                destination: crate::audio_route::AudioDestination::SystemDefault,
                access: crate::audio_route::AudioAccess::Shared,
                processing,
            },
            legacy_issues: Vec::new(),
        }
    }

    fn supervisor_route_fixture(eq_pipe_configured: bool) -> crate::supervisor::AudioRouteObservation {
        crate::supervisor::AudioRouteObservation {
            generation: 9,
            operation: crate::audio_route::RouteOperationState::Stable,
            applied: Some(crate::audio_route::AudioRoute {
                destination: crate::audio_route::AudioDestination::SystemDefault,
                access: crate::audio_route::AudioAccess::Shared,
                processing: crate::audio_route::AudioProcessing::Off,
            }),
            null_output_test: false,
            eq_pipe_configured,
        }
    }

    #[test]
    fn confirmed_eq_command_returns_a_generation_fenced_route_projection() {
        let mut runtime = shared_dsp::Snapshot {
            phase: shared_dsp::Phase::Ready,
            verified: true,
            in_path: Some(true),
            applied: Some(crate::equalizer::flat_preset()),
            ..Default::default()
        };
        let projection = confirmed_route_projection_from_observation(
            route_confirmation_fixture(crate::audio_route::AudioProcessing::Equalizer),
            &runtime,
            supervisor_route_fixture(true),
            42,
        ).unwrap();
        assert_eq!(projection.generation, 42);
        assert_eq!(projection.operation_id.as_deref(), Some("route:17"));
        assert_eq!(projection.operation, crate::audio_route::RouteOperationState::Stable);
        let route = projection.route.unwrap();
        assert_eq!(route.state, crate::audio_route::AudioRouteState::Confirmed);
        assert_eq!(route.confirmed.unwrap().processing, crate::audio_route::AudioProcessing::Equalizer);
        assert!(projection.evidence.contains(&crate::audio_route::RouteEvidence::DspGraphVerified));
        assert_eq!(projection.pcm_acquired, None, "Shared+EQ confirmation is not Direct PCM ownership");

        runtime.verified = false;
        assert!(confirmed_route_projection_from_observation(
            route_confirmation_fixture(crate::audio_route::AudioProcessing::Equalizer),
            &runtime,
            supervisor_route_fixture(true),
            43,
        ).is_none(), "unverified DSP state cannot confirm the UI route");
    }

    #[test]
    fn unavailable_or_null_route_observations_are_not_reported_as_confirmed() {
        let runtime = shared_dsp::Snapshot::default();
        assert!(confirmed_route_projection_from_observation(
            route_confirmation_fixture(crate::audio_route::AudioProcessing::Off),
            &runtime,
            supervisor_route_fixture(true),
            1,
        ).is_none(), "EQ pipe must be absent after a confirmed bypass");
        let mut unavailable = supervisor_route_fixture(false);
        unavailable.null_output_test = true;
        assert!(confirmed_route_projection_from_observation(
            route_confirmation_fixture(crate::audio_route::AudioProcessing::Off),
            &runtime,
            unavailable,
            2,
        ).is_none(), "null output never claims a physical output route");
    }

    #[test]
    fn failed_eq_prepare_restores_queue_without_autoplay_for_playing_paused_and_stopped_origins() {
        for origin in [audio::PlaybackState::Playing, audio::PlaybackState::Paused, audio::PlaybackState::Stopped] {
            // `prepare_reserved_device_switch` has already stopped an active
            // MPD stream before this helper is called.
            let playback = Arc::new(Mutex::new(audio::PlaybackState::Stopped));
            let effects = Arc::new(Mutex::new(Vec::<&'static str>::new()));
            let snapshot = Some(audio::DeviceSwitchSnapshot {
                queue_index: 2,
                elapsed: 31.25,
                state: origin,
            });
            let prepare_playback = playback.clone();
            let prepare_effects = effects.clone();
            let restore_playback = playback.clone();
            let restore_effects = effects.clone();

            let result = prepare_eq_after_playback_suspension(
                snapshot,
                move || {
                    assert_eq!(
                        *prepare_playback.lock().unwrap(),
                        audio::PlaybackState::Stopped
                    );
                    prepare_effects.lock().unwrap().push("prepare_dsp");
                    Err::<(), _>("graph_invalid".into())
                },
                move |saved| {
                    assert_eq!(saved, snapshot);
                    restore_effects.lock().unwrap().push("restore_queue_paused");
                    *restore_playback.lock().unwrap() = match origin {
                        audio::PlaybackState::Playing | audio::PlaybackState::Paused => audio::PlaybackState::Paused,
                        audio::PlaybackState::Stopped => audio::PlaybackState::Stopped,
                    };
                    Ok(())
                },
            );

            let (cause, rollback) = result.unwrap_err();
            assert_eq!(cause, "graph_invalid");
            assert!(rollback.is_ok());
            assert_eq!(
                *effects.lock().unwrap(),
                ["prepare_dsp", "restore_queue_paused"]
            );
            assert_eq!(
                *playback.lock().unwrap(),
                match origin {
                    audio::PlaybackState::Playing | audio::PlaybackState::Paused => audio::PlaybackState::Paused,
                    audio::PlaybackState::Stopped => audio::PlaybackState::Stopped,
                }
            );
            assert!(!effects.lock().unwrap().contains(&"resume_playback"));
        }
    }

    #[test]
    fn runtime_admission_errors_are_distinct_and_sanitized() {
        assert_eq!(
            camilla_runtime_diagnostic(shared_dsp::Error::RuntimeMissing),
            "EQ_CAMILLA_RUNTIME_MISSING"
        );
        assert_eq!(
            camilla_runtime_diagnostic(shared_dsp::Error::RuntimeUntrusted),
            "EQ_CAMILLA_RUNTIME_UNTRUSTED"
        );
        assert_eq!(
            mpd_eq_runtime_diagnostic("EQ_MPD_RUNTIME_MISSING"),
            "EQ_MPD_RUNTIME_MISSING"
        );
        assert_eq!(
            mpd_eq_runtime_diagnostic("private path /home/user/runtime"),
            "EQ_MPD_RUNTIME_UNAVAILABLE"
        );
    }

    #[test]
    fn route_failure_diagnostic_keeps_the_exact_stage_and_never_logs_raw_error_text() {
        let details = route_failure_details(
            "dsp_prepared_validation",
            "EQ_GraphInvalid; private path /home/user/music",
            true,
        );
        assert_eq!(details["stage"], "dsp_prepared_validation");
        assert_eq!(details["code"], "unclassified");
        assert_eq!(details["output_changed"], true);
        assert!(!details.to_string().contains("/home/user/music"));

        let known = route_failure_details("mpd_eq_output_start", "EQ_GraphInvalid", false);
        assert_eq!(known["code"], "graph_invalid");
    }

    fn private_equalizer_store(label: &str) -> (std::path::PathBuf, EqualizerStore) {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "sonante-eq-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let store = EqualizerStore::new(root.join("eq.json"));
        (root, store)
    }

    #[test]
    fn enabled_intent_and_selected_preset_survive_edits_and_restart_load() {
        let (root, store) = private_equalizer_store("intent");
        let initial = store.load().unwrap();
        let mut tuned = crate::equalizer::flat_preset();
        tuned.bands[0].enabled = true;
        tuned.bands[0].gain_db = 3.5;
        edit_store(
            &store,
            &revision(&initial).unwrap(),
            Edit::Create { name: "Desk".into(), source: None, preset: Some(tuned.clone()) },
        )
        .unwrap();
        let created = store.load().unwrap();
        let selected = created.active_preset.clone();
        let selected_id = match &selected { PresetRef::User { id } => id.clone(), _ => panic!("user preset expected") };
        assert_eq!(created.preset(&selected).unwrap().bands[0].gain_db, 3.5);

        assert!(!created.enabled, "a newly created EQ remains explicitly disabled");
        assert!(!startup_restore_allowed(created.enabled, true, true, true, true, false, false, true, false));

        update_preference(&store, &PreferenceUpdate::SetEnabled(true)).unwrap();
        let enabled = store.load().unwrap();
        edit_store(
            &store,
            &revision(&enabled).unwrap(),
            Edit::Rename { id: selected_id, name: "Desk revised".into() },
        )
        .unwrap();
        // A fresh load models the next application launch: the desired bit and
        // complete preset data are restored from the private store.
        let restarted = store.load().unwrap();
        assert!(restarted.enabled);
        assert_eq!(restarted.active_preset, selected);
        let restored_preset = restarted.preset(&restarted.active_preset).unwrap();
        assert_eq!(restored_preset.bands, tuned.bands);
        assert_eq!(restored_preset.name, "Desk revised");
        let (_, saved_revision) = load_from_store(&store).unwrap();
        update_preference(
            &store,
            &PreferenceUpdate::RequireEnabledRevision(saved_revision.clone()),
        )
        .unwrap();
        assert!(startup_restore_allowed(true, true, true, true, true, false, false, true, false));

        update_preference(&store, &PreferenceUpdate::SetEnabled(false)).unwrap();
        assert!(!store.load().unwrap().enabled);
        assert_eq!(
            update_preference(
                &store,
                &PreferenceUpdate::RequireEnabledRevision(saved_revision)
            ),
            Err("EQ_STARTUP_RESTORE_SUPERSEDED".into()),
            "a stale startup request cannot re-enable an explicitly disabled EQ"
        );
        assert_eq!(store.load().unwrap().active_preset, selected);
        std::fs::remove_file(root.join("eq.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn explicit_enable_preference_is_persisted_only_after_route_confirmation() {
        let (root, store) = private_equalizer_store("confirmed-intent");
        let enabling = PreferenceUpdate::SetEnabled(true);
        validate_preference_before_route(&store, &enabling).unwrap();
        assert!(
            !store.load().unwrap().enabled,
            "failed activation leaves the last confirmed preference unchanged"
        );
        // The caller invokes this only after set_route_admitted returns success
        // and the route handle has been confirmed.
        persist_preference_after_route(&store, &enabling).unwrap();
        assert!(store.load().unwrap().enabled);

        let disabling = PreferenceUpdate::SetEnabled(false);
        validate_preference_before_route(&store, &disabling).unwrap();
        assert!(
            store.load().unwrap().enabled,
            "a pending bypass does not rewrite the saved state"
        );
        persist_preference_after_route(&store, &disabling).unwrap();
        assert!(!store.load().unwrap().enabled);

        std::fs::remove_file(root.join("eq.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn startup_restore_requires_owned_shared_ready_and_idle_audio() {
        let safe = (true, true, true, true, true, false, false, true, false);
        assert!(startup_restore_allowed(safe.0, safe.1, safe.2, safe.3, safe.4, safe.5, safe.6, safe.7, safe.8));
        for index in 0..9 {
            let mut conditions = safe;
            match index {
                0 => conditions.0 = false, // Explicitly disabled.
                1 => conditions.1 = false, // Direct or first-run route.
                2 => conditions.2 = false, // No owned MPD process.
                3 => conditions.3 = false, // Shared route not confirmed.
                4 => conditions.4 = false, // Startup queue not ready.
                5 => conditions.5 = true,  // Isolated null output test.
                6 => conditions.6 = true,  // Existing EQ pipe.
                7 => conditions.7 = false, // Runtime is not idle.
                _ => conditions.8 = true,   // Shutdown requested.
            }
            assert!(
                !startup_restore_allowed(
                    conditions.0, conditions.1, conditions.2, conditions.3,
                    conditions.4, conditions.5, conditions.6, conditions.7, conditions.8
                ),
                "unsafe startup condition {index} must defer restoration"
            );
        }

        let state = shared_dsp::State::new();
        assert!(state.begin_restore());
        let preparing = state.snapshot();
        assert_eq!(preparing.phase, shared_dsp::Phase::Preparing);
        assert!(!preparing.verified);
        assert!(!startup_restore_confirmed(&preparing));
        state.cancel_restore();
        assert_eq!(state.snapshot().phase, shared_dsp::Phase::Off);
        state.begin_shutdown();
        assert!(!state.begin_restore());
        state.join();

        let confirmed = shared_dsp::Snapshot {
            phase: shared_dsp::Phase::Ready,
            applied: Some(crate::equalizer::flat_preset()),
            in_path: Some(true),
            verified: true,
            ..Default::default()
        };
        assert!(startup_restore_confirmed(&confirmed));
        assert!(!startup_restore_confirmed(&shared_dsp::Snapshot {
            in_path: Some(false),
            ..confirmed
        }));
        assert!(!startup_restore_confirmed(&shared_dsp::Snapshot {
            phase: shared_dsp::Phase::Error,
            error: Some(shared_dsp::Error::ApplyFailed),
            ..Default::default()
        }));
    }

    #[test]
    fn normal_dsp_shutdown_does_not_change_saved_eq_intent() {
        let (root, store) = private_equalizer_store("shutdown");
        update_preference(&store, &PreferenceUpdate::SetEnabled(true)).unwrap();
        let dsp = shared_dsp::State::new();
        dsp.begin_shutdown();
        dsp.join();
        assert!(store.load().unwrap().enabled);
        std::fs::remove_file(root.join("eq.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn autoeq_copy_persists_without_changing_selected_preset_or_enable_intent() {
        let (root, store) = private_equalizer_store("autoeq-copy");
        let initial = store.load().unwrap();
        edit_store(
            &store,
            &revision(&initial).unwrap(),
            Edit::Create { name: "Current".into(), source: None, preset: Some(equalizer::flat_preset()) },
        ).unwrap();
        update_preference(&store, &PreferenceUpdate::SetEnabled(true)).unwrap();
        let before = store.load().unwrap();
        let selected = before.active_preset.clone();
        let document = "Preamp: 2 dB\nFilter 1: ON LSC Fc 105 Hz Gain 3 dB Q 0.7\n".to_string();
        edit_store(&store, &revision(&before).unwrap(), Edit::ImportAutoEq { document, name: "Imported profile".into() }).unwrap();

        // Reload from disk: import persists the full copy but does not select,
        // enable, apply, or otherwise change the current EQ intent.
        let after = store.load().unwrap();
        assert_eq!(after.active_preset, selected);
        assert!(after.enabled);
        assert_eq!(after.user_presets.len(), before.user_presets.len() + 1);
        let imported = after.user_presets.iter().find(|p| p.name == "Imported profile").unwrap();
        assert_eq!(imported.preamp_db, 2.0);
        assert_eq!(imported.bands[0].frequency_hz, 105.0);
        assert_eq!(imported.bands[0].filter_type, equalizer::EqFilterType::LowShelf);
        std::fs::remove_file(root.join("eq.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn private_crud_cas_does_not_activate_and_preserves_corruption() {
        let dir = std::env::temp_dir().join(format!("sonante-eq-crud-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("equalizer-shared-v1.json");
        let store = EqualizerStore::new(path.clone());
        let base = store.load().unwrap();
        let initial = revision(&base).unwrap();
        let mut preset = equalizer::flat_preset();
        preset.preamp_db = -4.;
        edit_store(
            &store,
            &initial,
            Edit::Create {
                name: "Desk".into(),
                source: None,
                preset: Some(preset.clone()),
            },
        )
        .unwrap();
        let saved = store.load().unwrap();
        assert!(!saved.enabled);
        assert_eq!(saved.active().unwrap().preamp_db, -4.);
        let id = saved.active().unwrap().id;
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            edit_store(
                &store,
                &initial,
                Edit::Rename {
                    id: id.clone(),
                    name: "Changed".into()
                }
            ),
            Err("EQ_STORE_CHANGED".into())
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        edit_store(
            &store,
            &revision(&saved).unwrap(),
            Edit::Rename {
                id: id.clone(),
                name: "Office".into(),
            },
        )
        .unwrap();
        let saved = store.load().unwrap();
        assert_eq!(saved.active().unwrap().name, "Office");
        preset.bands.pop();
        assert_eq!(
            edit_store(
                &store,
                &revision(&saved).unwrap(),
                Edit::Update {
                    id: id.clone(),
                    preset
                }
            ),
            Err("EQ_PRESET_INVALID".into())
        );
        edit_store(
            &store,
            &revision(&saved).unwrap(),
            Edit::Delete {
                reference: PresetRef::User { id },
            },
        )
        .unwrap();
        assert_eq!(store.load().unwrap().active().unwrap().id, "flat");
        std::fs::write(&path, b"broken").unwrap();
        assert!(edit_store(
            &store,
            "",
            Edit::Select {
                reference: PresetRef::BuiltIn { id: "flat".into() }
            }
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

#[cfg(test)]
#[test]
fn bypass_level_return_ack_is_bound_to_current_profile_before_any_effect() {
    let p = crate::equalizer::flat_preset();
    let current = shared_dsp::Snapshot {
        applied: Some(p.clone()),
        verified: true,
        ..Default::default()
    };
    assert!(confirm_level_return(&current, None).is_err());
    assert!(confirm_level_return(&current, Some(&p)).is_ok());
    let mut different = p.clone();
    different.preamp_db = -6.;
    assert!(confirm_level_return(&current, Some(&different)).is_err());
    assert!(confirm_level_return(&Default::default(), None).is_ok());
}

#[cfg(test)]
mod daily_tests {
 use super::*;
 #[test]
 fn imported_copy_and_equipment_use_cas_without_activation_or_lost_references() {
  let root=std::env::temp_dir().join(format!("seq-daily-cas-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let store=EqualizerStore::new(root.join("eq.json"));
  let document=crate::equalizer_portable::export(&crate::equalizer::built_in_presets()[2]).unwrap();let initial=store.load().unwrap();let rev=revision(&initial).unwrap();
  edit_store(&store,&rev,Edit::Import{document:document.clone(),name:"Travel".into()}).unwrap();let c=store.load().unwrap();assert!(!c.enabled);assert_eq!(c.user_presets.len(),1);assert_ne!(c.user_presets[0].id,"import_preview");let selected=c.active_preset.clone();
  let bytes=std::fs::read(root.join("eq.json")).unwrap();assert_eq!(edit_store(&store,&rev,Edit::Import{document:document.clone(),name:"Stale".into()}),Err("EQ_STORE_CHANGED".into()));assert_eq!(std::fs::read(root.join("eq.json")).unwrap(),bytes);
  assert!(edit_store(&store,&revision(&c).unwrap(),Edit::Import{document,name:"Travel".into()}).is_err());assert_eq!(std::fs::read(root.join("eq.json")).unwrap(),bytes);
  edit_store(&store,&revision(&c).unwrap(),Edit::LinkEquipment{id:None,name:"DAC and headphones".into(),reference:selected.clone()}).unwrap();let c=store.load().unwrap();assert!(!c.enabled);let equipment=c.equipment_profiles[0].clone();
  assert!(edit_store(&store,&revision(&c).unwrap(),Edit::Delete{reference:selected.clone()}).is_err());assert_eq!(store.load().unwrap(),c);
  edit_store(&store,&revision(&c).unwrap(),Edit::Rename{id:c.user_presets[0].id.clone(),name:"Renamed".into()}).unwrap();let c=store.load().unwrap();assert_eq!(c.equipment_profiles[0].preferred_preset,Some(selected.clone()));
  edit_store(&store,&revision(&c).unwrap(),Edit::UnlinkEquipment{id:equipment.id}).unwrap();let c=store.load().unwrap();assert_eq!(c.user_presets.len(),1);
  edit_store(&store,&revision(&c).unwrap(),Edit::Delete{reference:selected}).unwrap();assert!(store.load().unwrap().user_presets.is_empty());std::fs::remove_file(root.join("eq.json")).unwrap();std::fs::remove_dir(root).unwrap();
 }
 #[test]
 fn equipment_is_optional_in_legacy_schema_and_references_names_and_limits_are_validated() {
  let c=EqualizerConfig::default();let mut v=serde_json::to_value(&c).unwrap();v.as_object_mut().unwrap().remove("equipment_profiles");let mut c:EqualizerConfig=serde_json::from_value(v).unwrap();assert!(c.equipment_profiles.is_empty());
  let flat=PresetRef::BuiltIn{id:"flat".into()};c.link_equipment(None,"Desk",flat.clone()).unwrap();let saved=c.clone();assert!(c.link_equipment(None,"desk",flat.clone()).is_err());assert_eq!(c,saved);
  assert!(c.link_equipment(Some("missing".into()),"Unknown",flat.clone()).is_err());assert!(c.link_equipment(None,"Bad",PresetRef::User{id:"missing".into()}).is_err());assert_eq!(c,saved);
  for i in 1..32 {c.link_equipment(None,&format!("equipment {i}"),flat.clone()).unwrap();}let saved=c.clone();assert!(c.link_equipment(None,"Overflow",flat).is_err());assert_eq!(c,saved);assert!(!c.enabled);
 }
}

#[cfg(test)]
#[test]
fn reference_bypass_confirmation_remains_bound_to_the_actual_negative_gain_source() {
    let p=crate::equalizer::built_in_presets()[2].clone();
    let snapshot=shared_dsp::Snapshot{comparison:Some(shared_dsp::comparison::Comparison{token:"owned".into(),side:shared_dsp::comparison::Side::Reference,source:p.clone(),effective_gain_db:shared_dsp::safety::assess(&p,48000).unwrap().effective_preamp_db,interrupted:false}),applied:None,verified:true,..Default::default()};
    assert!(confirm_level_return(&snapshot,None).is_err());assert!(confirm_level_return(&snapshot,Some(&p)).is_ok());let mut different=p.clone();different.preamp_db-=1.;assert!(confirm_level_return(&snapshot,Some(&different)).is_err());
}
