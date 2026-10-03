mod alsa_mixer;
mod analyzer;
mod audio;
mod config;
mod favorites;
mod persistence;
mod plex;
mod shared_volume;
mod supervisor;

use audio::{
    list_audio_devices, AudioDevice, AudioEngine, AudioState, DeviceSwitchSnapshot,
    MpdProbeFailure, PlaybackStatus, TrackMetadata, VolumeBackend, VolumeStatus,
};
use analyzer::{AnalyzerState, AudioAnalyzer};
use config::AppConfig;
use favorites::FavoriteAlbum;
use plex::{
    PlexAlbum, PlexClient, PlexCollection, PlexImageRef, PlexLibrary, PlexSearchResults, PlexTrack,
};
use serde::Serialize;
use shared_volume::{PipeWireVolume, SharedVolumeBackend};
use supervisor::{MpdHealth, MpdProcessObservation, MpdSupervisor, MpdUnavailableReason};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, RunEvent, State, Window, WindowEvent};

pub struct PlexState(pub Mutex<PlexClient>);
pub struct SupervisorState(pub Mutex<MpdSupervisor>);
pub struct ConfigState(pub Mutex<AppConfig>);
pub struct ConfigTransactionState(pub Mutex<()>);

const MAIN_WINDOW_LABEL: &str = "main";

fn should_exit_application_on_window_close(window_label: &str) -> bool {
    window_label == MAIN_WINDOW_LABEL
}

#[tauri::command]
fn start_audio_analyzer(
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    println!("[Analyzer] Start requested by window 'now-playing'.");
    analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?
        .start(app)
}

#[tauri::command]
fn stop_audio_analyzer(
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?
        .stop(Some(&app))
}

#[derive(Debug, Clone, Serialize)]
pub struct MpdStatusSnapshot {
    pub health: MpdHealth,
    pub playback: Option<PlaybackStatus>,
}

impl ConfigTransactionState {
    fn begin(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.0
            .lock()
            .map_err(|e| format!("Falha ao serializar a alteração de configuração: {}", e))
    }
}

#[tauri::command]
fn get_playback_status(
    sup_state: State<SupervisorState>,
    audio_state: State<AudioState>,
) -> Result<PlaybackStatus, String> {
    let mut playback = audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o estado de reprodução: {}", e))?
        .get_status()?;
    let backend = sup_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o controle de volume: {}", e))?
        .volume_backend();
    apply_volume_backend(&mut playback, backend);
    Ok(playback)
}

#[tauri::command]
fn get_mpd_status_snapshot(
    sup_state: State<SupervisorState>,
    audio_state: State<AudioState>,
) -> Result<MpdStatusSnapshot, String> {
    collect_mpd_status_snapshot(&sup_state.0, &audio_state.0)
}

fn observe_mpd_process(supervisor: &Mutex<MpdSupervisor>) -> Result<MpdProcessObservation, String> {
    supervisor
        .lock()
        .map_err(|e| format!("Falha ao acessar a saúde do MPD: {}", e))?
        .observe_health()
}

fn unavailable_snapshot(health: MpdHealth) -> MpdStatusSnapshot {
    MpdStatusSnapshot {
        health,
        playback: None,
    }
}

fn apply_volume_backend(playback: &mut PlaybackStatus, backend: VolumeBackend) {
    apply_volume_backend_with(playback, backend, shared_volume::read_pipewire_volume);
}

fn apply_volume_backend_with<F>(
    playback: &mut PlaybackStatus,
    backend: VolumeBackend,
    read_pipewire: F,
) where
    F: FnOnce() -> Result<PipeWireVolume, String>,
{
    if backend != VolumeBackend::PipeWire {
        playback.volume.identify_backend(backend);
        return;
    }

    playback.volume = match read_pipewire() {
        Ok(volume) => VolumeStatus::pipewire(volume.value, volume.muted),
        Err(_) => VolumeStatus::pipewire_unavailable(),
    };
}

