//! UI/persistence and lifecycle orchestration; preset data never activates on load.
use crate::equalizer::{EqPreset, EqualizerConfig, EqualizerStore, PresetRef};
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
    let result = tauri::async_runtime::spawn_blocking(load)
        .await
        .map_err(|_| "EQ_STORE_UNAVAILABLE")??;
    let (mut config, revision) = result;
    config.enabled = false; // Stored desire is never runtime truth.
    let shared = app
        .state::<ConfigState>()
        .0
        .lock()
        .map(|c| crate::is_shared_output(&c))
        .unwrap_or(false);
    // Presence only; verifying the pinned executable belongs to activation,
    // not catalog loading or normal playback/startup.
    let runtime_available =
        cfg!(debug_assertions) && std::env::var_os("SONANTE_EQ_CAMILLA_PATH").is_some() && std::env::var_os("SONANTE_EQ_MPD_PATH").is_some();
    Ok(Catalog {
        built_ins: crate::equalizer::built_in_presets(),
        builtin_groups: crate::equalizer::builtin_groups(),
        config,
        revision,
        runtime: runtime_snapshot(&app),
        shared,
        runtime_available,
    })
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Edit {
    Import { document: String, name: String },
    LinkEquipment { id: Option<String>, name: String, reference: PresetRef },
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
        Edit::LinkEquipment { id, name, reference } => config.link_equipment(id,&name,reference),
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
    config.set_enabled(false).map_err(|_| "EQ_PRESET_INVALID")?;
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
            let tx = app.state::<ConfigTransactionState>();
            loop {
                if tx.shutdown_requested() {
                    return;
                }
                match tx.operations.try_begin_background() {
                    Err(config_save_operation::BackgroundAdmission::Busy) => {
                        let _ = tauri::async_runtime::spawn_blocking(|| {
                            std::thread::sleep(Duration::from_secs(1))
                        })
                        .await;
                    }
                    Err(config_save_operation::BackgroundAdmission::Blocked) => return,
                    Ok(_guard) => {
                        if shared_config(&app).is_ok() {
                            let result = set_route_admitted(&app, None, false).await;
                            if result.is_err() {
                                eprintln!("[Equalizer] fallback=unconfirmed");
                            }
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
) -> Result<shared_dsp::Snapshot, String> {
    app.state::<AudioState>().0.lock().map_err(|_|"EQ_AUDIO_UNAVAILABLE")?.note_eq_intervention();
    if enabled {
        shared_config(&app)?;
        let preset = preset.ok_or("EQ_PRESET_REQUIRED")?;
        crate::equalizer::validate_v1(&preset).map_err(|_| "EQ_PRESET_INVALID")?;
        shared_dsp::safety::assess(&preset, 48000).map_err(|_| "EQ_UNSAFE_HEADROOM")?;
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
            tauri::async_runtime::spawn_blocking(move || {
                let tx = worker.state::<ConfigTransactionState>();
                let _guard = tx.begin()?;
                shared_config(&worker)?;
                worker
                    .state::<shared_dsp::State>()
                    .apply(preset)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| "EQ_APPLY_FAILED")??;
        } else {
            set_route(&app, Some(preset), None).await?;
        }
    } else {
        set_route(&app, None, bypass_expected).await?;
    }
    Ok(runtime_snapshot(&app))
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
) -> Result<(), String> {
    let transactions = app.state::<ConfigTransactionState>();
    let _guard = transactions.begin()?;
    let current = app.state::<shared_dsp::State>().snapshot();
    if preset.is_none() {
        confirm_level_return(&current, bypass_expected.as_ref())?;
    }
    // A failed DSP cleanup is not a healthy transition eligible for autoplay.
    let may_resume = preset.is_some() || (current.verified && (current.applied.is_some()||current.comparison.is_some()));
    set_route_admitted(app, preset, may_resume).await
}
async fn set_route_admitted(
    app: &AppHandle,
    preset: Option<EqPreset>,
    resume_requested: bool,
) -> Result<(), String> {
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
    let plex = app
        .state::<PlexState>()
        .0
        .lock()
        .map_err(|_| "EQ_QUEUE_UNAVAILABLE")?
        .clone();
    // Prepare dependency/config/graph BEFORE pausing or altering the old output.
    let prepared = if let Some(preset) = preset {
        let worker = app.clone();
        let callback = failure_callback(app);
        Some(
            tauri::async_runtime::spawn_blocking(move || {
                worker
                    .state::<shared_dsp::State>()
                    .prepare(preset, callback)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| "EQ_PREPARE_FAILED")??,
        )
    } else {
        None
    };
    let resolved = match resolve_device_switch_queue(&app.state::<AudioState>().0, &plex).await {
        Ok(q) => q,
        Err(e) => {
            // Only a newly prepared dependency belongs to this failed attempt.
            // Bypass resolution refusal must leave an existing DSP untouched.
            if prepared.is_some() {
                let worker = app.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    worker.state::<shared_dsp::State>().stop()
                })
                .await;
            }
            return Err(e);
        }
    };
    let worker = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        switch_output(&worker, &config, &resolved, prepared, watch)
    })
    .await
    .map_err(|_| "EQ_SWITCH_FAILED")?
}

fn switch_output(
    app: &AppHandle,
    config: &AppConfig,
    queue: &ResolvedDeviceSwitchQueue,
    prepared: Option<shared_dsp::Prepared>,
    watch: Option<audio::EqTransitionWatch>,
) -> Result<(), String> {
    let tx = app.state::<ConfigTransactionState>();
    let dsp = app.state::<shared_dsp::State>();
    let is_enabling = prepared.is_some();
    let result = (|| {
        let _restart = tx.try_begin_restart().ok_or("EQ_CANCELLED")?;
        shared_config(app)?;
        if watch.as_ref().is_some_and(|w| !w.current()) {
            return Err("EQ_SUPERSEDED".into());
        }
        if prepared.as_ref().is_some_and(|p| !dsp.verify_prepared(p)) {
            return Err("EQ_CANCELLED".into());
        }
        let audio = app.state::<AudioState>();
        let supervisor = app.state::<SupervisorState>();
        audio
            .0
            .lock()
            .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
            .invalidate_playback_intents();
        let preparation = {
            audio
                .0
                .lock()
                .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                .prepare_reserved_device_switch(queue)
        };
        let snapshot = match preparation {
            Ok(s) => s,
            Err(error) => {
                if error.snapshot.is_some() {
                    return Err(error_with_rollback(
                        error.cause,
                        restore_prepared_playback(error.snapshot, Some(queue), &audio),
                    ));
                }
                return Err(error.cause);
            }
        };
        let restore_paused = || -> Result<(), String> {
            let outcome = audio
                .0
                .lock()
                .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                .restore_eq_switch(snapshot, queue)?;
            if outcome != audio::DeviceSwitchRestoreOutcome::Complete {
                return Err("EQ_POSITION_NOT_RESTORED".into());
            }
            Ok(())
        };
        let analyzer_state = app.state::<AnalyzerState>();
        let suspended = {
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
                    restore_paused(),
                ))
            }
        };
        let mut resume = AnalyzerSwitchResume::new(|capture| {
            app.state::<AnalyzerState>()
                .0
                .lock()
                .map_err(|_| "EQ_ANALYZER_UNAVAILABLE".to_string())?
                .resume_after_output_switch(capture, app, || tx.shutdown_requested())
        });
        resume.capture = capture;
        let pipe = prepared.as_ref().map(|p| p.pipe.clone());
        let changed = Cell::new(false);
        let applied = (|| {
            if tx.shutdown_requested() {
                return Err("EQ_CANCELLED".into());
            }
            if pipe.is_none() {
                dsp.stop().map_err(|e| e.to_string())?;
            }
            changed.set(true); // start may stop the old process before failing.
            supervisor
                .0
                .lock()
                .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
                .start_shared_eq_output(config, pipe)?;
            if tx.shutdown_requested() {
                return Err("EQ_CANCELLED".into());
            }
            restore_paused()?;
            if is_enabling {
                dsp.commit().map_err(|e| e.to_string())?;
            }
            if let Some(watch) = &watch {
                if tx.shutdown_requested() {
                    return Err("EQ_CANCELLED".into());
                }
                let result = audio
                    .0
                    .lock()
                    .map_err(|_| "EQ_AUDIO_UNAVAILABLE")?
                    .resume_eq_switch(snapshot, queue, watch);
                match result {
                    Ok(outcome) => dsp.record_transition(outcome),
                    Err(e) => {
                        dsp.record_transition("unconfirmed");
                        return Err(e);
                    }
                }
            }
            Ok(())
        })();
        if let Err(original) = applied {
            if tx.shutdown_requested() {
                return Err(format!("{original};EQ_CANCELLED"));
            }
            let rollback = (|| {
                if changed.get() {
                    dsp.stop().map_err(|e| e.to_string())?;
                    supervisor
                        .0
                        .lock()
                        .map_err(|_| "EQ_SUPERVISOR_UNAVAILABLE")?
                        .start_shared_eq_output(config, None)?;
                }
                restore_paused()
            })();
            return Err(error_with_rollback(
                original,
                resume.after_rollback(rollback),
            ));
        }
        resume.confirmed = true;
        Ok(())
    })();
    // A dependency prepared before a refused queue/suspension cannot stay
    // published as applied. Do not stop an existing DSP on a preflight refusal.
    if result.is_err() && is_enabling {
        let _ = dsp.stop();
    }
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
  edit_store(&store,&revision(&c).unwrap(),Edit::Rename{id:c.user_presets[0].id.clone(),name:"Renamed".into()}).unwrap();let c=store.load().unwrap();assert_eq!(c.equipment_profiles[0].preset,selected);
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
