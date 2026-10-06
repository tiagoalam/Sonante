mod alsa_mixer;
mod analyzer;
mod artwork_progress;
mod audio;
mod config;
mod favorites;
mod mpris;
mod online_artwork;
mod persistence;
mod playlists;
mod plex;
mod shared_volume;
mod supervisor;

use audio::{
    list_audio_devices, AudioDevice, AudioEngine, AudioState, DeviceSwitchSnapshot,
    MediaLocator, MpdProbeFailure, PlaybackStatus, TrackMetadata, VolumeBackend, VolumeStatus,
};
use analyzer::{AnalyzerState, AudioAnalyzer};
use config::AppConfig;
use favorites::FavoriteAlbum;
use playlists::{NewPlaylistItem, Playlist, PlaylistState, PlaylistStore};
use plex::{
    PlexAlbum, PlexClient, PlexCollection, PlexImageRef, PlexLibrary, PlexMediaAvailability,
    PlexSearchResults, PlexTrack,
};
use serde::Serialize;
use shared_volume::{PipeWireVolume, SharedVolumeBackend};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use supervisor::{MpdHealth, MpdProcessObservation, MpdSupervisor, MpdUnavailableReason};
use std::sync::{atomic::{AtomicBool, Ordering}, Mutex};
use tauri::{AppHandle, Manager, RunEvent, State, Window, WindowEvent};

pub struct PlexState(pub Mutex<PlexClient>);
pub struct SupervisorState(pub Mutex<MpdSupervisor>);
pub struct ConfigState(pub Mutex<AppConfig>);
pub struct ConfigTransactionState(pub Mutex<()>);

const MAIN_WINDOW_LABEL: &str = "main";

fn should_start_mpd_on_startup(config_valid: bool, config: &AppConfig) -> bool {
    config_valid && !config.first_run
}

fn is_shared_output(config: &AppConfig) -> bool {
    config.audio_output_type == "pipewire"
        || config.audio_output_type == "shared"
        || config.alsa_device == "default"
}

fn should_rescan_library_on_startup(
    config_valid: bool,
    config: &AppConfig,
    database_exists: bool,
) -> bool {
    should_start_mpd_on_startup(config_valid, config)
        && !config.local_folders.is_empty()
        && !database_exists
}

fn local_folders_changed(current: &AppConfig, next: &AppConfig) -> bool {
    current.local_folders != next.local_folders
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryRefresh {
    Update,
    Rescan,
}

fn library_refresh(
    changed: bool,
    database_exists: bool,
    has_folders: bool,
) -> Option<LibraryRefresh> {
    if !changed {
        None
    } else if database_exists {
        Some(LibraryRefresh::Update)
    } else if has_folders {
        Some(LibraryRefresh::Rescan)
    } else {
        None
    }
}

fn library_refresh_after_sync(
    sync_changed: bool,
    start_changed: bool,
    finishing_first_run: bool,
    database_exists: bool,
    has_folders: bool,
) -> Option<LibraryRefresh> {
    library_refresh(
        sync_changed || start_changed || (finishing_first_run && !database_exists && has_folders),
        database_exists,
        has_folders,
    )
}

fn database_exists() -> Result<bool, String> {
    MpdSupervisor::database_path()
        .try_exists()
        .map_err(|e| format!("Falha ao verificar database MPD: {e}"))
}

fn resolve_local_library_path_in(
    library_dir: &Path,
    relative_path: &str,
    local_folders: &[String],
) -> Result<PathBuf, String> {
    let path = Path::new(relative_path);
    let mut components = path.components();
    let first = match components.next() {
        Some(Component::Normal(name)) => name,
        _ => return Err("Caminho local relativo inválido".into()),
    };
    if components.any(|component| !matches!(component, Component::Normal(_))) {
        return Err("Caminho local relativo inválido".into());
    }
    if !std::fs::symlink_metadata(library_dir)
        .map_err(|e| format!("Biblioteca local indisponível: {e}"))?
        .file_type()
        .is_dir()
    {
        return Err("Biblioteca local inválida".into());
    }

    let roots = local_folders
        .iter()
        .filter(|folder| Path::new(folder).is_dir())
        .map(|folder| {
            std::fs::canonicalize(folder)
                .map_err(|e| format!("Falha ao resolver pasta local configurada: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let link = library_dir.join(first);
    if !std::fs::symlink_metadata(&link)
        .map_err(|e| format!("Entrada da biblioteca local indisponível: {e}"))?
        .file_type()
        .is_symlink()
    {
        return Err("Entrada da biblioteca local inválida".into());
    }
    let link_target = std::fs::canonicalize(&link)
        .map_err(|e| format!("Symlink da biblioteca local indisponível: {e}"))?;
    if !roots.contains(&link_target) {
        return Err("Entrada fora das pastas locais configuradas".into());
    }

    let resolved = std::fs::canonicalize(library_dir.join(path))
        .map_err(|e| format!("Caminho local indisponível: {e}"))?;
    if !resolved.is_dir() || !roots.iter().any(|root| resolved.starts_with(root)) {
        return Err("Caminho fora das pastas locais configuradas".into());
    }
    Ok(resolved)
}

fn request_library_refresh(socket_path: &str, refresh: LibraryRefresh) -> Result<(), String> {
    match refresh {
        LibraryRefresh::Update => AudioEngine::update_library(socket_path),
        LibraryRefresh::Rescan => AudioEngine::rescan_library_at(socket_path),
    }
}

fn is_finishing_first_run(current_config: &AppConfig, new_config: &AppConfig) -> bool {
    current_config.first_run && !new_config.first_run
}

fn should_prepare_device_switch(audio_hw_changed: bool, finishing_first_run: bool) -> bool {
    audio_hw_changed && !finishing_first_run
}

fn should_exit_application_on_window_close(window_label: &str) -> bool {
    window_label == MAIN_WINDOW_LABEL
}

#[tauri::command]
fn start_audio_analyzer(
    session_id: String,
    window: Window,
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    if window.label() != "now-playing" || app.get_webview_window("now-playing").is_none() {
        return Err("A janela Now Playing não está disponível para iniciar o analyzer.".into());
    }
    let mut analyzer = analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?;
    if app.get_webview_window("now-playing").is_none() {
        return Err("A janela Now Playing foi fechada antes de iniciar o analyzer.".into());
    }
    analyzer.start_for_window(session_id, app)
}

#[tauri::command]
fn stop_audio_analyzer(
    session_id: String,
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?
        .stop_for_window(&session_id, &app)
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
fn next_track(
    config_state: State<ConfigState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante Next: {}", e))?;
        is_shared_output(&config)
    };
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar reprodução durante Next: {}", e))?
        .next(is_shared)
}

#[tauri::command]
fn previous_track(
    config_state: State<ConfigState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante Previous: {}", e))?;
        is_shared_output(&config)
    };
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar reprodução durante Previous: {}", e))?
        .previous(is_shared)
}

#[tauri::command]
fn seek_playback(
    seconds: f64,
    config_state: State<ConfigState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante seek: {}", e))?;
        is_shared_output(&config)
    };
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar reprodução durante seek: {}", e))?
        .seek(seconds, is_shared)
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

async fn restore_cached_queue_after_ready(app: AppHandle) {
    let audio_state = app.state::<AudioState>();
    let queue = match audio_state.0.lock() {
        Ok(audio) if audio.startup_queue_pending() => audio.get_queue(),
        Ok(_) => return,
        Err(_) => {
            eprintln!("[Audio] Falha ao acessar a fila para restauração no startup.");
            return;
        }
    };
    let plex_client = match app.state::<PlexState>().0.lock() {
        Ok(plex) => plex.clone(),
        Err(_) => {
            eprintln!("[Audio] Falha ao acessar Plex para restauração no startup.");
            if let Ok(mut audio) = audio_state.0.lock() {
                audio.fail_startup_queue_restore();
            }
            return;
        }
    };
    match resolve_playback_uris(&queue, &plex_client).await {
        Ok(uris) => match audio_state.0.lock() {
            Ok(mut audio) => {
                if let Err(error) = audio.restore_startup_queue(&uris) {
                    eprintln!("[Audio] Falha ao restaurar fila no MPD: {}", error);
                }
            }
            Err(_) => eprintln!("[Audio] Falha ao acessar o MPD para restauração no startup."),
        },
        Err(_) => {
            eprintln!("[Audio] Falha ao resolver mídia para restauração no startup.");
            if let Ok(mut audio) = audio_state.0.lock() {
                audio.fail_startup_queue_restore();
            }
        }
    }
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
async fn resolve_local_library_path(
    path: String,
    config_state: State<'_, ConfigState>,
) -> Result<String, String> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar pastas locais: {e}"))?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    tauri::async_runtime::spawn_blocking(move || {
        resolve_local_library_path_in(&library_dir, &path, &local_folders).and_then(|resolved| {
            resolved
                .into_os_string()
                .into_string()
                .map_err(|_| "Caminho local não pode ser exibido em UTF-8".to_string())
        })
    })
    .await
    .map_err(|e| format!("Falha ao resolver localização local: {e}"))?
}

#[tauri::command]
fn get_favorites() -> Result<Vec<FavoriteAlbum>, String> {
    let config = match AppConfig::load() {
        Ok(config) => Some(config),
        Err(error) => {
            eprintln!("[Favoritos] Configuração indisponível; artwork preservado: {error}");
            None
        }
    };
    FavoriteAlbum::load_all(config.as_ref())
}

#[tauri::command]
fn toggle_favorite(album: FavoriteAlbum) -> Result<bool, String> {
    let config = AppConfig::load()?;
    FavoriteAlbum::toggle(album, &config)
}

#[tauri::command]
fn list_playlists(state: State<PlaylistState>) -> Result<Vec<Playlist>, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .list()
}

#[tauri::command]
fn create_playlist(name: String, state: State<PlaylistState>) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .create(&name)
}