fn collect_mpd_status_snapshot(
    supervisor: &Mutex<MpdSupervisor>,
    audio: &Mutex<AudioEngine>,
) -> Result<MpdStatusSnapshot, String> {
    if let MpdProcessObservation::NotRunning(health) = observe_mpd_process(supervisor)? {
        return Ok(unavailable_snapshot(health));
    }

    let playback_result = audio
        .lock()
        .map_err(|e| format!("Falha ao acessar o cliente MPD: {}", e))?
        .get_status();

    match playback_result {
        Ok(mut playback) => {
            let (health, volume_backend) = {
                let mut supervisor = supervisor
                    .lock()
                    .map_err(|e| format!("Falha ao confirmar a saúde do MPD: {}", e))?;
                match supervisor.observe_health()? {
                    MpdProcessObservation::Running => {
                        let health = supervisor.mark_available();
                        (health, supervisor.volume_backend())
                    }
                    MpdProcessObservation::NotRunning(health) => {
                        return Ok(unavailable_snapshot(health));
                    }
                }
            };
            apply_volume_backend(&mut playback, volume_backend);
            Ok(MpdStatusSnapshot {
                health,
                playback: Some(playback),
            })
        }
        Err(_) => {
            if let MpdProcessObservation::NotRunning(health) = observe_mpd_process(supervisor)? {
                return Ok(unavailable_snapshot(health));
            }

            let reason = match audio
                .lock()
                .map_err(|e| format!("Falha ao verificar o protocolo MPD: {}", e))?
                .probe_mpd()
            {
                Err(MpdProbeFailure::SocketUnavailable) => {
                    MpdUnavailableReason::SocketUnavailable
                }
                Ok(()) | Err(MpdProbeFailure::ProtocolUnavailable) => {
                    MpdUnavailableReason::ProtocolUnavailable
                }
            };

            let health = {
                let mut supervisor = supervisor
                    .lock()
                    .map_err(|e| format!("Falha ao concluir a saúde do MPD: {}", e))?;
                match supervisor.observe_health()? {
                    MpdProcessObservation::Running => supervisor.mark_unavailable(reason),
                    MpdProcessObservation::NotRunning(health) => health,
                }
            };
            Ok(unavailable_snapshot(health))
        }
    }
}

#[tauri::command]
fn toggle_playback(state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().toggle_play_pause()
}

#[tauri::command]
fn next_track(state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().next()
}

#[tauri::command]
fn previous_track(state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().previous()
}

#[tauri::command]
fn seek_playback(seconds: f64, state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().seek(seconds)
}

#[tauri::command]
fn play_uris(uris: Vec<String>, start_index: usize, state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().play_uris(uris, start_index)
}

async fn resolve_playback_uris(
    tracks: &[TrackMetadata],
    plex_client: &PlexClient,
) -> Result<Vec<String>, String> {
    let mut uris = Vec::with_capacity(tracks.len());
    for track in tracks {
        let uri = if let Some(locator) = &track.media_locator {
            plex_client.resolve_media_locator(locator).await?
        } else {
            if track.uri.trim().is_empty() {
                return Err("Faixa sem referência de mídia válida.".to_string());
            }
            if AudioEngine::contains_plex_token(&track.uri) {
                return Err(
                    "URL Plex autenticada legada não pode ser reutilizada como identidade da faixa."
                        .to_string(),
                );
            }
            track.uri.clone()
        };
        uris.push(uri);
    }
    Ok(uris)
}

#[tauri::command]
async fn play_tracks(
    tracks: Vec<TrackMetadata>,
    start_index: usize,
    audio_state: State<'_, AudioState>,
    plex_state: State<'_, PlexState>,
) -> Result<(), String> {
    let plex_client = plex_state
        .0
        .lock()
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
        .clone();
    let playback_uris = resolve_playback_uris(&tracks, &plex_client).await?;
    audio_state
        .0
        .lock()
        .map_err(|_| "O estado de reprodução está indisponível.".to_string())?
        .play_tracks(tracks, playback_uris, start_index)
}

#[tauri::command]
fn set_volume(
    volume: u32,
    sup_state: State<SupervisorState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let backend = sup_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o controle de volume: {}", e))?
        .shared_volume_backend();
    set_volume_with_backend(
        backend,
        volume,
        shared_volume::set_pipewire_volume,
        |value| {
            audio_state
                .0
                .lock()
                .map_err(|e| format!("Falha ao acessar o volume do MPD: {}", e))?
                .set_volume(value)
        },
    )
}

fn set_volume_with_backend<P, M>(
    backend: Option<SharedVolumeBackend>,
    volume: u32,
    set_pipewire: P,
    set_mpd: M,
) -> Result<(), String>
where
    P: FnOnce(u32) -> Result<(), String>,
    M: FnOnce(u32) -> Result<(), String>,
{
    match backend {
        Some(SharedVolumeBackend::PipeWire) => set_pipewire(volume),
        Some(SharedVolumeBackend::MpdSoftware) | None => set_mpd(volume),
    }
}

