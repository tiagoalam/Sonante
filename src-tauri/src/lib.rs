mod audio;
mod config;
mod favorites;
mod plex;
mod supervisor;

use audio::{list_audio_devices, AudioDevice, AudioEngine, AudioState, PlaybackStatus, TrackMetadata};
use config::AppConfig;
use favorites::FavoriteAlbum;
use plex::{PlexAlbum, PlexClient, PlexCollection, PlexLibrary, PlexSearchResults, PlexTrack};
use supervisor::MpdSupervisor;
use std::sync::Mutex;
use tauri::{Manager, RunEvent, State, Window, WindowEvent};

pub struct PlexState(pub Mutex<PlexClient>);
pub struct SupervisorState(pub Mutex<MpdSupervisor>);
pub struct ConfigState(pub Mutex<AppConfig>);

#[tauri::command]
fn get_playback_status(state: State<AudioState>) -> Result<PlaybackStatus, String> {
    state.0.lock().unwrap().get_status()
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

#[tauri::command]
fn play_tracks(
    tracks: Vec<TrackMetadata>,
    start_index: usize,
    state: State<AudioState>,
) -> Result<(), String> {
    state.0.lock().unwrap().play_tracks(tracks, start_index)
}

#[tauri::command]
fn set_volume(volume: u32, state: State<AudioState>) -> Result<(), String> {
    state.0.lock().unwrap().set_volume(volume)
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
fn get_favorites() -> Vec<FavoriteAlbum> {
    FavoriteAlbum::load_all()
}

#[tauri::command]
fn toggle_favorite(album: FavoriteAlbum) -> Result<bool, String> {
    FavoriteAlbum::toggle(album)
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
fn save_config(
    new_config: AppConfig,
    config_state: State<ConfigState>,
    plex_state: State<PlexState>,
    sup_state: State<SupervisorState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let mut current_cfg = config_state.0.lock().unwrap();

    let audio_hw_changed = current_cfg.audio_output_type != new_config.audio_output_type
        || current_cfg.alsa_device != new_config.alsa_device
        || current_cfg.dop_enabled != new_config.dop_enabled
        || current_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
        || current_cfg.replay_gain != new_config.replay_gain;

    let folders_changed = current_cfg.local_folders != new_config.local_folders;

    new_config.save()?;
    plex_state.0.lock().unwrap().update_config(&new_config);

    if folders_changed {
        let _ = MpdSupervisor::sync_library_symlinks(&new_config.local_folders);
        audio_state.0.lock().unwrap().set_music_dir(&MpdSupervisor::library_dir().to_string_lossy());
        let _ = audio_state.0.lock().unwrap().rescan_library();
    }

    if audio_hw_changed {
        // 1. Captura o estado e segundo atual da música sem destruir a fila
        let playback_snapshot = if let Ok(mut audio) = audio_state.0.lock() {
            audio.prepare_device_switch()
        } else {
            None
        };

        // 2. Reinicia o MPD com a nova saída (liberando o ALSA)
        if let Ok(mut supervisor) = sup_state.0.lock() {
            supervisor.stop();
            let _ = supervisor.start(&new_config);
        }

        // 3. Reinsere a fila intacta e retoma a reprodução exatamente no mesmo ponto
        if let Ok(mut audio) = audio_state.0.lock() {
            let _ = audio.restore_after_device_switch(playback_snapshot);
        }
    }

    *current_cfg = new_config;
    Ok(())
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
    let _ = audio_engine.rescan_library();

    let plex_client = PlexClient::from_config(&initial_config);

    let app = tauri::Builder::default()
        .manage(AudioState(Mutex::new(audio_engine)))
        .manage(PlexState(Mutex::new(plex_client)))
        .manage(SupervisorState(Mutex::new(supervisor)))
        .manage(ConfigState(Mutex::new(initial_config)))
        .invoke_handler(tauri::generate_handler![
            get_playback_status,
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
            plex_create_pin,
            plex_check_pin,
            plex_get_servers,
            open_external_url,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                if let Some(sup_state) = window.app_handle().try_state::<SupervisorState>() {
                    if let Ok(mut sup) = sup_state.0.lock() {
                        if let Err(e) = sup.stop() {
                            eprintln!("[Supervisor] Falha no shutdown da janela: {}", e);
                        }
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Erro ao compilar o contexto do Tauri");

    app.run(|app_handle, event| {
        match event {
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
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