#[tauri::command]
fn create_playlist_with_items(
    name: String,
    items: Vec<NewPlaylistItem>,
    state: State<PlaylistState>,
) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .create_with_items(&name, items)
}

#[tauri::command]
fn rename_playlist(
    id: String,
    name: String,
    state: State<PlaylistState>,
) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .rename(&id, &name)
}

#[tauri::command]
fn delete_playlist(id: String, state: State<PlaylistState>) -> Result<(), String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .delete(&id)
}

#[tauri::command]
fn add_playlist_item(
    playlist_id: String,
    item: NewPlaylistItem,
    state: State<PlaylistState>,
) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .add_item(&playlist_id, item)
}

#[tauri::command]
fn add_playlist_items(
    playlist_id: String,
    items: Vec<NewPlaylistItem>,
    state: State<PlaylistState>,
) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .add_items(&playlist_id, items)
}

#[tauri::command]
fn remove_playlist_item(
    playlist_id: String,
    item_id: String,
    state: State<PlaylistState>,
) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .remove_item(&playlist_id, &item_id)
}

#[tauri::command]
fn reorder_playlist_items(
    playlist_id: String,
    ordered_item_ids: Vec<String>,
    state: State<PlaylistState>,
) -> Result<Playlist, String> {
    state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .reorder_items(&playlist_id, &ordered_item_ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PlaylistItemAvailabilityStatus {
    Available,
    Missing,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct PlaylistItemAvailability {
    item_id: String,
    status: PlaylistItemAvailabilityStatus,
    reason: Option<String>,
}

fn local_playlist_item_availability(
    item_id: String,
    result: Result<bool, String>,
) -> PlaylistItemAvailability {
    match result {
        Ok(true) => PlaylistItemAvailability {
            item_id,
            status: PlaylistItemAvailabilityStatus::Available,
            reason: None,
        },
        Ok(false) => PlaylistItemAvailability {
            item_id,
            status: PlaylistItemAvailabilityStatus::Missing,
            reason: None,
        },
        Err(reason) => PlaylistItemAvailability {
            item_id,
            status: PlaylistItemAvailabilityStatus::Unavailable,
            reason: Some(reason),
        },
    }
}

fn plex_playlist_item_availability(
    item_id: String,
    availability: PlexMediaAvailability,
) -> PlaylistItemAvailability {
    match availability {
        PlexMediaAvailability::Available => PlaylistItemAvailability {
            item_id,
            status: PlaylistItemAvailabilityStatus::Available,
            reason: None,
        },
        PlexMediaAvailability::Missing => PlaylistItemAvailability {
            item_id,
            status: PlaylistItemAvailabilityStatus::Missing,
            reason: None,
        },
        PlexMediaAvailability::Unavailable(reason) => PlaylistItemAvailability {
            item_id,
            status: PlaylistItemAvailabilityStatus::Unavailable,
            reason: Some(reason),
        },
    }
}

#[tauri::command]
async fn resolve_playlist_items(
    playlist_id: String,
    playlist_state: State<'_, PlaylistState>,
    audio_state: State<'_, AudioState>,
    plex_state: State<'_, PlexState>,
) -> Result<Vec<PlaylistItemAvailability>, String> {
    let playlist = playlist_state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .get(&playlist_id)?;
    let plex_client = plex_state
        .0
        .lock()
        .map(|client| client.clone())
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string());
    let mut statuses = Vec::with_capacity(playlist.items.len());
    for item in playlist.items {
        let status = match &item.media_locator {
            MediaLocator::Local { uri } => {
                let result = audio_state
                    .0
                    .lock()
                    .map_err(|_| "O estado da biblioteca local está indisponível.".to_string())
                    .and_then(|audio| audio.local_media_exists(uri));
                local_playlist_item_availability(item.id, result)
            }
            MediaLocator::Plex { .. } => {
                let availability = match &plex_client {
                    Ok(client) => client.media_availability(&item.media_locator).await,
                    Err(reason) => PlexMediaAvailability::Unavailable(reason.clone()),
                };
                plex_playlist_item_availability(item.id, availability)
            }
        };
        statuses.push(status);
    }
    Ok(statuses)
}

const NO_PLAYABLE_PLAYLIST_ITEMS: &str = "playlist_no_playable_items";
const SELECTED_PLAYLIST_ITEM_UNAVAILABLE: &str = "playlist_selected_item_unavailable";

#[derive(Serialize)]
struct PlaylistPlaybackResult {
    skipped_count: usize,
}

struct PreparedPlaylistPlayback {
    tracks: Vec<TrackMetadata>,
    playback_uris: Vec<String>,
    start_index: usize,
    skipped_count: usize,
}

fn prepare_playlist_playback(
    playlist: &Playlist,
    resolved_uris: Vec<Option<(String, Option<PlexImageRef>)>>,
    start_item_id: Option<&str>,
) -> Result<PreparedPlaylistPlayback, String> {
    if resolved_uris.len() != playlist.items.len() {
        return Err("A resolução da playlist retornou uma quantidade inválida de itens.".into());
    }
    if let Some(id) = start_item_id {
        if !playlist.items.iter().any(|item| item.id == id) {
            return Err("Item da playlist não encontrado.".into());
        }
    }
    let mut tracks = Vec::new();
    let mut playback_uris = Vec::new();
    let mut start_index = None;
    for (item, uri) in playlist.items.iter().zip(resolved_uris) {
        if let Some((uri, artwork)) = uri {
            if start_item_id == Some(item.id.as_str()) {
                start_index = Some(tracks.len());
            }
            tracks.push(TrackMetadata {
                title: item.metadata.title.clone(),
                artist: item.metadata.artist.clone(),
                album: item.metadata.album.clone(),
                thumb: None,
                plex_image: artwork,
                media_locator: Some(item.media_locator.clone()),
                uri: match &item.media_locator {
                    MediaLocator::Local { uri } => uri.clone(),
                    MediaLocator::Plex { .. } => String::new(),
                },
                duration: item.metadata.duration,
            });
            playback_uris.push(uri);
        } else if start_item_id == Some(item.id.as_str()) {
            return Err(SELECTED_PLAYLIST_ITEM_UNAVAILABLE.into());
        }
    }
    if tracks.is_empty() {
        return Err(NO_PLAYABLE_PLAYLIST_ITEMS.into());
    }
    Ok(PreparedPlaylistPlayback {
        skipped_count: playlist.items.len() - tracks.len(),
        tracks,
        playback_uris,
        start_index: start_index.unwrap_or(0),
    })
}

fn shuffle_prepared_playlist_playback(
    prepared: &mut PreparedPlaylistPlayback,
    mut random_index: impl FnMut(usize) -> Result<usize, String>,
) -> Result<(), String> {
    for end in (1..prepared.tracks.len()).rev() {
        let index = random_index(end + 1)?;
        if index > end {
            return Err("Índice aleatório inválido para a playlist.".into());
        }
        prepared.tracks.swap(index, end);
        prepared.playback_uris.swap(index, end);
    }
    prepared.start_index = 0;
    Ok(())
}

fn random_playlist_index(upper: usize, source: &mut impl Read) -> Result<usize, String> {
    let upper = upper as u64;
    let threshold = upper.wrapping_neg() % upper;
    loop {
        let mut bytes = [0u8; 8];
        source.read_exact(&mut bytes)
            .map_err(|error| format!("Falha ao obter aleatoriedade para a playlist: {error}"))?;
        let value = u64::from_ne_bytes(bytes);
        if value >= threshold {
            return Ok((value % upper) as usize);
        }
    }
}

#[tauri::command]
async fn play_playlist(
    playlist_id: String,
    start_item_id: Option<String>,
    shuffle: Option<bool>,
    playlist_state: State<'_, PlaylistState>,
    audio_state: State<'_, AudioState>,
    plex_state: State<'_, PlexState>,
) -> Result<PlaylistPlaybackResult, String> {
    if shuffle == Some(true) && start_item_id.is_some() {
        return Err("Não é possível combinar embaralhamento com início em uma faixa.".into());
    }
    let playlist = playlist_state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .get(&playlist_id)?;
    let plex_client = plex_state
        .0
        .lock()
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
        .clone();
    let mut resolved_uris = Vec::with_capacity(playlist.items.len());
    for item in &playlist.items {
        let uri = match &item.media_locator {
            MediaLocator::Local { uri } => {
                let exists = audio_state
                    .0
                    .lock()
                    .map_err(|_| "O estado de reprodução está indisponível.".to_string())?
                    .local_media_exists(uri);
                match exists {
                    Ok(true) => Some((uri.clone(), None)),
                    Ok(false) => None,
                    Err(_) => {
                        eprintln!("[Playlist] Falha ao verificar item local {} no MPD.", item.id);
                        None
                    }
                }
            }
            MediaLocator::Plex { .. } => {
                let (availability, artwork) = plex_client
                    .media_availability_with_artwork(&item.media_locator).await;
                match availability {
                    PlexMediaAvailability::Available => match plex_client
                        .resolve_media_locator(&item.media_locator).await {
                        Ok(uri) => Some((uri, artwork)),
                        Err(_) => {
                            eprintln!("[Playlist] Falha ao resolver item Plex {}.", item.id);
                            None
                        }
                    },
                    PlexMediaAvailability::Missing => None,
                    PlexMediaAvailability::Unavailable(_) => {
                        eprintln!("[Playlist] Item Plex {} indisponível nesta tentativa.", item.id);
                        None
                    }
                }
            }
        };
        resolved_uris.push(uri);
    }
    let mut prepared = prepare_playlist_playback(&playlist, resolved_uris, start_item_id.as_deref())?;
    if shuffle == Some(true) && prepared.tracks.len() > 1 {
        let mut source = std::fs::File::open("/dev/urandom")
            .map_err(|error| format!("Falha ao obter aleatoriedade para a playlist: {error}"))?;
        shuffle_prepared_playlist_playback(&mut prepared, |upper| random_playlist_index(upper, &mut source))?;
    }
    audio_state
        .0
        .lock()
        .map_err(|_| "O estado de reprodução está indisponível.".to_string())?
        .play_tracks(prepared.tracks, prepared.playback_uris, prepared.start_index)?;
    Ok(PlaylistPlaybackResult {
        skipped_count: prepared.skipped_count,
    })
}

#[tauri::command]
async fn get_local_cover(path: String) -> Result<Option<String>, String> {
    let lib_dir = supervisor::MpdSupervisor::library_dir();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || {
        AudioEngine::get_local_cover_at(&socket_path, &lib_dir, &path)
    })
    .await
    .map_err(|e| format!("Falha na tarefa de capa local: {e}"))?
}

#[tauri::command]
async fn get_online_album_cover(
    request: online_artwork::OnlineAlbumCoverRequest,
    config_state: State<'_, ConfigState>,
) -> Result<Option<String>, String> {
    let enabled = config_state
        .0
        .lock()
        .map_err(|_| "Configuração indisponível.".to_string())?
        .online_artwork_enabled;
    online_artwork::get_online_album_cover(request, enabled).await
}

#[tauri::command]
async fn get_online_cover_cache_status(
    request: online_artwork::OnlineAlbumCoverRequest,
) -> Result<String, String> {
    online_artwork::cached_cover_status(request).await
}

#[tauri::command]
async fn get_artwork_enrichment_progress(
) -> Result<std::collections::BTreeMap<String, artwork_progress::Entry>, String> {
    artwork_progress::read().await
}

#[tauri::command]
async fn save_artwork_enrichment_progress(
    changes: Vec<artwork_progress::Change>,
) -> Result<(), String> {
    artwork_progress::write(changes).await
}

#[tauri::command]
fn pick_directory() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Selecionar Pasta de Músicas")
        .pick_folder()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn rescan_library(
    state: State<AudioState>,
    config_state: State<ConfigState>,
) -> Result<(), String> {
    // O nome IPC permanece para compatibilidade; com database existente, o refresh é incremental.
    let has_folders = !config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar pastas locais: {e}"))?
        .local_folders
        .is_empty();
    if let Some(refresh) = library_refresh(true, database_exists()?, has_folders) {
        let socket_path = state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar a biblioteca local: {e}"))?
            .local_album_socket_path();
        request_library_refresh(&socket_path, refresh)?;
    }
    Ok(())
}

#[tauri::command]
fn get_config(state: State<ConfigState>) -> Result<AppConfig, String> {
    AppConfig::load()?;
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
    AppConfig::load()?;
    let observed_cfg = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a configuração atual: {}", e))?
        .clone();

    let observed_finishing_first_run = is_finishing_first_run(&observed_cfg, &new_config);
    let output_change_expected = !observed_finishing_first_run
        && (observed_cfg.audio_output_type != new_config.audio_output_type
            || observed_cfg.alsa_device != new_config.alsa_device
            || observed_cfg.dop_enabled != new_config.dop_enabled
            || observed_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
            || observed_cfg.replay_gain != new_config.replay_gain);
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

    let finishing_first_run = is_finishing_first_run(&current_cfg, &new_config);
    let audio_hw_changed = current_cfg.audio_output_type != new_config.audio_output_type
        || current_cfg.alsa_device != new_config.alsa_device
        || current_cfg.dop_enabled != new_config.dop_enabled
        || current_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
        || current_cfg.replay_gain != new_config.replay_gain;

    let folders_changed = local_folders_changed(&current_cfg, &new_config);
    let database_existed = if folders_changed || finishing_first_run || audio_hw_changed {
        database_exists()?
    } else {
        true
    };
    let mut library_changed_during_start = false;
    let mut playback_snapshot = None;
    let mut resume_analyzer = false;

    if finishing_first_run {
        let start_result = sup_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar MPD na configuração inicial: {}", e))?
            .start(&new_config);
        library_changed_during_start = match start_result {
            Ok(sync) => sync.changed,
            Err(error) => {
                let rollback =
                    rollback_first_run_completion(folders_changed, &current_cfg, &sup_state);
                return Err(error_with_rollback(
                    format!(
                        "Falha ao iniciar MPD ao concluir a configuração inicial: {}",
                        error
                    ),
                    rollback,
                ));
            }
        };
    } else if should_prepare_device_switch(audio_hw_changed, finishing_first_run) {
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
        library_changed_during_start = match switch_result {
            Ok(sync) => sync.changed,
            Err(switch_error) => {
                let rollback = rollback_audio_switch(
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
        };

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

    if folders_changed
        || library_changed_during_start
        || (finishing_first_run && !new_config.local_folders.is_empty())
    {
        let update_library = || -> Result<(), String> {
            let sync = MpdSupervisor::sync_library_symlinks(&new_config.local_folders)?;
            let socket_path = {
                let mut audio = audio_state
                    .0
                    .lock()
                    .map_err(|e| format!("Falha ao acessar a biblioteca local: {}", e))?;
                audio.set_music_dir(&sync.library_dir.to_string_lossy());
                audio.local_album_socket_path()
            };
            if let Some(refresh) = library_refresh_after_sync(
                sync.changed,
                library_changed_during_start,
                finishing_first_run,
                database_existed,
                !new_config.local_folders.is_empty(),
            ) {
                request_library_refresh(&socket_path, refresh)?;
            }
            Ok(())
        };
        if let Err(library_error) = update_library() {
            let rollback = rollback_applied_config_change(
                finishing_first_run,
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
                finishing_first_run,
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
                finishing_first_run,
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
            finishing_first_run,
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
            if analyzer.has_window_session() && app.get_webview_window("now-playing").is_some() {
                if let Err(error) = analyzer.start(app) {
                    eprintln!(
                        "[Analyzer] Configuração aplicada, mas o analyzer não pôde ser retomado: {}",
                        error
                    );
                }
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
    finishing_first_run: bool,
    audio_hw_changed: bool,
    folders_changed: bool,
    previous_config: &AppConfig,
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    playback_uris: &[String],
    sup_state: &State<'_, SupervisorState>,
    audio_state: &State<'_, AudioState>,
) -> Result<(), String> {
    if finishing_first_run {
        return rollback_first_run_completion(folders_changed, previous_config, sup_state);
    }

    let mut failures = Vec::new();
    if folders_changed {
        let library_rollback = MpdSupervisor::sync_library_symlinks(&previous_config.local_folders)
            .and_then(|sync| {
                if let Some(refresh) = library_refresh(
                    sync.changed,
                    database_exists()?,
                    !previous_config.local_folders.is_empty(),
                ) {
                    let socket_path = audio_state
                        .0
                        .lock()
                        .map_err(|e| {
                            format!("Falha ao acessar a biblioteca durante rollback: {}", e)
                        })?
                        .local_album_socket_path();
                    request_library_refresh(&socket_path, refresh)?;
                }
                Ok(())
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

fn rollback_first_run_completion(
    folders_changed: bool,
    previous_config: &AppConfig,
    sup_state: &State<'_, SupervisorState>,
) -> Result<(), String> {
    let mut failures = Vec::new();
    if let Err(error) = sup_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o supervisor durante rollback: {}", e))?
        .stop()
    {
        failures.push(format!("MPD: {}", error));
    }

    if folders_changed {
        if let Err(error) = MpdSupervisor::sync_library_symlinks(&previous_config.local_folders) {
            failures.push(format!("biblioteca: {}", error));
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[tauri::command]
async fn get_local_albums(
    state: State<'_, AudioState>,
    config_state: State<'_, ConfigState>,
) -> Result<Vec<audio::LocalAlbum>, String> {
    let socket_path = state.0.lock()
        .map_err(|e| format!("Falha ao acessar conexão da biblioteca local: {e}"))?
        .local_album_socket_path();
    let local_folders = config_state.0.lock()
        .map_err(|e| format!("Falha ao acessar pastas da biblioteca local: {e}"))?
        .local_folders
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let source_ids = MpdSupervisor::local_library_sources(&local_folders)?
            .into_iter()
            .map(|source| source.id)
            .collect();
        AudioEngine::get_local_albums_at(&socket_path, &source_ids)
    })
        .await
        .map_err(|e| format!("Falha na tarefa de consulta da biblioteca local: {e}"))?
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
    let (initial_config, config_valid) = match AppConfig::load() {
        Ok(config) => (config, true),
        Err(error) => {
            eprintln!("[Persistência] {error}");
            (AppConfig::default(), false)
        }
    };

    let should_start_mpd = should_start_mpd_on_startup(config_valid, &initial_config);
    let startup_rescan = if should_start_mpd {
        match MpdSupervisor::database_path().try_exists() {
            Ok(database_exists) => {
                should_rescan_library_on_startup(config_valid, &initial_config, database_exists)
            }
            Err(error) => {
                eprintln!(
                    "[Audio] Falha ao verificar database MPD no startup: {}",
                    error
                );
                false
            }
        }
    } else {
        false
    };

    let mut supervisor = MpdSupervisor::new(&socket_path);
    let mpd_started = if should_start_mpd {
        match supervisor.start(&initial_config) {
            Ok(_) => true,
            Err(error) => {
                eprintln!("[Aviso] Erro no supervisor de áudio: {}", error);
                false
            }
        }
    } else {
        supervisor.mark_unavailable(MpdUnavailableReason::StartupFailed);
        false
    };

    let audio_engine = AudioEngine::new(
        &socket_path,
        &MpdSupervisor::library_dir().to_string_lossy(),
    );
    let audio_analyzer = AudioAnalyzer::new(
        socket_path.clone(),
        MpdSupervisor::analyzer_fifo_path(),
    );
    if mpd_started && startup_rescan {
        if let Err(e) = audio_engine.rescan_library() {
            eprintln!(
                "[Audio] Falha ao solicitar rescan inicial da biblioteca: {}",
                e
            );
        }
    }

    let plex_client = PlexClient::from_config(&initial_config);

    let app = tauri::Builder::default()
        .manage(AudioState(Mutex::new(audio_engine)))
        .manage(AnalyzerState(Mutex::new(audio_analyzer)))
        .manage(PlexState(Mutex::new(plex_client)))
        .manage(SupervisorState(Mutex::new(supervisor)))
        .manage(ConfigState(Mutex::new(initial_config)))
        .manage(ConfigTransactionState(Mutex::new(())))
        .manage(PlaylistState(Mutex::new(PlaylistStore::default())))
        .manage(mpris::MprisState::default())
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
            resolve_local_library_path,
            get_local_cover,
            get_online_album_cover,
            get_online_cover_cache_status,
            get_artwork_enrichment_progress,
            save_artwork_enrichment_progress,
            get_local_albums,
            get_favorites,
            toggle_favorite,
            list_playlists,
            create_playlist,
            create_playlist_with_items,
            rename_playlist,
            delete_playlist,
            add_playlist_item,
            add_playlist_items,
            remove_playlist_item,
            reorder_playlist_items,
            resolve_playlist_items,
            play_playlist,
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
                            if let Err(error) = analyzer.stop_for_window_close(window.app_handle()) {
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

    let restore_started = AtomicBool::new(false);
    let shutdown_started = AtomicBool::new(false);
    app.run(move |app_handle, event| {
        match event {
            RunEvent::Ready => {
                mpris::start(app_handle.clone());
                if mpd_started && !restore_started.swap(true, Ordering::SeqCst) {
                    let app = app_handle.clone();
                    tauri::async_runtime::spawn(restore_cached_queue_after_ready(app));
                }
            }
            RunEvent::ExitRequested { .. } | RunEvent::Exit => {
                if shutdown_started.swap(true, Ordering::SeqCst) {
                    return;
                }
                mpris::shutdown(app_handle);
                if let Some(audio_state) = app_handle.try_state::<AudioState>() {
                    match audio_state.0.lock() {
                        Ok(mut audio) => {
                            if let Err(error) = audio.persist_shutdown_resume() {
                                eprintln!("[Audio] Falha ao salvar posição no shutdown: {}", error);
                            }
                        }
                        Err(_) => eprintln!("[Audio] Falha ao acessar posição no shutdown."),
                    }
                }
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
    use playlists::{PlaylistItem, PlaylistItemMetadata};
    use std::cell::Cell;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    static NEXT_LOCAL_PATH_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct LocalPathFixture {
        root: PathBuf,
        library: PathBuf,
    }

    impl LocalPathFixture {
        fn new() -> Self {
            let id = NEXT_LOCAL_PATH_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("sonante-local-path-{}-{id}", std::process::id()));
            let library = root.join("library");
            fs::create_dir_all(&library).unwrap();
            Self { root, library }
        }
    }

    impl Drop for LocalPathFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn local_library_path_resolves_configured_root_and_specific_duplicate_name() {
        let fixture = LocalPathFixture::new();
        let first = fixture.root.join("first/Music");
        let second = fixture.root.join("second/Music");
        fs::create_dir_all(first.join("Album")).unwrap();
        fs::create_dir_all(second.join("Album")).unwrap();
        symlink(&first, fixture.library.join("Music")).unwrap();
        symlink(&second, fixture.library.join("Music (2)")).unwrap();
        let folders = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];

        assert_eq!(
            resolve_local_library_path_in(&fixture.library, "Music/Album", &folders).unwrap(),
            first.join("Album")
        );
        assert_eq!(
            resolve_local_library_path_in(&fixture.library, "Music (2)/Album", &folders).unwrap(),
            second.join("Album")
        );
        assert_eq!(
            resolve_local_library_path_in(&fixture.library, "Music", &folders).unwrap(),
            first
        );
    }

    #[test]
    fn local_library_path_rejects_absolute_parent_and_empty_paths() {
        let fixture = LocalPathFixture::new();
        let root = fixture.root.join("Music");
        fs::create_dir(&root).unwrap();
        symlink(&root, fixture.library.join("Music")).unwrap();
        let folders = vec![root.to_string_lossy().into_owned()];

        for invalid in ["", "/Music", "../Music", "Music/../Music"] {
            assert!(
                resolve_local_library_path_in(&fixture.library, invalid, &folders).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn local_library_path_rejects_missing_path_and_unconfigured_link() {
        let fixture = LocalPathFixture::new();
        let configured = fixture.root.join("configured");
        let other = fixture.root.join("other");
        fs::create_dir(&configured).unwrap();
        fs::create_dir(&other).unwrap();
        symlink(&configured, fixture.library.join("Music")).unwrap();
        symlink(&other, fixture.library.join("Other")).unwrap();
        let folders = vec![configured.to_string_lossy().into_owned()];

        assert!(
            resolve_local_library_path_in(&fixture.library, "Music/Missing", &folders).is_err()
        );
        assert!(resolve_local_library_path_in(&fixture.library, "Other", &folders).is_err());
    }

    #[test]
    fn local_library_path_rejects_symlink_escape_from_configured_root() {
        let fixture = LocalPathFixture::new();
        let configured = fixture.root.join("configured");
        let outside = fixture.root.join("outside");
        fs::create_dir(&configured).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&configured, fixture.library.join("Music")).unwrap();
        symlink(&outside, configured.join("Escape")).unwrap();
        let folders = vec![configured.to_string_lossy().into_owned()];

        assert!(resolve_local_library_path_in(&fixture.library, "Music/Escape", &folders).is_err());
    }

    fn playback_playlist(locators: Vec<MediaLocator>) -> Playlist {
        Playlist {
            id: "playlist".into(),
            name: "Teste".into(),
            created_at: 1,
            updated_at: 1,
            items: locators.into_iter().enumerate().map(|(index, media_locator)| {
                PlaylistItem {
                    id: format!("occurrence-{index}"),
                    media_locator,
                    metadata: PlaylistItemMetadata {
                        title: format!("Faixa {index}"),
                        artist: "Artista".into(),
                        album: "Álbum".into(),
                        duration: Some(100.0),
                    },
                }
            }).collect(),
        }
    }

    fn local_locator(uri: &str) -> MediaLocator {
        MediaLocator::Local { uri: uri.into() }
    }

    #[test]
    fn playlist_playback_preserves_order_duplicates_and_mixed_sources() {
        let plex = MediaLocator::Plex {
            server_id: "server".into(),
            part_key: "/library/parts/42/file.flac".into(),
            rating_key: Some("42".into()),
            file_path: None,
        };
        let playlist = playback_playlist(vec![
            local_locator("Álbum/01 \\\"a\\\".flac"),
            plex.clone(),
            local_locator("Álbum/01 \\\"a\\\".flac"),
        ]);
        let prepared = prepare_playlist_playback(&playlist, vec![
            Some(("Álbum/01 \\\"a\\\".flac".into(), None)),
            Some(("https://plex.test/stream".into(), Some(PlexImageRef {
                server_id: "server".into(),
                path: "/library/metadata/42/thumb/1".into(),
            }))),
            Some(("Álbum/01 \\\"a\\\".flac".into(), None)),
        ], None).unwrap();
        assert_eq!(prepared.tracks.len(), 3);
        assert_eq!(prepared.start_index, 0);
        assert_eq!(prepared.skipped_count, 0);
        assert_eq!(prepared.tracks.iter().map(|track| track.title.as_str()).collect::<Vec<_>>(),
                   vec!["Faixa 0", "Faixa 1", "Faixa 2"]);
        assert_eq!(prepared.playback_uris[0], prepared.playback_uris[2]);
        assert_eq!(prepared.tracks[1].media_locator, Some(plex));
        assert_eq!(prepared.tracks[1].plex_image.as_ref().unwrap().server_id, "server");
        assert!(prepared.tracks[1].uri.is_empty());

        let from_duplicate = prepare_playlist_playback(&playlist, vec![
            Some(("Álbum/01 \\\"a\\\".flac".into(), None)),
            Some(("https://plex.test/stream".into(), None)),
            Some(("Álbum/01 \\\"a\\\".flac".into(), None)),
        ], Some("occurrence-2")).unwrap();
        assert_eq!(from_duplicate.start_index, 2);
    }

    #[test]
    fn playlist_playback_skips_missing_and_unavailable_without_reordering() {
        let playlist = playback_playlist((0..5).map(|index| local_locator(&format!("{index}.flac"))).collect());
        let prepared = prepare_playlist_playback(&playlist, vec![
            Some(("0.flac".into(), None)), None, Some(("2.flac".into(), None)), None,
            Some(("4.flac".into(), None)),
        ], Some("occurrence-2")).unwrap();
        assert_eq!(prepared.playback_uris, vec!["0.flac", "2.flac", "4.flac"]);
        assert_eq!(prepared.start_index, 1);
        assert_eq!(prepared.skipped_count, 2);
    }

    #[test]
    fn playlist_playback_rejects_unplayable_selection_or_empty_queue_before_mpd() {
        let playlist = playback_playlist(vec![local_locator("first.flac"), local_locator("second.flac")]);
        assert_eq!(prepare_playlist_playback(&playlist, vec![None, None], None).err().unwrap(),
                   NO_PLAYABLE_PLAYLIST_ITEMS);
        assert_eq!(prepare_playlist_playback(&playlist, vec![Some(("first.flac".into(), None)), None],
                    Some("occurrence-1")).err().unwrap(), SELECTED_PLAYLIST_ITEM_UNAVAILABLE);
        assert!(prepare_playlist_playback(&playlist, vec![Some(("first.flac".into(), None))], None).is_err());
    }

    #[test]
    fn playlist_shuffle_rejects_zero_playable_items_before_playback() {
        let empty = playback_playlist(vec![]);
        assert_eq!(prepare_playlist_playback(&empty, vec![], None).err().unwrap(),
                   NO_PLAYABLE_PLAYLIST_ITEMS);
        let unavailable = playback_playlist(vec![local_locator("missing.flac"), local_locator("offline.flac")]);
        assert_eq!(prepare_playlist_playback(&unavailable, vec![None, None], None).err().unwrap(),
                   NO_PLAYABLE_PLAYLIST_ITEMS);
    }

    #[test]
    fn playlist_shuffle_keeps_one_playable_item() {
        let playlist = playback_playlist(vec![local_locator("missing.flac"), local_locator("one.flac")]);
        let mut prepared = prepare_playlist_playback(&playlist, vec![None, Some(("one.flac".into(), None))], None).unwrap();
        shuffle_prepared_playlist_playback(&mut prepared, |_| panic!("single item needs no randomness")).unwrap();
        assert_eq!(prepared.playback_uris, ["one.flac"]);
        assert_eq!(prepared.start_index, 0);
        assert_eq!(prepared.skipped_count, 1);
    }

    #[test]
    fn playlist_shuffle_preserves_playable_occurrences_and_persisted_order() {
        let plex = MediaLocator::Plex {
            server_id: "server".into(),
            part_key: "/library/parts/42/file.flac".into(),
            rating_key: Some("42".into()),
            file_path: None,
        };
        let playlist = playback_playlist(vec![
            local_locator("Álbum/one \\\"quote\\\".flac"),
            local_locator("missing.flac"),
            plex,
            local_locator("Álbum/one \\\"quote\\\".flac"),
            local_locator("unavailable.flac"),
            local_locator("four.flac"),
        ]);
        let original = playlist.clone();
        let mut prepared = prepare_playlist_playback(&playlist, vec![
            Some(("Álbum/one \\\"quote\\\".flac".into(), None)),
            None,
            Some(("https://plex.test/stream".into(), None)),
            Some(("Álbum/one \\\"quote\\\".flac".into(), None)),
            None,
            Some(("four.flac".into(), None)),
        ], None).unwrap();
        let mut before = prepared.tracks.iter().zip(&prepared.playback_uris)
            .map(|(track, uri)| (track.title.clone(), uri.clone())).collect::<Vec<_>>();
        let mut choices = [0, 1, 0].into_iter();
        shuffle_prepared_playlist_playback(&mut prepared, |_| Ok(choices.next().unwrap())).unwrap();
        let mut after = prepared.tracks.iter().zip(&prepared.playback_uris)
            .map(|(track, uri)| (track.title.clone(), uri.clone())).collect::<Vec<_>>();
        assert_ne!(after, before);
        before.sort();
        after.sort();
        assert_eq!(after, before);
        assert_eq!(prepared.playback_uris.iter().filter(|uri| uri.as_str() == "Álbum/one \\\"quote\\\".flac").count(), 2);
        assert_eq!(prepared.skipped_count, 2);
        assert_eq!(prepared.start_index, 0);
        assert_eq!(playlist, original);
    }

    static NEXT_MPD_HEALTH_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn main_window_close_requests_application_exit() {
        assert!(should_exit_application_on_window_close("main"));
    }

    #[test]
    fn secondary_window_close_does_not_request_application_exit() {
        assert!(!should_exit_application_on_window_close("now-playing"));
    }

    #[test]
    fn configured_installation_is_eligible_for_startup() {
        let mut config = AppConfig::default();
        config.first_run = false;

        assert!(should_start_mpd_on_startup(true, &config));
        assert!(!should_start_mpd_on_startup(false, &config));
    }

    #[test]
    fn first_run_is_not_eligible_for_automatic_startup() {
        assert!(!should_start_mpd_on_startup(true, &AppConfig::default()));
    }

    #[test]
    fn shared_seek_mode_matches_supervisor_output_definition() {
        let mut config = AppConfig::default();
        config.alsa_device = "hw:CARD=DAC,DEV=0".to_string();
        config.audio_output_type = "alsa".to_string();
        assert!(!is_shared_output(&config));

        config.audio_output_type = "pipewire".to_string();
        assert!(is_shared_output(&config));
        config.audio_output_type = "shared".to_string();
        assert!(is_shared_output(&config));
        config.audio_output_type = "alsa".to_string();
        config.alsa_device = "default".to_string();
        assert!(is_shared_output(&config));
    }

    #[test]
    fn startup_rescan_only_recovers_a_missing_database_with_local_folders() {
        let mut config = AppConfig::default();
        config.first_run = false;
        config.local_folders = vec!["/music".to_string()];

        assert!(!should_rescan_library_on_startup(true, &config, true));
        assert!(should_rescan_library_on_startup(true, &config, false));
        assert!(!should_rescan_library_on_startup(false, &config, false));

        config.local_folders.clear();
        assert!(!should_rescan_library_on_startup(true, &config, false));

        config.local_folders.push("/music".to_string());
        config.first_run = true;
        assert!(!should_rescan_library_on_startup(true, &config, false));
    }

    #[test]
    fn library_refresh_uses_update_for_existing_database_and_rescan_for_first_database() {
        assert_eq!(
            library_refresh(true, true, true),
            Some(LibraryRefresh::Update)
        );
        assert_eq!(
            library_refresh(true, true, false),
            Some(LibraryRefresh::Update)
        );
        assert_eq!(
            library_refresh(true, false, true),
            Some(LibraryRefresh::Rescan)
        );
        assert_eq!(library_refresh(true, false, false), None);
        assert_eq!(library_refresh(false, true, true), None);
        assert_eq!(library_refresh(false, false, true), None);
        assert_eq!(
            library_refresh_after_sync(false, false, false, true, true),
            None
        );
        assert_eq!(
            library_refresh_after_sync(false, true, false, true, true),
            Some(LibraryRefresh::Update)
        );
        assert_eq!(
            library_refresh_after_sync(false, false, true, false, true),
            Some(LibraryRefresh::Rescan)
        );
        assert_eq!(
            library_refresh_after_sync(true, true, true, false, true),
            Some(LibraryRefresh::Rescan)
        );
    }

    #[test]
    fn versioned_database_migration_requests_only_one_initial_rescan() {
        let fixture = LocalPathFixture::new();
        let legacy_database = fixture.root.join("mpd.db");
        let versioned_database = MpdSupervisor::database_path_in(&fixture.root);
        let mut config = AppConfig::default();
        config.first_run = false;
        config.local_folders = vec![fixture.root.join("Music").to_string_lossy().into_owned()];
        fs::write(&legacy_database, "legacy database fixture").unwrap();

        assert_eq!(versioned_database, fixture.root.join("mpd-v2.db"));
        assert!(legacy_database.exists());
        assert!(!versioned_database.exists());
        assert!(should_rescan_library_on_startup(
            true,
            &config,
            versioned_database.exists()
        ));

        fs::write(&versioned_database, "versioned database fixture").unwrap();
        assert!(!should_rescan_library_on_startup(
            true,
            &config,
            versioned_database.exists()
        ));
        assert!(legacy_database.exists());
    }

    #[test]
    fn changing_local_folders_is_detected_for_library_reconciliation() {
        let mut current = AppConfig::default();
        current.first_run = false;
        let mut next = current.clone();
        assert!(!local_folders_changed(&current, &next));

        next.local_folders.push("/music".to_string());
        assert!(local_folders_changed(&current, &next));
    }

    #[test]
    fn finishing_first_run_is_explicit_and_skips_device_switch_preparation() {
        let current = AppConfig::default();
        let mut completed = current.clone();
        completed.first_run = false;

        assert!(is_finishing_first_run(&current, &completed));
        assert!(!is_finishing_first_run(&completed, &completed));
        assert!(!should_prepare_device_switch(true, true));
        assert!(should_prepare_device_switch(true, false));
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

    fn spawn_find_server(socket_path: &Path, response: &'static [u8]) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(socket_path).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.23.15\n").unwrap();
            stream.flush().unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command, "find file \"missing.flac\"\n");
            stream.write_all(response).unwrap();
            stream.flush().unwrap();
        })
    }

    #[test]
    fn successful_mpd_query_without_item_is_missing() {
        let (dir, socket_path, _) = mpd_health_test_paths("playlist-item-missing");
        let server = spawn_find_server(&socket_path, b"OK\n");
        let audio = AudioEngine::new(&socket_path.to_string_lossy(), &dir.to_string_lossy());

        let availability = local_playlist_item_availability(
            "item-1".to_string(),
            audio.local_media_exists("missing.flac"),
        );

        assert_eq!(availability.status, PlaylistItemAvailabilityStatus::Missing);
        assert_eq!(availability.reason, None);
        server.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_mpd_query_is_not_missing() {
        let (dir, socket_path, _) = mpd_health_test_paths("playlist-item-unavailable");
        let audio = AudioEngine::new(&socket_path.to_string_lossy(), &dir.to_string_lossy());

        let availability = local_playlist_item_availability(
            "item-1".to_string(),
            audio.local_media_exists("missing.flac"),
        );

        assert_eq!(
            availability.status,
            PlaylistItemAvailabilityStatus::Unavailable
        );
        assert!(availability.reason.is_some());
        fs::remove_dir_all(dir).unwrap();
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