#[tauri::command]
fn get_queue(state: State<AudioState>) -> Result<Vec<audio::TrackMetadata>, String> {
    Ok(state.0.lock().unwrap().get_queue())
}

#[tauri::command]
fn play_queue_index(index: usize, state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().play_index(index)
}

#[tauri::command]
fn clear_queue(state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().clear_queue()
}

#[tauri::command]
fn set_window_title(title: String, window: Window) -> Result<(), String> {
    window.set_title(&title).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_local_directory(
    path: String,
    state: State<AudioState>,
) -> Result<Vec<audio::LocalItem>, String> {
    state.0.lock().unwrap().list_directory(&path)
}

#[tauri::command]
fn get_favorites(config_state: State<ConfigState>) -> Result<Vec<FavoriteAlbum>, String> {
    let server_id = config_state
        .0
        .lock()
        .map_err(|_| "O estado da configuração está indisponível.".to_string())?
        .plex_server_id
        .clone();
    FavoriteAlbum::load_all(server_id.as_deref())
}

#[tauri::command]
fn toggle_favorite(
    album: FavoriteAlbum,
    config_state: State<ConfigState>,
) -> Result<bool, String> {
    let server_id = config_state
        .0
        .lock()
        .map_err(|_| "O estado da configuração está indisponível.".to_string())?
        .plex_server_id
        .clone();
    FavoriteAlbum::toggle(album, server_id.as_deref())
}

#[tauri::command]
fn get_local_cover(path: String) -> Option<String> {
    let lib_dir = supervisor::MpdSupervisor::library_dir();
    let p = std::path::Path::new(&path);
    let full_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        lib_dir.join(p)
    };

    if full_path.is_dir() {
        audio::find_folder_cover_path(&full_path)
    } else if let Some(parent) = full_path.parent() {
        audio::find_folder_cover_path(parent)
    } else {
        None
    }
}

#[tauri::command]
fn pick_directory() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Selecionar Pasta de Músicas")
        .pick_folder()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn rescan_library(state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().rescan_library()
}

#[tauri::command]
fn get_config(state: State<ConfigState>) -> Result<AppConfig, String> {
    Ok(state.0.lock().unwrap().clone())
}

#[tauri::command]
fn get_audio_devices() -> Vec<AudioDevice> {
    list_audio_devices()
}

#[tauri::command]
async fn plex_create_pin() -> Result<plex::PlexPin, String> {
    plex::request_plex_pin().await
}

#[tauri::command]
async fn plex_check_pin(pin_id: u64) -> Result<Option<String>, String> {
    plex::check_plex_pin(pin_id).await
}

#[tauri::command]
async fn plex_get_servers(auth_token: String) -> Result<Vec<plex::PlexServerResource>, String> {
    plex::get_plex_servers(&auth_token).await
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("Falha ao abrir navegador: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn save_config(
    new_config: AppConfig,
    app: AppHandle,
    config_state: State<'_, ConfigState>,
    transaction_state: State<'_, ConfigTransactionState>,
    plex_state: State<'_, PlexState>,
    sup_state: State<'_, SupervisorState>,
    audio_state: State<'_, AudioState>,
    analyzer_state: State<'_, AnalyzerState>,
) -> Result<(), String> {
    let observed_cfg = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a configuração atual: {}", e))?
        .clone();

    let output_change_expected = observed_cfg.audio_output_type != new_config.audio_output_type
        || observed_cfg.alsa_device != new_config.alsa_device
        || observed_cfg.dop_enabled != new_config.dop_enabled
        || observed_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
        || observed_cfg.replay_gain != new_config.replay_gain;
    let playback_uris = if output_change_expected {
        let queue = audio_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar a fila antes da troca de saída: {}", e))?
            .get_queue();
        let plex_client = plex_state
            .0
            .lock()
            .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
            .clone();
        resolve_playback_uris(&queue, &plex_client).await?
    } else {
        Vec::new()
    };

    let _transaction_guard = transaction_state.begin()?;
    let current_cfg = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a configuração atual: {}", e))?
        .clone();
    if current_cfg != observed_cfg {
        return Err(
            "A configuração mudou durante a preparação da troca de saída. Tente novamente."
                .to_string(),
        );
    }

    let audio_hw_changed = current_cfg.audio_output_type != new_config.audio_output_type
        || current_cfg.alsa_device != new_config.alsa_device
        || current_cfg.dop_enabled != new_config.dop_enabled
        || current_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
        || current_cfg.replay_gain != new_config.replay_gain;

    let folders_changed = current_cfg.local_folders != new_config.local_folders;
    let mut playback_snapshot = None;
    let mut resume_analyzer = false;

    if audio_hw_changed {
        // 1. Captura o estado e segundo atual da música sem destruir a fila
        let preparation = audio_state
            .0
            .lock()
            .map_err(|e| {
                format!(
                    "Falha ao preparar troca de saída: não foi possível acessar o estado de reprodução: {}",
                    e
                )
            })?
            .prepare_device_switch();
        playback_snapshot = match preparation {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if error.snapshot.is_some() {
                    let rollback = restore_prepared_playback(
                        error.snapshot,
                        &playback_uris,
                        &audio_state,
                    );
                    return Err(error_with_rollback(
                        format!("Falha ao preparar troca de saída: {}", error.cause),
                        rollback,
                    ));
                }
                return Err(format!(
                    "Falha ao preparar troca de saída: {}",
                    error.cause
                ));
            }
        };
        resume_analyzer = {
            let mut analyzer = analyzer_state
                .0
                .lock()
                .map_err(|e| format!("Falha ao acessar o analyzer antes da troca: {}", e))?;
            let was_active = analyzer.is_active();
            if was_active {
                if let Err(error) = analyzer.stop(Some(&app)) {
                    eprintln!(
                        "[Analyzer] Falha ao suspender analyzer antes da troca de saída: {}",
                        error
                    );
                }
            }
            was_active
        };
        // 2. Reinicia o MPD com a nova saída (liberando o ALSA)
        let switch_result = match sup_state.0.lock() {
            Ok(mut supervisor) => supervisor.start(&new_config),
            Err(e) => {
                let original = format!(
                    "Falha ao iniciar nova saída: não foi possível acessar o supervisor do MPD: {}",
                    e
                );
                let rollback = restore_prepared_playback(
                    playback_snapshot,
                    &playback_uris,
                    &audio_state,
                );
                return Err(error_with_rollback(original, rollback));
            }
        };
        if let Err(switch_error) = switch_result {
            let rollback =
                rollback_audio_switch(
                    &current_cfg,
                    playback_snapshot,
                    &playback_uris,
                    &sup_state,
                    &audio_state,
                );
            return Err(error_with_rollback(
                format!("Falha ao iniciar nova saída: {}", switch_error),
                rollback,
            ));
        }

        // 3. Restaura fila/faixa/posição quando possível; Playing/Paused terminam pausados e Stopped permanece parado
        let restore_result = match audio_state.0.lock() {
            Ok(audio) => audio.restore_after_device_switch(playback_snapshot, &playback_uris),
            Err(e) => Err(format!(
                "não foi possível acessar a fila após trocar a saída: {}",
                e
            )),
        };
        if let Err(restore_error) = restore_result {
            let rollback =
                rollback_audio_switch(
                    &current_cfg,
                    playback_snapshot,
                    &playback_uris,
                    &sup_state,
                    &audio_state,
                );
            return Err(error_with_rollback(
                format!("Falha ao restaurar reprodução: {}", restore_error),
                rollback,
            ));
        }
    }

    if folders_changed {
        let update_library = || -> Result<(), String> {
            MpdSupervisor::sync_library_symlinks(&new_config.local_folders)?;
            let mut audio = audio_state
                .0
                .lock()
                .map_err(|e| format!("Falha ao acessar a biblioteca local: {}", e))?;
            audio.set_music_dir(&MpdSupervisor::library_dir().to_string_lossy());
            audio.rescan_library()
        };
        if let Err(library_error) = update_library() {
            let rollback = rollback_applied_config_change(
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                &playback_uris,
                &sup_state,
                &audio_state,
            );
            return Err(error_with_rollback(
                format!("Falha ao atualizar a biblioteca local: {}", library_error),
                rollback,
            ));
        }
    }

    let mut config_guard = match config_state.0.lock() {
        Ok(guard) => guard,
        Err(e) => {
            let original = format!("Falha ao publicar a nova configuração: {}", e);
            let rollback = rollback_applied_config_change(
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                &playback_uris,
                &sup_state,
                &audio_state,
            );
            return Err(error_with_rollback(original, rollback));
        }
    };
    let mut plex_guard = match plex_state.0.lock() {
        Ok(guard) => guard,
        Err(e) => {
            drop(config_guard);
            let original = format!("Falha ao atualizar a configuração do Plex: {}", e);
            let rollback = rollback_applied_config_change(
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                &playback_uris,
                &sup_state,
                &audio_state,
            );
            return Err(error_with_rollback(original, rollback));
        }
    };

    if let Err(save_error) = new_config.save() {
        drop(plex_guard);
        drop(config_guard);
        let rollback = rollback_applied_config_change(
            audio_hw_changed,
            folders_changed,
            &current_cfg,
            playback_snapshot,
            &playback_uris,
            &sup_state,
            &audio_state,
        );
        return Err(error_with_rollback(
            format!("Falha ao persistir configuração: {}", save_error),
            rollback,
        ));
    }

    plex_guard.update_config(&new_config);
    *config_guard = new_config;
    drop(plex_guard);
    drop(config_guard);

    if resume_analyzer {
        if let Ok(mut analyzer) = analyzer_state.0.lock() {
            if let Err(error) = analyzer.start(app) {
                eprintln!(
                    "[Analyzer] Configuração aplicada, mas o analyzer não pôde ser retomado: {}",
                    error
                );
            }
        }
    }
    Ok(())
}

fn error_with_rollback(original_error: String, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => format!("{}. Estado anterior restaurado.", original_error),
        Err(rollback_error) => format!(
            "{}. Rollback também falhou: {}",
            original_error, rollback_error
        ),
    }
}

fn restore_prepared_playback(
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    playback_uris: &[String],
    audio_state: &State<'_, AudioState>,
) -> Result<(), String> {
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
        .restore_after_device_switch(playback_snapshot, playback_uris)
}

fn rollback_audio_switch(
    previous_config: &AppConfig,
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    playback_uris: &[String],
    sup_state: &State<'_, SupervisorState>,
    audio_state: &State<'_, AudioState>,
) -> Result<(), String> {
    sup_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o supervisor durante rollback: {}", e))?
        .start(previous_config)?;
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
        .restore_after_device_switch(playback_snapshot, playback_uris)
}

fn rollback_applied_config_change(
    audio_hw_changed: bool,
    folders_changed: bool,
    previous_config: &AppConfig,
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    playback_uris: &[String],
    sup_state: &State<'_, SupervisorState>,
    audio_state: &State<'_, AudioState>,
) -> Result<(), String> {
    let mut failures = Vec::new();
    if folders_changed {
        let library_rollback = MpdSupervisor::sync_library_symlinks(&previous_config.local_folders)
            .and_then(|_| {
                audio_state
                    .0
                    .lock()
                    .map_err(|e| format!("Falha ao acessar a biblioteca durante rollback: {}", e))?
                    .rescan_library()
            });
        if let Err(error) = library_rollback {
            failures.push(format!("biblioteca: {}", error));
        }
    }

    if audio_hw_changed {
        if let Err(error) =
            rollback_audio_switch(
                previous_config,
                playback_snapshot,
                playback_uris,
                sup_state,
                audio_state,
            )
        {
            failures.push(format!("saída de áudio: {}", error));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[tauri::command]
fn get_local_albums(state: State<AudioState>) -> Result<Vec<audio::LocalAlbum>, String> {
    state.0.lock().unwrap().get_local_albums()
}

#[tauri::command]
async fn get_plex_libraries(state: State<'_, PlexState>) -> Result<Vec<PlexLibrary>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_music_libraries().await
}

#[tauri::command]
async fn get_plex_albums(
    section_key: String,
    sort_by: String,
    state: State<'_, PlexState>,
) -> Result<Vec<PlexAlbum>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_albums(&section_key, &sort_by).await
}

#[tauri::command]
async fn get_plex_collections(
    section_key: String,
    state: State<'_, PlexState>,
) -> Result<Vec<PlexCollection>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_collections(&section_key).await
}

#[tauri::command]
async fn get_collection_albums(
    rating_key: String,
    state: State<'_, PlexState>,
) -> Result<Vec<PlexAlbum>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_collection_albums(&rating_key).await
}

#[tauri::command]
async fn get_artist_albums(
    rating_key: String,
    state: State<'_, PlexState>,
) -> Result<Vec<PlexAlbum>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_artist_albums(&rating_key).await
}

#[tauri::command]
async fn get_artist_top_tracks(
    rating_key: String,
    state: State<'_, PlexState>,
) -> Result<Vec<PlexTrack>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_artist_top_tracks(&rating_key).await
}

#[tauri::command]
async fn get_album_tracks(
    rating_key: String,
    state: State<'_, PlexState>,
) -> Result<Vec<PlexTrack>, String> {
    let client = state.0.lock().unwrap().clone();
    client.get_album_tracks(&rating_key).await
}

#[tauri::command]
async fn search_plex(
    query: String,
    section_key: Option<String>,
    state: State<'_, PlexState>,
) -> Result<PlexSearchResults, String> {
    let client = state.0.lock().unwrap().clone();
    client.search(&query, section_key.as_deref()).await
}

#[tauri::command]
async fn get_plex_image(
    image: PlexImageRef,
    state: State<'_, PlexState>,
) -> Result<tauri::ipc::Response, String> {
    let client = state
        .0
        .lock()
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
        .clone();
    let bytes = client.get_image(&image).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().to_string();
    let initial_config = AppConfig::load();

    let mut supervisor = MpdSupervisor::new(&socket_path);
    if let Err(e) = supervisor.start(&initial_config) {
        eprintln!("[Aviso] Erro no supervisor de áudio: {}", e);
    }

    let audio_engine = AudioEngine::new(
        &socket_path,
        &MpdSupervisor::library_dir().to_string_lossy(),
    );
    let audio_analyzer = AudioAnalyzer::new(
        socket_path.clone(),
        MpdSupervisor::analyzer_fifo_path(),
    );
    if let Err(e) = audio_engine.rescan_library() {
        eprintln!(
            "[Audio] Falha ao solicitar rescan inicial da biblioteca: {}",
            e
        );
    }

    let plex_client = PlexClient::from_config(&initial_config);

    let app = tauri::Builder::default()
        .manage(AudioState(Mutex::new(audio_engine)))
        .manage(AnalyzerState(Mutex::new(audio_analyzer)))
        .manage(PlexState(Mutex::new(plex_client)))
        .manage(SupervisorState(Mutex::new(supervisor)))
        .manage(ConfigState(Mutex::new(initial_config)))
        .manage(ConfigTransactionState(Mutex::new(())))
        .invoke_handler(tauri::generate_handler![
            get_playback_status,
            get_mpd_status_snapshot,
            start_audio_analyzer,
            stop_audio_analyzer,
            toggle_playback,
            next_track,
            previous_track,
            seek_playback,
            play_uris,
            play_tracks,
            set_volume,
            get_queue,
            play_queue_index,
            clear_queue,
            set_window_title,
            list_local_directory,
            get_local_cover,
            get_local_albums,
            get_favorites,
            toggle_favorite,
            pick_directory,
            rescan_library,
            get_config,
            save_config,
            get_audio_devices,
            get_plex_libraries,
            get_plex_albums,
            get_plex_collections,
            get_collection_albums,
            get_artist_albums,
            get_artist_top_tracks,
            get_album_tracks,
            search_plex,
            get_plex_image,
            plex_create_pin,
            plex_check_pin,
            plex_get_servers,
            open_external_url,
        ])
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                if window.label() == "now-playing" {
                    if let Some(state) = window.app_handle().try_state::<AnalyzerState>() {
                        if let Ok(mut analyzer) = state.0.lock() {
                            if let Err(error) = analyzer.stop(Some(window.app_handle())) {
                                eprintln!("[Analyzer] Falha no cleanup da janela: {}", error);
                            }
                        }
                    }
                } else if should_exit_application_on_window_close(window.label()) {
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Erro ao compilar o contexto do Tauri");

    app.run(|app_handle, event| {
        match event {
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
                if let Some(state) = app_handle.try_state::<AnalyzerState>() {
                    if let Ok(mut analyzer) = state.0.lock() {
                        if let Err(error) = analyzer.stop(Some(app_handle)) {
                            eprintln!("[Analyzer] Falha no shutdown da aplicação: {}", error);
                        }
                    }
                }
                if let Some(sup_state) = app_handle.try_state::<SupervisorState>() {
                    if let Ok(mut supervisor) = sup_state.0.lock() {
                        if let Err(e) = supervisor.stop() {
                            eprintln!("[Supervisor] Falha no shutdown da aplicação: {}", e);
                        }
                    }
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    static NEXT_MPD_HEALTH_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn main_window_close_requests_application_exit() {
        assert!(should_exit_application_on_window_close("main"));
    }

    #[test]
    fn secondary_window_close_does_not_request_application_exit() {
        assert!(!should_exit_application_on_window_close("now-playing"));
    }

    fn playback_with_mpd_volume(value: i32) -> PlaybackStatus {
        PlaybackStatus {
            state: "stop".to_string(),
            elapsed: 0.0,
            duration: 0.0,
            audio_format: String::new(),
            current_media: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            thumb: None,
            plex_image: None,
            volume: VolumeStatus {
                value: value.clamp(0, 100) as u32,
                muted: value == 0,
                writable: true,
                available: true,
                backend: audio::VolumeBackend::MpdSoftware,
            },
            is_updating: false,
        }
    }

    #[test]
    fn pipewire_volume_write_never_calls_mpd_setvol() {
        let pipewire_calls = Cell::new(0);
        let mpd_calls = Cell::new(0);

        set_volume_with_backend(
            Some(SharedVolumeBackend::PipeWire),
            42,
            |value| {
                assert_eq!(value, 42);
                pipewire_calls.set(pipewire_calls.get() + 1);
                Ok(())
            },
            |_| {
                mpd_calls.set(mpd_calls.get() + 1);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(pipewire_calls.get(), 1);
        assert_eq!(mpd_calls.get(), 0);
    }

    #[test]
    fn software_and_direct_volume_writes_keep_using_mpd() {
        for backend in [Some(SharedVolumeBackend::MpdSoftware), None] {
            let mpd_calls = Cell::new(0);
            set_volume_with_backend(
                backend,
                58,
                |_| panic!("PipeWire não deve ser usado neste backend"),
                |value| {
                    assert_eq!(value, 58);
                    mpd_calls.set(mpd_calls.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(mpd_calls.get(), 1);
        }
    }

    #[test]
    fn temporary_pipewire_failure_does_not_fall_back_to_mpd() {
        let mpd_calls = Cell::new(0);
        let result = set_volume_with_backend(
            Some(SharedVolumeBackend::PipeWire),
            30,
            |_| Err("temporariamente indisponível".to_string()),
            |_| {
                mpd_calls.set(mpd_calls.get() + 1);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert_eq!(mpd_calls.get(), 0);

        let mut playback = playback_with_mpd_volume(75);
        apply_volume_backend_with(
            &mut playback,
            VolumeBackend::PipeWire,
            || Err("temporariamente indisponível".to_string()),
        );
        assert_eq!(playback.volume.backend, VolumeBackend::Unavailable);
        assert!(!playback.volume.available);
        assert!(!playback.volume.writable);
    }

    #[test]
    fn pipewire_read_replaces_mpd_volume_in_polled_status() {
        let mut playback = playback_with_mpd_volume(75);
        apply_volume_backend_with(
            &mut playback,
            VolumeBackend::PipeWire,
            || {
                Ok(PipeWireVolume {
                    value: 41,
                    muted: true,
                })
            },
        );

        assert_eq!(playback.volume.value, 41);
        assert!(playback.volume.muted);
        assert!(playback.volume.available);
        assert!(playback.volume.writable);
        assert_eq!(playback.volume.backend, audio::VolumeBackend::PipeWire);
    }

    #[test]
    fn mpd_volume_is_labeled_with_the_effective_mixer_without_changing_its_value() {
        for backend in [VolumeBackend::AlsaHardware, VolumeBackend::MpdSoftware] {
            let mut playback = playback_with_mpd_volume(63);
            apply_volume_backend_with(&mut playback, backend, || {
                panic!("Leitura PipeWire não deve ocorrer para volume controlado pelo MPD")
            });

            assert_eq!(playback.volume.value, 63);
            assert!(playback.volume.available);
            assert!(playback.volume.writable);
            assert_eq!(playback.volume.backend, backend);
        }
    }

    fn mpd_health_test_paths(test_name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let id = NEXT_MPD_HEALTH_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("mpd-health-tests")
            .join(format!("{}-{}-{}", std::process::id(), test_name, id));
        fs::create_dir_all(&dir).unwrap();
        let socket_path = dir.join("mpd.socket");
        let pid_path = dir.join("mpd.pid");
        (dir, socket_path, pid_path)
    }

    fn sleeping_child() -> Child {
        Command::new("sleep").arg("30").spawn().unwrap()
    }

    fn test_supervisor(socket_path: &Path, pid_path: &Path, child: Child) -> MpdSupervisor {
        let mut supervisor = MpdSupervisor::new_with_runtime_paths(socket_path, pid_path);
        supervisor.set_process_for_test(child, false);
        supervisor
    }

    fn spawn_status_server(socket_path: &Path) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(socket_path).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.23.15\n").unwrap();
            stream.flush().unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command, "status\n");
            stream
                .write_all(b"volume: 75\nstate: stop\nOK\n")
                .unwrap();
            stream.flush().unwrap();
        })
    }

    fn spawn_invalid_protocol_server(socket_path: &Path) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(socket_path).unwrap();
        thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"NOT MPD\n").unwrap();
                stream.flush().unwrap();
            }
        })
    }

    #[test]
    fn mpd_status_snapshot_is_available_only_with_valid_mpd_communication() {
        let (dir, socket_path, pid_path) = mpd_health_test_paths("available");
        let server = spawn_status_server(&socket_path);
        let supervisor = Mutex::new(test_supervisor(&socket_path, &pid_path, sleeping_child()));
        let audio = Mutex::new(AudioEngine::new(
            &socket_path.to_string_lossy(),
            &dir.to_string_lossy(),
        ));

        let snapshot = collect_mpd_status_snapshot(&supervisor, &audio).unwrap();
        assert_eq!(snapshot.health, MpdHealth::Available);
        assert_eq!(
            snapshot
                .playback
                .as_ref()
                .map(|status| status.state.as_str()),
            Some("stop")
        );

        server.join().unwrap();
        supervisor.lock().unwrap().stop().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mpd_status_snapshot_reports_process_exit_without_playback() {
        let (dir, socket_path, pid_path) = mpd_health_test_paths("process-exited");
        let mut child = Command::new("sh").arg("-c").arg("exit 9").spawn().unwrap();
        child.wait().unwrap();
        let supervisor = Mutex::new(test_supervisor(&socket_path, &pid_path, child));
        let audio = Mutex::new(AudioEngine::new(
            &socket_path.to_string_lossy(),
            &dir.to_string_lossy(),
        ));

        let snapshot = collect_mpd_status_snapshot(&supervisor, &audio).unwrap();
        assert_eq!(
            snapshot.health,
            MpdHealth::Unavailable(MpdUnavailableReason::ProcessExited)
        );
        assert!(snapshot.playback.is_none());
        assert!(!supervisor.lock().unwrap().has_process_for_test());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mpd_status_snapshot_distinguishes_socket_unavailable() {
        let (dir, socket_path, pid_path) = mpd_health_test_paths("socket-unavailable");
        let supervisor = Mutex::new(test_supervisor(&socket_path, &pid_path, sleeping_child()));
        let audio = Mutex::new(AudioEngine::new(
            &socket_path.to_string_lossy(),
            &dir.to_string_lossy(),
        ));

        let snapshot = collect_mpd_status_snapshot(&supervisor, &audio).unwrap();
        assert_eq!(
            snapshot.health,
            MpdHealth::Unavailable(MpdUnavailableReason::SocketUnavailable)
        );
        assert!(snapshot.playback.is_none());

        supervisor.lock().unwrap().stop().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mpd_status_snapshot_distinguishes_protocol_unavailable() {
        let (dir, socket_path, pid_path) = mpd_health_test_paths("protocol-unavailable");
        let server = spawn_invalid_protocol_server(&socket_path);
        let supervisor = Mutex::new(test_supervisor(&socket_path, &pid_path, sleeping_child()));
        let audio = Mutex::new(AudioEngine::new(
            &socket_path.to_string_lossy(),
            &dir.to_string_lossy(),
        ));

        let snapshot = collect_mpd_status_snapshot(&supervisor, &audio).unwrap();
        assert_eq!(
            snapshot.health,
            MpdHealth::Unavailable(MpdUnavailableReason::ProtocolUnavailable)
        );
        assert!(snapshot.playback.is_none());

        server.join().unwrap();
        supervisor.lock().unwrap().stop().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn config_transaction_state_serializes_concurrent_changes() {
        let state = Arc::new(ConfigTransactionState(Mutex::new(())));
        let first_guard = state.begin().unwrap();
        let worker_state = Arc::clone(&state);
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            let _guard = worker_state.begin().unwrap();
            acquired_tx.send(()).unwrap();
        });

        attempt_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(first_guard);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn transaction_error_preserves_original_and_rollback_failures() {
        let error = error_with_rollback(
            "falha original".to_string(),
            Err("falha no rollback".to_string()),
        );
        assert!(error.contains("falha original"));
        assert!(error.contains("falha no rollback"));
        assert!(error.contains("Rollback também falhou"));

        let restored = error_with_rollback("falha original".to_string(), Ok(()));
        assert!(restored.contains("falha original"));
        assert!(restored.contains("Estado anterior restaurado"));
    }
}
