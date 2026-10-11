mod alsa_mixer;
mod alsa_preflight;
mod analyzer;
mod artwork_progress;
mod audio;
mod catalog_genres;
mod config;
mod config_save_operation;
mod diagnostics;
mod favorites;
mod library_doctor;
mod local_media;
mod local_import_preview;
mod local_import_artwork;
mod local_watcher;
#[allow(dead_code)]
mod managed_preparation;
mod managed_save_intent;
mod mpris;
mod online_artwork;
mod persistence;
mod playlists;
mod plex;
mod shared_volume;
mod supervisor;
mod tags;

use audio::{
    list_audio_devices, AudioDevice, AudioEngine, AudioState, DeviceSwitchSnapshot,
    MediaLocator, MpdPlaybackSnapshot, MpdProbeFailure, MpdRefreshCompletion, PlaybackStatus,
    ResolvedDeviceSwitchQueue, TargetedLibraryUpdateErrorCode, TrackMetadata, VolumeBackend, VolumeStatus,
};
use analyzer::{AnalyzerState, AudioAnalyzer};
use config::{AppConfig, ConfigSaveError};
use favorites::FavoriteAlbum;
use local_watcher::LocalWatcherState;
use playlists::{NewPlaylistItem, Playlist, PlaylistState, PlaylistStore};
use plex::{
    PlexAlbum, PlexClient, PlexCollection, PlexImageRef, PlexLibrary, PlexMediaAvailability,
    PlexSearchResults, PlexTrack,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shared_volume::{PipeWireVolume, SharedVolumeBackend};
use std::cell::Cell;
use std::io::Read;
use std::path::{Path, PathBuf};
use supervisor::{MpdHealth, MpdProcessObservation, MpdSupervisor, MpdUnavailableReason};
use std::sync::{atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering}, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, Window, WindowEvent};

pub struct PlexState(pub Mutex<PlexClient>);
pub struct SupervisorState(pub Mutex<MpdSupervisor>);
pub struct ConfigState(pub Mutex<AppConfig>);
pub struct ConfigTransactionState {
    operations: Arc<config_save_operation::Coordinator>,
    startup_config_read_gate: Mutex<bool>,
    restart_gate: Mutex<()>,
    preparation: managed_preparation::ManagedPreparationCoordinator,
    lifecycle: AtomicU8,
    shutdown_worker_started: AtomicBool,
    shutdown_complete: AtomicBool,
}

struct RestartPermit<'a> {
    _guard: std::sync::MutexGuard<'a, ()>,
}

// Only a validated, live startup reconciliation may own library.next.
// Hold the gate through admission, then let readers check persistent config and
// journal without mistaking this operation's staging for an abandoned Save.
struct StartupConfigReadGuard<'a> {
    state: &'a Mutex<bool>,
    admission: Option<std::sync::MutexGuard<'a, bool>>,
}

impl StartupConfigReadGuard<'_> {
    fn allow_reads(&mut self, config_dir: &Path) -> Result<(), String> {
        managed_save_intent::check_startup(config_dir)?;
        AppConfig::load_from_path(&config_dir.join("config.json"))?;
        if let Some(mut admission) = self.admission.take() {
            *admission = true;
        }
        Ok(())
    }
}

impl Drop for StartupConfigReadGuard<'_> {
    fn drop(&mut self) {
        if let Some(admission) = self.admission.as_mut() {
            **admission = false;
        } else {
            *self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
        }
    }
}

const MAIN_WINDOW_LABEL: &str = "main";
const LOCAL_LIBRARY_RECONCILED_EVENT: &str = "local-library-reconciled";
const SHUTDOWN_REQUESTED: u8 = 1;
const SHUTDOWN_STARTED: u8 = 1 << 1;
const MANUAL_RESCAN_TIMEOUT: Duration = Duration::from_secs(300);
const MANUAL_RESCAN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ADDED_SOURCE_FULL_RESCAN_REQUIRED: &str = "LOCAL_SOURCE_FULL_RESCAN_REQUIRED";
const ADDED_SOURCE_STAGING_CLEANUP_FAILED: &str = "LOCAL_SOURCE_STAGING_CLEANUP_FAILED";
static MIRROR_GENERATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct SettingsSaveDiagnostics {
    strategy: Cell<&'static str>,
    rejection: Cell<&'static str>,
    mpd_last_confirmed: Cell<&'static str>,
}

impl SettingsSaveDiagnostics {
    fn new() -> Self {
        Self {
            strategy: Cell::new("skip"),
            rejection: Cell::new("not_evaluated"),
            mpd_last_confirmed: Cell::new("unknown"),
        }
    }

    fn stage(&self, stage: &'static str, started: Instant, outcome: &'static str) {
        eprintln!(
            "[MirrorSave] stage={stage} duration_ms={} outcome={outcome}",
            started.elapsed().as_millis()
        );
    }

    fn finish(&self, result: &Result<(), String>) {
        eprintln!("{}", self.summary(result));
    }

    fn summary(&self, result: &Result<(), String>) -> String {
        let (mpd_state, mpd_availability) = match self.mpd_last_confirmed.get() {
            "stopped" => ("stopped", "unavailable"),
            "available_at_audit" | "available_at_restart" | "available_at_recovery_rescan"
            | "available_at_playback_restore" => ("running_last_confirmed", "last_confirmed"),
            _ => ("unknown", "unconfirmed"),
        };
        format!(
            "[MirrorSave] final_strategy={} update_rejection={} outcome={} mpd_state={} mpd_availability={} mpd_last_confirmed={}",
            self.strategy.get(),
            self.rejection.get(),
            if result.is_ok() { "ok" } else { "error" },
            mpd_state,
            mpd_availability,
            self.mpd_last_confirmed.get()
        )
    }
}

fn should_start_mpd_on_startup(config_valid: bool, config: &AppConfig) -> bool {
    config_valid && !config.first_run
}

fn is_shared_output(config: &AppConfig) -> bool {
    config.audio_output_type == "pipewire"
        || config.audio_output_type == "shared"
        || config.alsa_device == "default"
}

fn validate_config_change_preflight(
    observed: &AppConfig,
    current: &AppConfig,
    proposed: &AppConfig,
    probe: impl FnOnce(&str) -> Result<(), alsa_preflight::PreflightError>,
) -> Result<(), String> {
    if current != observed {
        return Err(
            "A configuração mudou durante a preparação da troca de saída. Tente novamente."
                .to_string(),
        );
    }
    let current_direct = (!is_shared_output(current)).then_some(current.alsa_device.as_str());
    // Keep the existing Shared -> Direct gate, including legacy hw: spellings.
    // Only the new Direct -> Direct case uses proof of distinct ownership.
    let shared_to_direct = is_shared_output(current)
        && proposed.alsa_device.starts_with("hw:")
        && proposed.alsa_device.len() > 3;
    if !is_shared_output(proposed)
        && proposed.audio_output_type == "alsa"
        && (shared_to_direct
            || alsa_preflight::should_probe(&proposed.alsa_device, current_direct))
    {
        probe(&proposed.alsa_device).map_err(|error| error.code().to_string())?;
    }
    Ok(())
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

fn is_local_folders_only_change(current: &AppConfig, next: &AppConfig) -> bool {
    if !local_folders_changed(current, next) {
        return false;
    }
    let mut expected = current.clone();
    expected.local_folders = next.local_folders.clone();
    expected == *next
}

fn validate_config_save_scope(current: &AppConfig, next: &AppConfig) -> Result<(), String> {
    // Initial setup still applies its complete configuration in one operation.
    // Normal Saves must not route unpublished folders through an output switch.
    if !current.first_run
        && local_folders_changed(current, next)
        && !is_local_folders_only_change(current, next)
    {
        return Err("LOCAL_FOLDERS_MIXED_CONFIG_CHANGE".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryRefresh {
    Update,
    Rescan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LocalTagMpdRefresh {
    requested: bool,
    success: bool,
    job_id: Option<u64>,
    error_code: Option<TargetedLibraryUpdateErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LocalTrackTagUpdateResult {
    committed: bool,
    tags: tags::LocalTrackTags,
    mpd_refresh: LocalTagMpdRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LocalAlbumTagMpdRefreshState {
    NotNeeded,
    Accepted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LocalAlbumTagMpdRefresh {
    state: LocalAlbumTagMpdRefreshState,
    job_id: Option<u64>,
    error_code: Option<TargetedLibraryUpdateErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LocalAlbumTagBatchTrackResult {
    #[serde(flatten)]
    write: tags::LocalAlbumTagBatchTrackOutcome,
    mpd_refresh: LocalAlbumTagMpdRefresh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct LocalAlbumTagBatchResult {
    album_id: String,
    applicable: bool,
    blocked_reason: Option<tags::LocalAlbumTagBatchBlockedReason>,
    tracks: Vec<LocalAlbumTagBatchTrackResult>,
    committed_count: usize,
    unchanged_count: usize,
    failed_count: usize,
    not_attempted_count: usize,
    partial_commit: bool,
}

fn complete_local_tag_update<F>(
    relative_path: &str,
    write_result: Result<tags::LocalTrackTagWriteResult, tags::LocalTrackTagsError>,
    refresh: F,
) -> Result<LocalTrackTagUpdateResult, tags::LocalTrackTagsError>
where
    F: FnOnce(&str) -> Result<u64, TargetedLibraryUpdateErrorCode>,
{
    let write_result = write_result?;
    if !write_result.committed {
        return Ok(LocalTrackTagUpdateResult {
            committed: false,
            tags: write_result.tags,
            mpd_refresh: LocalTagMpdRefresh {
                requested: false,
                success: false,
                job_id: None,
                error_code: None,
            },
        });
    }

    let mpd_refresh = match refresh(relative_path) {
        Ok(job_id) => LocalTagMpdRefresh {
            requested: true,
            success: true,
            job_id: Some(job_id),
            error_code: None,
        },
        Err(error_code) => LocalTagMpdRefresh {
            requested: true,
            success: false,
            job_id: None,
            error_code: Some(error_code),
        },
    };
    Ok(LocalTrackTagUpdateResult {
        committed: true,
        tags: write_result.tags,
        mpd_refresh,
    })
}

fn complete_local_album_tag_batch<F>(
    mut write_result: tags::LocalAlbumTagBatchApplyResult,
    mut refresh: F,
) -> LocalAlbumTagBatchResult
where
    F: FnMut(&str) -> Result<u64, TargetedLibraryUpdateErrorCode>,
{
    write_result
        .tracks
        .sort_by(|left, right| left.uri.cmp(&right.uri));
    let tracks = write_result
        .tracks
        .into_iter()
        .map(|write| {
            let mpd_refresh = if write.state
                == tags::LocalAlbumTagBatchTrackOutcomeState::Committed
            {
                match refresh(&write.uri) {
                    Ok(job_id) => LocalAlbumTagMpdRefresh {
                        state: LocalAlbumTagMpdRefreshState::Accepted,
                        job_id: Some(job_id),
                        error_code: None,
                    },
                    Err(error_code) => LocalAlbumTagMpdRefresh {
                        state: LocalAlbumTagMpdRefreshState::Failed,
                        job_id: None,
                        error_code: Some(error_code),
                    },
                }
            } else {
                LocalAlbumTagMpdRefresh {
                    state: LocalAlbumTagMpdRefreshState::NotNeeded,
                    job_id: None,
                    error_code: None,
                }
            };
            LocalAlbumTagBatchTrackResult { write, mpd_refresh }
        })
        .collect();

    LocalAlbumTagBatchResult {
        album_id: write_result.album_id,
        applicable: write_result.applicable,
        blocked_reason: write_result.blocked_reason,
        tracks,
        committed_count: write_result.committed_count,
        unchanged_count: write_result.unchanged_count,
        failed_count: write_result.failed_count,
        not_attempted_count: write_result.not_attempted_count,
        partial_commit: write_result.partial_commit,
    }
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
    let resolved = local_media::resolve_local_path(library_dir, relative_path, local_folders)
        .map_err(|error| error.to_string())?;
    let metadata = std::fs::metadata(&resolved.physical_path)
        .map_err(|error| format!("Falha ao examinar caminho local: {error}"))?;
    if !metadata.is_dir() && !metadata.is_file() {
        return Err("O caminho local não é um arquivo ou diretório regular".to_string());
    }
    Ok(resolved.physical_path)
}

fn request_library_refresh(socket_path: &str, refresh: LibraryRefresh) -> Result<(), String> {
    match refresh {
        LibraryRefresh::Update => AudioEngine::update_library(socket_path),
        LibraryRefresh::Rescan => AudioEngine::rescan_library_at(socket_path),
    }
}

fn request_rescan_and_wait(socket_path: &str) -> Result<(), String> {
    request_rescan_and_wait_traced(socket_path, None)
}

fn request_rescan_and_wait_traced(
    socket_path: &str,
    diagnostics: Option<&SettingsSaveDiagnostics>,
) -> Result<(), String> {
    let started = Instant::now();
    let job = AudioEngine::request_mpd_rescan_job(socket_path);
    if let Some(diagnostics) = diagnostics {
        diagnostics.stage("refresh_request", started, if job.is_ok() { "ok" } else { "error" });
    }
    let job_id = job?;
    let started = Instant::now();
    let completion = AudioEngine::wait_for_mpd_update_job(
        socket_path,
        job_id,
        MANUAL_RESCAN_TIMEOUT,
        MANUAL_RESCAN_POLL_INTERVAL,
    );
    if let Some(diagnostics) = diagnostics {
        diagnostics.stage("refresh_wait", started, refresh_completion_code(&completion));
    }
    match completion {
        MpdRefreshCompletion::Completed => {}
        MpdRefreshCompletion::Timeout => {
            return Err("Tempo esgotado aguardando o rescan do MPD.".to_string())
        }
        MpdRefreshCompletion::TransportError(error) => {
            return Err(format!("Falha de transporte durante o rescan do MPD: {error}"))
        }
        MpdRefreshCompletion::ProtocolError(error) => {
            return Err(format!("Resposta inválida durante o rescan do MPD: {error}"))
        }
        MpdRefreshCompletion::Unavailable(error) => {
            return Err(format!("MPD indisponível durante o rescan: {error}"))
        }
    }
    Ok(())
}

fn refresh_completion_code(completion: &MpdRefreshCompletion) -> &'static str {
    match completion {
        MpdRefreshCompletion::Completed => "ok",
        MpdRefreshCompletion::Timeout => "timeout",
        MpdRefreshCompletion::TransportError(_) => "transport_error",
        MpdRefreshCompletion::ProtocolError(_) => "protocol_error",
        MpdRefreshCompletion::Unavailable(_) => "unavailable",
    }
}

fn describe_refresh_failure(completion: MpdRefreshCompletion) -> String {
    match completion {
        MpdRefreshCompletion::Completed => "refresh concluído".to_string(),
        MpdRefreshCompletion::Timeout => "tempo esgotado".to_string(),
        MpdRefreshCompletion::TransportError(error) => {
            format!("falha de transporte: {error}")
        }
        MpdRefreshCompletion::ProtocolError(error) => {
            format!("resposta inválida: {error}")
        }
        MpdRefreshCompletion::Unavailable(error) => format!("MPD indisponível: {error}"),
    }
}

fn request_rescan_wait_and_audit(socket_path: &str) -> Result<(), String> {
    request_rescan_wait_and_audit_traced(socket_path, None)
}

fn request_rescan_wait_and_audit_traced(
    socket_path: &str,
    diagnostics: Option<&SettingsSaveDiagnostics>,
) -> Result<(), String> {
    request_rescan_and_wait_traced(socket_path, diagnostics)?;
    let started = Instant::now();
    let audit_result = AudioEngine::audit_mpd_local_catalog_at(socket_path);
    if let Some(diagnostics) = diagnostics {
        let outcome = match &audit_result {
            Ok(audit) if audit.is_clean() => "ok",
            Ok(_) => "invalid_catalog",
            Err(_) => "error",
        };
        diagnostics.stage("audit", started, outcome);
        if outcome == "ok" {
            diagnostics.mpd_last_confirmed.set("available_at_audit");
        }
    }
    let audit = audit_result?;
    if !audit.is_clean() {
        return Err(format!(
            "A auditoria do catálogo encontrou {} URI(s) local(is) inválida(s).",
            audit.invalid_count
        ));
    }
    Ok(())
}

fn request_startup_refresh_and_audit(
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
) -> Result<(), String> {
    request_startup_refresh_and_audit_traced(socket_path, refresh_plan, None)
}

fn request_startup_refresh_and_audit_traced(
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
    diagnostics: Option<&SettingsSaveDiagnostics>,
) -> Result<(), String> {
    request_refresh_and_audit_traced(socket_path, refresh_plan, diagnostics, None)
}

fn wait_saved_folders_job(
    socket_path: &str,
    job_id: u64,
    operation: Option<&config_save_operation::Operation>,
) -> Result<MpdRefreshCompletion, String> {
    if let Some(operation) = operation {
        operation.phase(config_save_operation::Phase::Indexing);
        let _preview_job = operation.preview_job(job_id);
        AudioEngine::wait_for_managed_mpd_update(
            socket_path,
            Some(job_id),
            MANUAL_RESCAN_POLL_INTERVAL,
            || operation.shutdown_requested(),
        )
        .map_err(|_| config_save_operation::INTERRUPTED.to_string())
    } else {
        Ok(AudioEngine::wait_for_mpd_update_job(
            socket_path,
            job_id,
            MANUAL_RESCAN_TIMEOUT,
            MANUAL_RESCAN_POLL_INTERVAL,
        ))
    }
}

fn request_refresh_and_audit_traced(
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
    diagnostics: Option<&SettingsSaveDiagnostics>,
    operation: Option<&config_save_operation::Operation>,
) -> Result<(), String> {
    let mut refresh_duration = Duration::ZERO;
    let mut wait_duration = Duration::ZERO;
    let mut audit_duration = Duration::ZERO;
    let (strategy, directory_count) = match refresh_plan {
        local_media::mirror::ManagedRefreshPlan::Full => ("full", 0),
        local_media::mirror::ManagedRefreshPlan::Targeted { directories, .. } => {
            ("targeted", directories.len())
        }
    };
    let result = (|| {
        if let Some(operation) = operation {
            if operation.shutdown_requested() {
                return Err(config_save_operation::INTERRUPTED.to_string());
            }
        }
        match refresh_plan {
            local_media::mirror::ManagedRefreshPlan::Full => {
                let started = Instant::now();
                let job_id = AudioEngine::request_mpd_rescan_job(socket_path);
                refresh_duration += started.elapsed();
                if let Some(diagnostics) = diagnostics {
                    diagnostics.stage("refresh_request", started, if job_id.is_ok() { "ok" } else { "error" });
                }
                let job_id = job_id?;
                let started = Instant::now();
                let completion = wait_saved_folders_job(socket_path, job_id, operation)?;
                wait_duration += started.elapsed();
                if let Some(diagnostics) = diagnostics {
                    diagnostics.stage("refresh_wait", started, refresh_completion_code(&completion));
                }
                if completion != MpdRefreshCompletion::Completed {
                    return Err(format!(
                        "Falha durante o rescan completo da biblioteca: {}",
                        describe_refresh_failure(completion)
                    ));
                }
            }
            local_media::mirror::ManagedRefreshPlan::Targeted { directories, .. } => {
                for directory in directories {
                    if let Some(operation) = operation {
                        if operation.shutdown_requested() {
                            return Err(config_save_operation::INTERRUPTED.to_string());
                        }
                    }
                    let started = Instant::now();
                    let job_id = AudioEngine::update_library_path(socket_path, directory);
                    refresh_duration += started.elapsed();
                    if let Some(diagnostics) = diagnostics {
                        diagnostics.stage("refresh_request", started, if job_id.is_ok() { "ok" } else { "error" });
                    }
                    let job_id = job_id.map_err(|error| {
                        format!("Falha ao solicitar update MPD direcionado: {error:?}")
                    })?;
                    let started = Instant::now();
                    let completion = wait_saved_folders_job(socket_path, job_id, operation)?;
                    wait_duration += started.elapsed();
                    if let Some(diagnostics) = diagnostics {
                        diagnostics.stage("refresh_wait", started, refresh_completion_code(&completion));
                    }
                    if completion != MpdRefreshCompletion::Completed {
                        return Err(format!(
                            "Falha durante update MPD direcionado: {}",
                            describe_refresh_failure(completion)
                        ));
                    }
                }
            }
        }

        if let Some(operation) = operation {
            if operation.shutdown_requested() {
                return Err(config_save_operation::INTERRUPTED.to_string());
            }
            operation.phase(config_save_operation::Phase::Validating);
        }
        let started = Instant::now();
        let audit = AudioEngine::audit_mpd_local_catalog_at(socket_path);
        audit_duration += started.elapsed();
        if let Some(diagnostics) = diagnostics {
            let outcome = match &audit {
                Ok(audit) if audit.is_clean() => "ok",
                Ok(_) => "invalid_catalog",
                Err(_) => "error",
            };
            diagnostics.stage("audit", started, outcome);
            if outcome == "ok" {
                diagnostics.mpd_last_confirmed.set("available_at_audit");
            }
        }
        let audit = audit?;
        if !audit.is_clean() {
            return Err(format!(
                "A auditoria do catálogo encontrou {} URI(s) local(is) inválida(s).",
                audit.invalid_count
            ));
        }
        Ok(())
    })();
    eprintln!(
        "[Mirror] managed_refresh={} directories={} refresh_ms={} wait_ms={} audit_ms={} outcome={}",
        strategy,
        directory_count,
        refresh_duration.as_millis(),
        wait_duration.as_millis(),
        audit_duration.as_millis(),
        if result.is_ok() { "ok" } else { "error" }
    );
    result
}

#[cfg(test)]
fn refresh_saved_folders(
    socket_path: &str,
    plan: &local_media::mirror::ManagedRefreshPlan,
    previous_kind: local_media::mirror::ManagedMirrorPreviousKind,
    diagnostics: Option<&SettingsSaveDiagnostics>,
) -> Result<(), String> {
    match (previous_kind, plan) {
        (local_media::mirror::ManagedMirrorPreviousKind::Legacy, _) => {
            request_rescan_wait_and_audit_traced(socket_path, diagnostics)
        }
        (_, local_media::mirror::ManagedRefreshPlan::Targeted { .. }) => {
            request_startup_refresh_and_audit_traced(socket_path, plan, diagnostics)
        }
        (_, local_media::mirror::ManagedRefreshPlan::Full) => {
            request_rescan_wait_and_audit_traced(socket_path, diagnostics)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RestorePreviousOutcome {
    Restored,
    PlaybackIncomplete(String),
    SkippedForShutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedMirrorRefreshMode {
    Manual,
    Check,
    Startup,
    Filesystem,
}

fn finish_unchanged_managed_generation(
    mode: ManagedMirrorRefreshMode,
    socket_path: &str,
) -> Result<bool, String> {
    match mode {
        ManagedMirrorRefreshMode::Manual => {
            request_rescan_wait_and_audit(socket_path)?;
            Ok(true)
        }
        ManagedMirrorRefreshMode::Check
        | ManagedMirrorRefreshMode::Startup
        | ManagedMirrorRefreshMode::Filesystem => Ok(false),
    }
}

fn refresh_published_managed_generation(
    mode: ManagedMirrorRefreshMode,
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
) -> Result<(), String> {
    match mode {
        ManagedMirrorRefreshMode::Manual => request_rescan_wait_and_audit(socket_path),
        ManagedMirrorRefreshMode::Check
        | ManagedMirrorRefreshMode::Startup
        | ManagedMirrorRefreshMode::Filesystem => {
            request_startup_refresh_and_audit(socket_path, refresh_plan)
        }
    }
}

fn complete_published_mirror_refresh<
    Refresh,
    Capture,
    RecoverySnapshot,
    Stop,
    Rollback,
    Restore,
    CleanupRejected,
    Commit,
    CleanupPrevious,
>(
    mut refresh_and_audit: Refresh,
    mut capture_current_playback: Capture,
    mut stop_controlled_mpd: Stop,
    mut rollback_mirror: Rollback,
    mut restore_previous: Restore,
    mut cleanup_rejected: CleanupRejected,
    mut commit: Commit,
    mut cleanup_previous: CleanupPrevious,
) -> Result<(), String>
where
    Refresh: FnMut() -> Result<(), String>,
    Capture: FnMut() -> Result<RecoverySnapshot, String>,
    Stop: FnMut() -> Result<(), String>,
    Rollback: FnMut() -> Result<(), String>,
    Restore: FnMut(Result<RecoverySnapshot, String>) -> Result<RestorePreviousOutcome, String>,
    CleanupRejected: FnMut() -> Result<(), String>,
    Commit: FnMut() -> Result<(), ConfigSaveError>,
    CleanupPrevious: FnMut() -> Result<(), String>,
{
    let primary_error = match refresh_and_audit() {
        Ok(()) => match commit() {
            Ok(()) => {
                return cleanup_previous().map_err(|error| {
                    format!(
                        "A transação foi confirmada, mas a geração anterior não pôde ser limpa: {error}"
                    )
                })
            }
            Err(error @ ConfigSaveError::AfterRenameUnconfirmed(_)) => {
                return Err(uncertain_config_save_message(&error));
            }
            Err(error @ ConfigSaveError::BeforeRename(_)) => {
                format!(
                    "O catálogo foi atualizado, mas a transação não pôde ser confirmada: {error}"
                )
            }
        },
        Err(error) => error,
    };

    let recovery_snapshot = capture_current_playback();
    if let Err(stop_error) = stop_controlled_mpd() {
        return Err(format!(
            "{primary_error}; recuperação incompleta: não foi possível confirmar o encerramento do MPD controlado ({stop_error}); o mirror não foi revertido."
        ));
    }
    if let Err(rollback_error) = rollback_mirror() {
        return Err(format!(
            "{primary_error}; recuperação falhou: o MPD foi encerrado e está indisponível; o mirror não pôde ser revertido ({rollback_error})."
        ));
    }
    let restore_outcome = match restore_previous(recovery_snapshot) {
        Ok(outcome) => outcome,
        Err(restore_error) => {
            return Err(format!(
                "{primary_error}; o mirror anterior foi revertido, mas a recuperação do MPD, catálogo ou reprodução falhou ({restore_error})."
            ))
        }
    };
    if let Err(cleanup_error) = cleanup_rejected() {
        return match &restore_outcome {
            RestorePreviousOutcome::PlaybackIncomplete(error) => Err(format!(
                "{primary_error}; mirror revertido, MPD reiniciado e catálogo anterior atualizado (auditado se managed); reprodução não restaurada ({error}); a geração rejeitada não pôde ser limpa ({cleanup_error})."
            )),
            RestorePreviousOutcome::Restored => Err(format!(
                "{primary_error}; mirror revertido, MPD reiniciado e catálogo anterior atualizado (auditado se managed), reprodução restaurada; a geração rejeitada não pôde ser limpa ({cleanup_error})."
            )),
            RestorePreviousOutcome::SkippedForShutdown => Err(format!(
                "{primary_error}; mirror revertido, mas o MPD, catálogo e reprodução não foram recuperados devido ao encerramento da aplicação; a geração rejeitada não pôde ser limpa ({cleanup_error})."
            )),
        };
    }
    match restore_outcome {
        RestorePreviousOutcome::Restored => Err(format!(
            "{primary_error}; mirror revertido, MPD reiniciado e catálogo anterior atualizado (auditado se managed), reprodução restaurada."
        )),
        RestorePreviousOutcome::PlaybackIncomplete(error) => Err(format!(
            "{primary_error}; mirror revertido, MPD reiniciado e catálogo anterior atualizado (auditado se managed), mas a reprodução não foi restaurada ({error})."
        )),
        RestorePreviousOutcome::SkippedForShutdown => Err(format!(
            "{primary_error}; mirror revertido e geração rejeitada limpa, mas MPD, catálogo e reprodução não foram recuperados porque a aplicação está sendo encerrada."
        )),
    }
}

fn stop_supervisor(supervisor: &Mutex<MpdSupervisor>) -> Result<(), String> {
    supervisor
        .lock()
        .map_err(|error| format!("Falha ao acessar o supervisor MPD: {error}"))?
        .stop()
}

fn report_failed_previous_catalog_refresh(
    error: String,
    stop_result: Result<(), String>,
) -> String {
    match stop_result {
        Ok(()) => format!(
            "MPD reiniciado, mas o catálogo anterior não pôde ser atualizado e validado ({error}); o MPD de recuperação foi encerrado e está indisponível; reprodução não restaurada."
        ),
        Err(stop_error) => format!(
            "MPD reiniciado, mas o catálogo anterior não pôde ser atualizado e validado ({error}); falha ao confirmar o encerramento do MPD de recuperação ({stop_error}); disponibilidade do motor indeterminada e reprodução não restaurada."
        ),
    }
}

fn restore_previous_after_manual_rescan(
    transaction_state: &ConfigTransactionState,
    supervisor: &Mutex<MpdSupervisor>,
    config: &AppConfig,
    socket_path: &str,
    snapshot: Result<MpdPlaybackSnapshot, String>,
    previous_kind: local_media::mirror::ManagedMirrorPreviousKind,
    diagnostics: Option<&SettingsSaveDiagnostics>,
) -> Result<RestorePreviousOutcome, String> {
    let Some(_restart_permit) = transaction_state.try_begin_restart() else {
        if let Some(diagnostics) = diagnostics {
            diagnostics.stage("restart", Instant::now(), "skipped_shutdown");
        }
        return Ok(RestorePreviousOutcome::SkippedForShutdown);
    };
    let started = Instant::now();
    let restart = supervisor
        .lock()
        .map_err(|error| {
            format!(
                "Falha de recuperação: MPD indisponível; não foi possível acessar o supervisor para reinício ({error})"
            )
        })?
        .start(config)
        .map_err(|error| {
            format!(
                "Falha de recuperação: MPD anterior não pôde ser reiniciado e validado; motor indisponível ({error})"
            )
        });
    if let Some(diagnostics) = diagnostics {
        diagnostics.stage("restart", started, if restart.is_ok() { "ok" } else { "error" });
        diagnostics.mpd_last_confirmed.set(if restart.is_ok() {
            "available_at_restart"
        } else {
            "unknown_after_restart_error"
        });
    }
    restart?;

    let started = Instant::now();
    let refresh = match previous_kind {
        local_media::mirror::ManagedMirrorPreviousKind::Legacy => {
            request_rescan_and_wait(socket_path)
        }
        local_media::mirror::ManagedMirrorPreviousKind::Managed => {
            request_rescan_wait_and_audit(socket_path)
        }
    };
    if let Some(diagnostics) = diagnostics {
        diagnostics.stage("recovery_rescan", started, if refresh.is_ok() { "ok" } else { "error" });
        if refresh.is_ok() {
            diagnostics.mpd_last_confirmed.set("available_at_recovery_rescan");
        }
    }
    if let Err(error) = refresh {
        let started = Instant::now();
        let stop_result = stop_supervisor(supervisor);
        if let Some(diagnostics) = diagnostics {
            diagnostics.stage("recovery_stop", started, if stop_result.is_ok() { "ok" } else { "error" });
            diagnostics.mpd_last_confirmed.set(if stop_result.is_ok() {
                "stopped"
            } else {
                "unknown_after_stop_error"
            });
        }
        return Err(report_failed_previous_catalog_refresh(
            error,
            stop_result,
        ));
    }
    match snapshot {
        Ok(snapshot) => {
            let started = Instant::now();
            let restored = AudioEngine::restore_mpd_playback_snapshot(socket_path, &snapshot);
            if let Some(diagnostics) = diagnostics {
                diagnostics.stage("playback_restore", started, if restored.is_ok() { "ok" } else { "error" });
                diagnostics.mpd_last_confirmed.set(if restored.is_ok() {
                    "available_at_playback_restore"
                } else {
                    "unknown_after_playback_error"
                });
            }
            restored.map_err(|error| {
                format!("MPD reiniciado e catálogo anterior atualizado (auditado se managed), mas a reprodução não pôde ser restaurada; disponibilidade atual do MPD não confirmada ({error})")
            })?;
            Ok(RestorePreviousOutcome::Restored)
        }
        Err(error) => {
            if let Some(diagnostics) = diagnostics {
                diagnostics.stage("playback_restore", Instant::now(), "skipped_capture_error");
            }
            Ok(RestorePreviousOutcome::PlaybackIncomplete(format!(
                "não foi possível capturar o estado atual antes do stop: {error}"
            )))
        }
    }
}

fn run_managed_manual_rescan(app: &AppHandle) -> Result<(), String> {
    run_managed_mirror_refresh(app, ManagedMirrorRefreshMode::Manual).map(|_| ())
}

fn run_managed_mirror_refresh(
    app: &AppHandle,
    mode: ManagedMirrorRefreshMode,
) -> Result<bool, String> {
    let transaction_state = app.state::<ConfigTransactionState>();
    let mut config_read_guard = transaction_state.lock_startup_config_reads(mode)?;
    let _transaction_guard = transaction_state.begin()?;
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    let config = app
        .state::<ConfigState>()
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar pastas locais: {error}"))?
        .clone();
    if let Some(guard) = config_read_guard.as_mut() {
        guard.allow_reads(&MpdSupervisor::sonante_config_dir())?;
    }
    // Revoke ownership before releasing the transaction, also on error/unwind.
    let _config_read_guard = config_read_guard;
    let socket_path = app
        .state::<AudioState>()
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar a biblioteca local: {error}"))?
        .local_album_socket_path();
    let live_path = MpdSupervisor::library_dir();
    let staging_path = live_path.with_file_name("library.next");
    let roots = config
        .local_folders
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let generation_id = format!(
        "{}-{}-{}",
        match mode {
            ManagedMirrorRefreshMode::Manual => "manual-rescan",
            ManagedMirrorRefreshMode::Check => "manual-check",
            ManagedMirrorRefreshMode::Startup => "startup-reconcile",
            ManagedMirrorRefreshMode::Filesystem => "filesystem-reconcile",
        },
        std::process::id(),
        MIRROR_GENERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let preparation = local_media::mirror::prepare_managed_generation(
        &live_path,
        &staging_path,
        &roots,
        &generation_id,
    )?;
    let local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) = preparation else {
        return finish_unchanged_managed_generation(mode, &socket_path);
    };
    if transaction_state.shutdown_requested() {
        return Err(error_with_rollback(
            "A reconciliação foi cancelada antes da publicação porque a aplicação está sendo encerrada."
                .to_string(),
            local_media::mirror::cleanup_managed_generation(&prepared.staging_path),
        ));
    }
    if mode != ManagedMirrorRefreshMode::Manual {
        match AudioEngine::wait_for_mpd_idle(
            &socket_path,
            MANUAL_RESCAN_TIMEOUT,
            MANUAL_RESCAN_POLL_INTERVAL,
        ) {
            MpdRefreshCompletion::Completed => {}
            completion => {
                return Err(error_with_rollback(
                    format!(
                        "Falha ao aguardar o refresh MPD já em andamento: {}",
                        describe_refresh_failure(completion)
                    ),
                    local_media::mirror::cleanup_managed_generation(&prepared.staging_path),
                ))
            }
        }
    }
    let refresh_plan = prepared.refresh_plan.clone();
    let publication = match local_media::mirror::publish_prepared_managed_generation(
        &live_path,
        &prepared,
    ) {
        Ok(publication) => publication,
        Err(error) => {
            return Err(error_with_rollback(
                format!("Falha ao publicar o mirror preparado: {error}"),
                local_media::mirror::cleanup_managed_generation(&prepared.staging_path),
            ))
        }
    };
    if transaction_state.shutdown_requested() {
        let rollback = publication
            .rollback()
            .and_then(|_| publication.cleanup_rejected_after_rollback());
        return Err(error_with_rollback(
            "A reconciliação foi cancelada após a publicação porque a aplicação está sendo encerrada."
                .to_string(),
            rollback,
        ));
    }
    let supervisor_state = app.state::<SupervisorState>();
    let previous_kind = publication.previous_kind();
    complete_published_mirror_refresh(
        || refresh_published_managed_generation(mode, &socket_path, &refresh_plan),
        || AudioEngine::capture_mpd_playback_snapshot(&socket_path),
        || stop_supervisor(&supervisor_state.0),
        || publication.rollback(),
        |snapshot| {
            match previous_kind {
                Some(previous_kind) => restore_previous_after_manual_rescan(
                    &transaction_state,
                    &supervisor_state.0,
                    &config,
                    &socket_path,
                    snapshot,
                    previous_kind,
                    None,
                ),
                None => Err(
                    "A primeira publicação não possui geração anterior para restaurar."
                        .to_string(),
                ),
            }
        },
        || publication.cleanup_rejected_after_rollback(),
        || Ok(()),
        || publication.cleanup_previous(),
    )
    .map(|_| true)
}

fn persist_experimental_local_folders_config(
    app: &AppHandle,
    expected_current: &AppConfig,
    new_config: &AppConfig,
) -> Result<(), ConfigSaveError> {
    let config_state = app.state::<ConfigState>();
    let plex_state = app.state::<PlexState>();
    let mut config_guard = config_state
        .0
        .lock()
        .map_err(|error| format!("Falha ao publicar a nova configuração: {error}"))?;
    if *config_guard != *expected_current {
        return Err(
            "A configuração mudou durante a transação das pastas locais. Tente novamente."
                .to_string()
                .into(),
        );
    }
    let mut plex_guard = plex_state
        .0
        .lock()
        .map_err(|error| format!("Falha ao atualizar a configuração do Plex: {error}"))?;
    if let Err(error) = new_config.save() {
        return Err(config_save_failure_with_rollback(error, || {
            expected_current.save()
        }));
    }
    plex_guard.update_config(new_config);
    *config_guard = new_config.clone();
    Ok(())
}

fn run_experimental_local_folders_save(
    app: &AppHandle,
    observed_config: &AppConfig,
    new_config: &AppConfig,
    operation: &config_save_operation::Operation,
    queue_clear_authorization: Option<&QueueClearAuthorization>,
) -> (Result<(), String>, bool) {
    let diagnostics = SettingsSaveDiagnostics::new();
    let uncertain = Cell::new(false);
    let result = run_experimental_local_folders_save_inner(
        app,
        observed_config,
        new_config,
        &diagnostics,
        operation,
        &uncertain,
        queue_clear_authorization,
    );
    diagnostics.finish(&result);
    (result, uncertain.get())
}

fn added_source_save_needs_targeted_refresh(
    previous: &local_media::ResolvedLocalSources,
    prepared: &local_media::mirror::MirrorBuildResult,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
) -> Result<bool, &'static str> {
    let added = prepared
        .manifest
        .sources
        .values()
        .any(|root| !previous.sources().values().any(|existing| existing == root));
    if added
        && (previous.topology() != local_media::LocalLibraryTopology::Managed
            || !matches!(refresh_plan, local_media::mirror::ManagedRefreshPlan::Targeted { .. }))
    {
        Err(ADDED_SOURCE_FULL_RESCAN_REQUIRED)
    } else {
        Ok(added)
    }
}

fn reject_added_source_save_with_cleanup(
    reason: &'static str,
    cleanup: impl FnOnce() -> Result<(), String>,
) -> String {
    match cleanup() {
        Ok(()) => reason.to_string(),
        Err(_) => format!("{reason};{ADDED_SOURCE_STAGING_CLEANUP_FAILED}"),
    }
}

fn check_managed_save_shutdown(operation: &config_save_operation::Operation) -> Result<(), String> {
    if operation.shutdown_requested() {
        Err(format!(
            "{}; {}: intenção e gerações preservadas.",
            config_save_operation::INTERRUPTED,
            managed_save_intent::BLOCKED
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone)]
struct RemovalCatalogExpectation(std::collections::HashSet<String>);

impl RemovalCatalogExpectation {
    fn from_before(
        plan: &local_media::mirror::PureSourceRemoval,
        before: std::collections::HashSet<String>,
    ) -> Option<Self> {
        let mut remaining = std::collections::HashSet::new();
        for record in before {
            let uri = record.split_once(": ")?.1;
            let id = uri.split('/').next()?;
            if plan.remaining_ids.iter().any(|known| known == id) {
                remaining.insert(record);
            } else if !plan.removed_ids.iter().any(|known| known == id) {
                return None;
            }
        }
        Some(Self(remaining))
    }

    fn validate(&self, socket_path: &str) -> Result<(), String> {
        if AudioEngine::read_validated_local_catalog_at(socket_path)? != self.0 {
            return Err("LOCAL_SOURCE_REMOVAL_CATALOG_MISMATCH".to_string());
        }
        Ok(())
    }
}

struct RemovalRefreshExpectation {
    catalog: RemovalCatalogExpectation,
    queue: audio::MpdQueueStructure,
}

/// Backend-only opt-in. The first refused Save supplies a binding in its coded
/// error; resubmission must explicitly confirm that exact queue/source proposal.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueClearAuthorization {
    confirmed: bool,
    binding: String,
}

struct ConfirmedQueueRemoval {
    owner: audio::QueueStructureReservation,
    queue: audio::MpdQueueStructure,
    backup: managed_save_intent::QueueBackup,
    tracks: Vec<TrackMetadata>,
    playback: MpdPlaybackSnapshot,
    logical_resume: Option<audio::ReservedLogicalQueueSnapshot>,
    is_shared: bool,
    catalog_before: std::collections::HashSet<String>,
    catalog_after: RemovalCatalogExpectation,
}

fn queue_references_removed_sources(
    plan: &local_media::mirror::PureSourceRemoval,
    logical: &[TrackMetadata],
    queue: &audio::MpdQueueStructure,
) -> bool {
    let affected = |uri: &str| {
        uri.split_once('/')
            .is_some_and(|(id, _)| plan.removed_ids.iter().any(|removed| removed == id))
    };
    logical.iter().any(|track| {
        affected(&track.uri)
            || matches!(&track.media_locator,
        Some(MediaLocator::Local { uri }) if affected(uri))
    }) || queue.entries.iter().any(|entry| affected(&entry.uri))
}

fn queue_clear_binding(
    previous: &AppConfig,
    next: &AppConfig,
    plan: &local_media::mirror::PureSourceRemoval,
    backup: &managed_save_intent::QueueBackup,
) -> Result<String, String> {
    // Playback may advance naturally during confirmation. Bind structural state,
    // occurrences/IDs and exact configs/sources, not wall-clock elapsed/cursor.
    let bytes = serde_json::to_vec(&(
        previous,
        next,
        &plan.removed_ids,
        &plan.remaining_ids,
        &backup.tracks,
        &backup.mpd,
        backup.playlist_version,
    ))
    .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn admit_confirmed_queue_removal(
    audio: &Mutex<AudioEngine>,
    socket_path: &str,
    previous: &AppConfig,
    next: &AppConfig,
    plan: &local_media::mirror::PureSourceRemoval,
    authorization: Option<&QueueClearAuthorization>,
) -> Result<Option<ConfirmedQueueRemoval>, String> {
    let (owner, tracks, had_logical_resume, logical_resume) = {
        let mut audio = audio
            .lock()
            .map_err(|_| "Estado de áudio indisponível na remoção local.")?;
        let Some(owner) = audio.reserve_queue_structure() else {
            // Without ownership, neither queue can be proven unaffected. Do not
            // fall through to publication/rescan with potentially affected items.
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE".into());
        };
        let tracks = audio.get_queue();
        let had_logical_resume = audio.queue_has_logical_resume();
        // Fence the cursor in the same critical section as the logical capture.
        // Proven unaffected queues keep their existing nonstructural controls.
        let logical_resume = if had_logical_resume
            && queue_references_removed_sources(
                plan,
                &tracks,
                &audio::MpdQueueStructure {
                    entries: Vec::new(),
                    playlist_version: 0,
                },
            ) {
            Some(
                audio
                    .capture_reserved_logical_queue(&owner)
                    .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED")?,
            )
        } else {
            None
        };
        (owner, tracks, had_logical_resume, logical_resume)
    };
    let queue = AudioEngine::capture_mpd_queue_structure(socket_path).map_err(|e| e.to_string())?;
    if !queue_references_removed_sources(plan, &tracks, &queue) {
        return Ok(None);
    }
    if had_logical_resume {
        let captured = logical_resume
            .as_ref()
            .ok_or("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED")?;
        // Do not discard a preserved cursor on a remaining source just because
        // another occurrence in the queue belongs to a removed source.
        let current = captured
            .tracks
            .get(captured.resume.queue_index)
            .ok_or("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED")?;
        if !queue_references_removed_sources(
            plan,
            std::slice::from_ref(current),
            &audio::MpdQueueStructure {
                entries: Vec::new(),
                playlist_version: 0,
            },
        ) {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED".into());
        }
    }
    let catalog_before = AudioEngine::read_validated_local_catalog_at(socket_path)?;
    let Some(catalog_after) = RemovalCatalogExpectation::from_before(plan, catalog_before.clone())
    else {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED".into());
    };
    let mut all = plan.clone();
    all.remaining_ids.extend(plan.removed_ids.iter().cloned());
    if !removal_queue_matches_remaining_sources(
        &all,
        &tracks,
        &queue,
        &RemovalCatalogExpectation(catalog_before.clone()),
    ) {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED".into());
    }
    let playback = AudioEngine::capture_mpd_playback_snapshot(socket_path)?;
    if playback.state == audio::PlaybackState::Stopped
        && (playback.current_index.is_some() || playback.elapsed != 0.0)
    {
        // The existing stopped restore cannot prove this cursor without starting
        // audio. Refuse before creating an intention or changing either queue.
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED".into());
    }
    let backup = if let Some(captured) = &logical_resume {
        managed_save_intent::QueueBackup::new_with_logical_resume(
            &tracks,
            &playback.uris,
            &queue,
            &playback,
            managed_save_intent::BackupLogicalResume {
                queue_index: captured.resume.queue_index,
                elapsed: captured.resume.elapsed,
                state: captured.resume.state,
            },
        )
        .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED")?
    } else {
        let backup =
            managed_save_intent::QueueBackup::new(&tracks, &playback.uris, &queue, &playback)?;
        let binding = queue_clear_binding(previous, next, plan, &backup)?;
        match authorization {
            None => {
                return Err(format!(
                    "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED:{binding}"
                ))
            }
            Some(auth) if !auth.confirmed || auth.binding != binding => {
                return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into())
            }
            Some(_) => {}
        }
        backup
    };
    if !removal_queue_before_publish(socket_path, &queue)? {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
    }
    Ok(Some(ConfirmedQueueRemoval {
        owner,
        queue,
        backup,
        tracks,
        playback,
        logical_resume,
        is_shared: is_shared_output(previous),
        catalog_before,
        catalog_after,
    }))
}

fn clear_confirmed_removal_queue(
    confirmed: &mut ConfirmedQueueRemoval,
    audio: &Mutex<AudioEngine>,
    intent: &mut managed_save_intent::SaveIntent,
    operation: &config_save_operation::Operation,
) -> Result<(), String> {
    let (effect, socket) = {
        let mut audio = audio
            .lock()
            .map_err(|_| "Estado de áudio indisponível na limpeza confirmada.")?;
        if let Some(captured) = &confirmed.logical_resume {
            let current = audio.capture_reserved_logical_queue(&confirmed.owner)?;
            if current.resume != captured.resume || current.tracks != captured.tracks {
                return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
            }
        }
        let socket = audio.local_album_socket_path();
        (
            audio.begin_reserved_queue_effect(&mut confirmed.owner)?,
            socket,
        )
    };
    check_managed_save_shutdown(operation)?;
    intent.before_exchange()?; // Revalidate the durable backup/config/generations before clear.
    if confirmed.logical_resume.is_some()
        && AudioEngine::capture_mpd_playback_snapshot(&socket)
            .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED")?
            != confirmed.playback
    {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
    }
    let (empty, bytes) = effect.clear_reserved_queue(&confirmed.queue)?;
    audio
        .lock()
        .map_err(|_| "Estado de áudio indisponível na limpeza confirmada.")?
        .apply_verified_reserved_queue(&confirmed.owner, Vec::new())?;
    confirmed.queue = empty;
    if let Err(error) = intent.persist_queue_cache(&bytes) {
        // Publication has not happened. Restore against the proven previous
        // catalog when possible, but a persistence error still retains the
        // intention, both generations and the structural fence as Uncertain.
        let recovery = (|| {
            check_managed_save_shutdown(operation)?;
            intent.before_exchange()?;
            audio
                .lock()
                .map_err(|_| "Estado de áudio indisponível na recuperação.")?
                .begin_reserved_queue_effect(&mut confirmed.owner)?;
            let socket = audio
                .lock()
                .map_err(|_| "Estado de áudio indisponível na recuperação.")?
                .local_album_socket_path();
            if AudioEngine::read_validated_local_catalog_at(&socket)? != confirmed.catalog_before {
                return Err("LOCAL_SOURCE_REMOVAL_PREVIOUS_CATALOG_MISMATCH".into());
            }
            AudioEngine::restore_mpd_playback_snapshot(&socket, &confirmed.playback)?;
            // No output changed: use the original configuration's classification.
            confirm_removed_queue_recovery(confirmed, audio, &socket, intent, confirmed.is_shared)?;
            intent.before_exchange()
        })();
        match audio.lock() {
            Ok(mut audio) => audio.retain_uncertain_queue_reservation(&mut confirmed.owner),
            Err(_) => eprintln!("[SettingsSave] queue_recovery=unconfirmed audio_state=unavailable reservation=retained"),
        }
        return Err(match recovery {
            Ok(()) => format!(
                "{error}; fila anterior recuperada e validada; conclusão persistente incerta."
            ),
            Err(recovery_error) => {
                format!("{error}; recuperação da fila não confirmada: {recovery_error}")
            }
        });
    }
    check_managed_save_shutdown(operation)
}

fn confirm_removed_queue_recovery(
    confirmed: &mut ConfirmedQueueRemoval,
    audio: &Mutex<AudioEngine>,
    socket_path: &str,
    intent: &mut managed_save_intent::SaveIntent,
    is_shared: bool,
) -> Result<(), String> {
    let effect = audio
        .lock()
        .map_err(|_| "Estado de áudio indisponível na recuperação.")?
        .begin_reserved_queue_effect(&mut confirmed.owner)?;
    if AudioEngine::read_validated_local_catalog_at(socket_path)? != confirmed.catalog_before {
        return Err("LOCAL_SOURCE_REMOVAL_PREVIOUS_CATALOG_MISMATCH".into());
    }
    let bytes = if let Some(captured) = &confirmed.logical_resume {
        let verified =
            effect.confirm_reserved_logical_queue_restore(captured, &confirmed.playback)?;
        audio
            .lock()
            .map_err(|_| "Estado de áudio indisponível na recuperação.")?
            .apply_verified_reserved_logical_queue(&confirmed.owner, verified)?
    } else {
        let (tracks, bytes) = effect.confirm_reserved_queue_restore(
            confirmed.tracks.clone(),
            &confirmed.playback,
            is_shared,
        )?;
        audio
            .lock()
            .map_err(|_| "Estado de áudio indisponível na recuperação.")?
            .apply_verified_reserved_queue(&confirmed.owner, tracks)?;
        bytes
    };
    intent.persist_queue_cache(&bytes)
}

fn require_confirmed_queue_recovery_for_cleanup(
    recovered: bool,
    persistence_failed: bool,
) -> Result<(), String> {
    if !recovered || persistence_failed {
        return Err(
            "LOCAL_SOURCE_REMOVAL_QUEUE_COMPLETION_UNCONFIRMED: intenção e gerações preservadas."
                .into(),
        );
    }
    Ok(())
}

fn removal_queue_matches_remaining_sources(
    plan: &local_media::mirror::PureSourceRemoval,
    logical: &[TrackMetadata],
    queue: &audio::MpdQueueStructure,
    catalog: &RemovalCatalogExpectation,
) -> bool {
    logical.len() == queue.entries.len() && logical.iter().zip(&queue.entries).all(|(track, entry)| {
        let uri = match &track.media_locator {
            Some(MediaLocator::Local { uri }) if uri == &track.uri => uri.as_str(),
            None => track.uri.as_str(),
            _ => return false, // Plex/contradictory identities are not proven managed sources.
        };
        uri == entry.uri && local_media::is_safe_relative_mpd_uri(uri)
            && uri.split_once('/').is_some_and(|(id, _)| plan.remaining_ids.iter().any(|known| known == id))
            && catalog.0.contains(&format!("file: {uri}"))
    })
}

fn removal_queue_before_publish(
    socket_path: &str,
    expected: &audio::MpdQueueStructure,
) -> Result<bool, String> {
    match AudioEngine::capture_mpd_queue_structure(socket_path) {
        Ok(actual) => Ok(actual == *expected),
        Err(audio::MpdQueueStructureError::Invalid | audio::MpdQueueStructureError::Changed) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn admit_removal_refresh(
    audio: &Mutex<AudioEngine>,
    socket_path: &str,
    plan: &local_media::mirror::PureSourceRemoval,
) -> Result<Option<(audio::QueueStructureReservation, RemovalRefreshExpectation)>, String> {
    let (reservation, logical) = {
        let mut guard = audio.lock().map_err(|_| "Estado de áudio indisponível na remoção local.".to_string())?;
        let Some(reservation) = guard.reserve_queue_structure() else { return Ok(None) };
        (reservation, guard.get_queue())
    };
    let queue = match AudioEngine::capture_mpd_queue_structure(socket_path) {
        Ok(queue) => queue,
        Err(audio::MpdQueueStructureError::Invalid | audio::MpdQueueStructureError::Changed) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if logical.len() != queue.entries.len() { return Ok(None); }
    let before = AudioEngine::read_validated_local_catalog_at(socket_path)?;
    let Some(catalog) = RemovalCatalogExpectation::from_before(plan, before) else { return Ok(None) };
    if !removal_queue_matches_remaining_sources(plan, &logical, &queue, &catalog)
        || !removal_queue_before_publish(socket_path, &queue)?
    {
        return Ok(None);
    }
    Ok(Some((reservation, RemovalRefreshExpectation { catalog, queue })))
}

fn request_removal_refresh_and_validate(
    socket_path: &str,
    plan: &local_media::mirror::PureSourceRemoval,
    expected: &RemovalRefreshExpectation,
    diagnostics: Option<&SettingsSaveDiagnostics>,
    operation: Option<&config_save_operation::Operation>,
) -> Result<(), String> {
    request_refresh_and_audit_traced(socket_path, &plan.refresh_plan, diagnostics, operation)?;
    let started = Instant::now();
    let result = expected.catalog.validate(socket_path).and_then(|_| {
        let queue = AudioEngine::capture_mpd_queue_structure(socket_path).map_err(|error| error.to_string())?;
        if queue != expected.queue {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".to_string());
        }
        Ok(())
    });
    if let Some(diagnostics) = diagnostics {
        diagnostics.stage("removal_validation", started, if result.is_ok() { "ok" } else { "error" });
    }
    result
}

fn run_experimental_local_folders_save_inner(
    app: &AppHandle,
    observed_config: &AppConfig,
    new_config: &AppConfig,
    diagnostics: &SettingsSaveDiagnostics,
    operation: &config_save_operation::Operation,
    uncertain: &Cell<bool>,
    queue_clear_authorization: Option<&QueueClearAuthorization>,
) -> Result<(), String> {
    let transaction_state = app.state::<ConfigTransactionState>();
    // The operation owns the transaction slot from admission through durable
    // completion. Reacquiring begin() here would race shutdown or self-deadlock.
    if operation.shutdown_requested() { return Err(config_save_operation::INTERRUPTED.to_string()); }
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    let current_config = app
        .state::<ConfigState>()
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar a configuração atual: {error}"))?
        .clone();
    if current_config != *observed_config {
        return Err(
            "A configuração mudou durante a preparação das pastas locais. Tente novamente."
                .to_string(),
        );
    }
    if !is_local_folders_only_change(&current_config, new_config) {
        return Err("A transação experimental aceita somente alteração de pastas locais.".to_string());
    }

    let live_path = MpdSupervisor::library_dir();
    let previous_sources = local_media::resolve_local_sources(&live_path, &current_config.local_folders)
        .map_err(|error| format!("A geração local anterior não pôde ser validada: {error}"))?;
    let staging_path = live_path.with_file_name("library.next");
    let roots = new_config
        .local_folders
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let generation_id = format!(
        "folders-save-{}-{}",
        std::process::id(),
        MIRROR_GENERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let preparation = local_media::mirror::prepare_managed_generation(
        &live_path,
        &staging_path,
        &roots,
        &generation_id,
    )?;
    let local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) = preparation else {
        diagnostics.rejection.set("equivalent_generation");
        return persist_experimental_local_folders_config(
            app,
            &current_config,
            new_config,
        )
        .map_err(|error| {
            uncertain.set(matches!(&error, ConfigSaveError::AfterRenameUnconfirmed(_)));
            config_save_error_message(&error)
        });
    };

    if operation.shutdown_requested() {
        return Err(error_with_rollback(
            config_save_operation::INTERRUPTED.to_string(),
            local_media::mirror::cleanup_prepared_managed_generation(&prepared),
        ));
    }

    let mut removal_plan = None;
    let mut refresh_plan = match local_media::mirror::plan_added_sources_refresh_diagnostic(&live_path, &prepared) {
        Ok((Some(plan), reason)) => {
            diagnostics.rejection.set(reason);
            plan
        }
        Ok((None, reason)) => {
            diagnostics.rejection.set(reason);
            match local_media::mirror::plan_removed_sources_refresh(&live_path, &prepared) {
                Ok(Some(plan)) => {
                    let refresh = plan.refresh_plan.clone();
                    removal_plan = Some(plan);
                    diagnostics.rejection.set("none");
                    refresh
                }
                Ok(None) => local_media::mirror::ManagedRefreshPlan::Full,
                Err(error) => return Err(error_with_rollback(error,
                    local_media::mirror::cleanup_prepared_managed_generation(&prepared))),
            }
        }
        Err(error) => {
            diagnostics.rejection.set("planning_error");
            return Err(error_with_rollback(
                format!("Falha ao planejar refresh das novas fontes: {error}"),
                local_media::mirror::cleanup_prepared_managed_generation(&prepared),
            ))
        }
    };

    let added_sources = match added_source_save_needs_targeted_refresh(
        &previous_sources,
        &prepared,
        &refresh_plan,
    ) {
        Ok(added) => added,
        Err(_) => {
            diagnostics.strategy.set("rejected_before_publish");
            return Err(reject_added_source_save_with_cleanup(ADDED_SOURCE_FULL_RESCAN_REQUIRED, || {
                local_media::mirror::cleanup_prepared_managed_generation(&prepared)
            }));
        }
    };
    diagnostics.strategy.set(if matches!(refresh_plan, local_media::mirror::ManagedRefreshPlan::Targeted { .. }) {
        "update_targeted"
    } else {
        "rescan"
    });

    let audio_state = app.state::<AudioState>();
    let socket_path = audio_state
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar a biblioteca local: {error}"))?
        .local_album_socket_path();
    if matches!(refresh_plan, local_media::mirror::ManagedRefreshPlan::Targeted { .. }) {
        let started = Instant::now();
        operation.phase(config_save_operation::Phase::Indexing);
        let completion = match AudioEngine::wait_for_managed_mpd_update(
            &socket_path, None, MANUAL_RESCAN_POLL_INTERVAL, || operation.shutdown_requested(),
        ) {
            Ok(completion) => completion,
            Err(()) => return Err(error_with_rollback(
                config_save_operation::INTERRUPTED.to_string(),
                local_media::mirror::cleanup_prepared_managed_generation(&prepared),
            )),
        };
        diagnostics.stage("pre_update_idle_wait", started, refresh_completion_code(&completion));
        match completion {
            MpdRefreshCompletion::Completed => {}
            completion => {
                return Err(error_with_rollback(
                    format!(
                        "Falha ao aguardar o refresh MPD antes de adicionar fontes: {}",
                        describe_refresh_failure(completion)
                    ),
                    local_media::mirror::cleanup_prepared_managed_generation(&prepared),
                ))
            }
        }
    }
    if operation.shutdown_requested() {
        return Err(error_with_rollback(
            config_save_operation::INTERRUPTED.to_string(),
            local_media::mirror::cleanup_prepared_managed_generation(&prepared),
        ));
    }
    // Reserve the proven unaffected queue structure through publication
    // and catalog validation, without holding AudioState during I/O or job waits.
    // Release at the same boundary BEFORE commit or recovery.
    let mut confirmed_removal = None;
    let removal_guard = match removal_plan.as_ref() {
        Some(plan) => match admit_removal_refresh(&audio_state.0, &socket_path, plan) {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                match admit_confirmed_queue_removal(
                    &audio_state.0,
                    &socket_path,
                    &current_config,
                    new_config,
                    plan,
                    queue_clear_authorization,
                ) {
                    Ok(Some(confirmed)) => confirmed_removal = Some(confirmed),
                    Ok(None) => {
                        removal_plan = None;
                        refresh_plan = local_media::mirror::ManagedRefreshPlan::Full;
                        diagnostics.rejection.set("removal_queue_or_catalog_unsafe");
                        diagnostics.strategy.set("rescan");
                    }
                    Err(error) => {
                        return Err(error_with_rollback(
                            error,
                            local_media::mirror::cleanup_prepared_managed_generation(&prepared),
                        ))
                    }
                }
                None
            }
            Err(error) => return Err(error_with_rollback(error,
                local_media::mirror::cleanup_prepared_managed_generation(&prepared))),
        },
        None => None,
    };
    if queue_clear_authorization.is_some() && removal_plan.is_none() {
        return Err(error_with_rollback(
            "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into(),
            local_media::mirror::cleanup_prepared_managed_generation(&prepared),
        ));
    }
    let mut intent = match &confirmed_removal {
        Some(confirmed) => managed_save_intent::SaveIntent::create_with_queue_backup(
            &MpdSupervisor::sonante_config_dir(),
            &current_config,
            new_config,
            &prepared,
            &confirmed.backup,
        ),
        None => managed_save_intent::SaveIntent::create(
            &MpdSupervisor::sonante_config_dir(),
            &current_config,
            new_config,
            &prepared,
        ),
    }?;
    if let Some(confirmed) = confirmed_removal.as_mut() {
        // Arm before any destructive effect; panic/early return keeps the fence.
        clear_confirmed_removal_queue(confirmed, &audio_state.0, &mut intent, operation)?;
    }
    // Capture only descriptors/metadata before publication. No image bytes are
    // read until the exact generation and its source identities are revalidated.
    let pending_preview_artwork = if added_sources
        && previous_sources.topology() == local_media::LocalLibraryTopology::Managed
    {
        match &refresh_plan {
            local_media::mirror::ManagedRefreshPlan::Targeted { directories, .. } => {
                local_import_artwork::ArtworkScope::new(&staging_path, &prepared.manifest, directories)
            }
            _ => None,
        }
    } else {
        None
    };
    let started = Instant::now();
    let publication_result = if let Some(plan) = &removal_plan {
        intent.publish_removal(&prepared, plan)
    } else if added_sources {
        intent.publish(&prepared, Some(&refresh_plan))
    } else {
        intent.publish(&prepared, None)
    };
    diagnostics.stage(
        "mirror_publish",
        started,
        if publication_result.is_ok() { "ok" } else { "error" },
    );
    let publication = match publication_result {
        Ok(publication) => publication,
        Err(error) => {
            if added_sources {
                diagnostics.strategy.set("rejected_before_publish");
            }
            if error == local_media::mirror::TARGETED_ADDITION_PLAN_CHANGED {
                diagnostics.rejection.set("plan_changed_before_publish");
            }
            // An intent exists now: preserve both generations even if publication
            // was refused. Startup must not infer recovery from this error text.
            return Err(format!(
                "{error}; {}: intenção e staging preservadas.",
                managed_save_intent::BLOCKED,
            ));
        }
    };
    let Some(previous_kind) = publication.previous_kind() else {
        if confirmed_removal.is_some() {
            return Err("LOCAL_SOURCE_REMOVAL_PREVIOUS_GENERATION_UNCONFIRMED: intenção e gerações preservadas.".into());
        }
        let original =
            "A alteração de pastas locais perdeu a geração anterior durante a publicação."
                .to_string();
        let started = Instant::now();
        let rollback = publication.rollback();
        diagnostics.stage(
            "mirror_rollback",
            started,
            if rollback.is_ok() { "ok" } else { "error" },
        );
        let rollback = rollback.and_then(|_| publication.cleanup_rejected_after_rollback());
        return Err(error_with_rollback(original, rollback));
    };
    let supervisor_state = app.state::<SupervisorState>();
    let recovery_validated = Cell::new(false);
    let completed = Cell::new(None);
    let managed_plan = if previous_kind == local_media::mirror::ManagedMirrorPreviousKind::Legacy {
        &local_media::mirror::ManagedRefreshPlan::Full
    } else { &refresh_plan };
    if added_sources {
        if let local_media::mirror::ManagedRefreshPlan::Targeted { directories, .. } = managed_plan {
            if operation.enable_addition_preview(&socket_path, directories).is_err() {
                // Preview limits never change the admitted Save transaction.
                eprintln!("[MirrorSave] provisional_preview=unavailable");
            } else {
                operation.enable_preview_artwork(pending_preview_artwork.and_then(|scope| {
                    scope.published_at(&live_path, publication.preview_generation_identity())
                }));
            }
        }
    }
    let refresh_result = match (&removal_plan, &removal_guard, &confirmed_removal) {
        (Some(plan), Some((_, proof)), _) => request_removal_refresh_and_validate(
            &socket_path, plan, proof, Some(diagnostics), Some(operation),
        ),
        (Some(plan), _, Some(confirmed)) => request_removal_refresh_and_validate(
            &socket_path,
            plan,
            &RemovalRefreshExpectation {
                catalog: confirmed.catalog_after.clone(),
                queue: confirmed.queue.clone(),
            },
            Some(diagnostics),
            Some(operation),
        ),
        _ => request_refresh_and_audit_traced(
            &socket_path, managed_plan, Some(diagnostics), Some(operation),
        ),
    };
    drop(removal_guard);
    let refresh_result = if confirmed_removal.is_some() {
        refresh_result.and_then(|_| intent.check_queue_cache_completion())
    } else {
        refresh_result
    };
    // A published generation with a pending/unaudited catalog cannot be
    // rolled back or cleaned as though it were recovered. Keep the intent
    // and both generations; coordinated shutdown stops the owned MPD later.
    check_managed_save_shutdown(operation)?;
    let mut refresh_result = Some(refresh_result);
    let destructive = confirmed_removal.is_some();
    let original_snapshot = confirmed_removal
        .as_ref()
        .map(|confirmed| confirmed.playback.clone());
    let config_persistence_failed = Cell::new(false);
    let result = complete_published_mirror_refresh(
        || refresh_result.take().unwrap_or_else(|| Err("Refresh já consumido.".to_string())),
        || match &original_snapshot {
            Some(snapshot) => Ok(snapshot.clone()),
            None => AudioEngine::capture_mpd_playback_snapshot(&socket_path),
        },
        || {
            let started = Instant::now();
            let result = stop_supervisor(&supervisor_state.0);
            diagnostics.stage("stop", started, if result.is_ok() { "ok" } else { "error" });
            diagnostics.mpd_last_confirmed.set(if result.is_ok() {
                "stopped"
            } else {
                "unknown_after_stop_error"
            });
            result
        },
        || {
            let started = Instant::now();
            let result = publication.rollback();
            diagnostics.stage(
                "mirror_rollback",
                started,
                if result.is_ok() { "ok" } else { "error" },
            );
            result
        },
        |snapshot| {
            if let Some(confirmed) = confirmed_removal.as_mut() {
                audio_state
                    .0
                    .lock()
                    .map_err(|_| "Estado de áudio indisponível na recuperação.")?
                    .begin_reserved_queue_effect(&mut confirmed.owner)?;
            }
            let result = restore_previous_after_manual_rescan(
                &transaction_state,
                &supervisor_state.0,
                &current_config,
                &socket_path,
                snapshot,
                previous_kind,
                Some(diagnostics),
            );
            let result = result.and_then(|outcome| {
                if let (RestorePreviousOutcome::Restored, Some(confirmed)) =
                    (&outcome, confirmed_removal.as_mut())
                {
                    confirm_removed_queue_recovery(
                        confirmed,
                        &audio_state.0,
                        &socket_path,
                        &mut intent,
                        is_shared_output(&current_config),
                    )?;
                }
                Ok(outcome)
            });
            recovery_validated.set(matches!(&result, Ok(RestorePreviousOutcome::Restored)));
            result
        },
        || {
            if destructive {
                require_confirmed_queue_recovery_for_cleanup(
                    recovery_validated.get(),
                    config_persistence_failed.get(),
                )?;
            }
            publication.cleanup_rejected_after_rollback()?;
            if recovery_validated.get() {
                completed.set(Some(managed_save_intent::Completed::Recovered));
            }
            Ok(())
        },
        || {
            let result =
                persist_experimental_local_folders_config(app, &current_config, new_config);
            if result.is_err() {
                config_persistence_failed.set(true);
            }
            result
        },
        || {
            publication.cleanup_previous()?;
            completed.set(Some(managed_save_intent::Completed::Committed));
            Ok(())
        },
    );
    let (result, unconfirmed) = intent.conclude_transaction(result, completed.get());
    uncertain.set(unconfirmed);
    if let Some(confirmed) = confirmed_removal.as_mut() {
        if unconfirmed {
            match audio_state.0.lock() {
                Ok(mut audio) => audio.retain_uncertain_queue_reservation(&mut confirmed.owner),
                Err(_) => eprintln!("[SettingsSave] queue_recovery=unconfirmed audio_state=unavailable reservation=retained"),
            }
        } else {
            confirmed.owner.release_after_confirmed_completion();
        }
    }
    result
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
    fn new() -> Self {
        Self {
            operations: Arc::new(config_save_operation::Coordinator::new()),
            startup_config_read_gate: Mutex::new(false),
            restart_gate: Mutex::new(()),
            preparation: managed_preparation::ManagedPreparationCoordinator::new(),
            lifecycle: AtomicU8::new(0),
            shutdown_worker_started: AtomicBool::new(false),
            shutdown_complete: AtomicBool::new(false),
        }
    }

    fn begin(&self) -> Result<config_save_operation::Transaction<'_>, String> {
        self.operations.begin()
    }

    fn lock_startup_config_reads(
        &self,
        mode: ManagedMirrorRefreshMode,
    ) -> Result<Option<StartupConfigReadGuard<'_>>, String> {
        if mode != ManagedMirrorRefreshMode::Startup {
            return Ok(None);
        }
        let admission = self
            .startup_config_read_gate
            .lock()
            .map_err(|_| "Controle de leitura da configuração indisponível.".to_string())?;
        if *admission {
            return Err(config_save_operation::BUSY.to_string());
        }
        Ok(Some(StartupConfigReadGuard {
            state: &self.startup_config_read_gate,
            admission: Some(admission),
        }))
    }

    fn shutdown_requested(&self) -> bool {
        self.lifecycle.load(Ordering::SeqCst) & SHUTDOWN_REQUESTED != 0
    }

    fn request_shutdown(&self) {
        self.preparation.begin_shutdown_with(|| {
            self.operations.request_shutdown(|| {
                self.lifecycle.fetch_or(SHUTDOWN_REQUESTED, Ordering::SeqCst);
            });
        });
    }

    fn begin_effective_shutdown(&self) {
        let _restart_gate = self
            .restart_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.lifecycle.fetch_or(SHUTDOWN_STARTED, Ordering::SeqCst);
    }

    fn shutdown_started(&self) -> bool {
        self.lifecycle.load(Ordering::SeqCst) & SHUTDOWN_STARTED != 0
    }

    fn start_shutdown_worker(&self) -> bool {
        !self.shutdown_worker_started.swap(true, Ordering::SeqCst)
    }

    fn wait_for_active_transaction(&self) -> config_save_operation::Transaction<'_> {
        self.operations.wait_for_idle()
    }

    fn try_begin_restart(&self) -> Option<RestartPermit<'_>> {
        let guard = self
            .restart_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.shutdown_requested() || self.shutdown_started() {
            None
        } else {
            Some(RestartPermit { _guard: guard })
        }
    }

    fn mark_shutdown_complete(&self) {
        self.shutdown_complete.store(true, Ordering::SeqCst);
    }

    fn shutdown_complete(&self) -> bool {
        self.shutdown_complete.load(Ordering::SeqCst)
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
fn toggle_playback(
    config_state: State<ConfigState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante Play/Resume: {e}"))?;
        is_shared_output(&config)
    };
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar reprodução durante Play/Resume: {e}"))?
        .toggle_play_pause(is_shared)
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

async fn resolve_device_switch_queue(
    audio: &Mutex<AudioEngine>,
    plex_client: &PlexClient,
) -> Result<ResolvedDeviceSwitchQueue, String> {
    let captured = audio
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila antes da troca de saída: {}", e))?
        .capture_device_switch_queue()?;
    let uris = resolve_playback_uris(captured.tracks(), plex_client).await?;
    captured.bind_resolved_uris(uris)
}

async fn restore_cached_queue_after_ready(app: AppHandle) {
    let audio_state = app.state::<AudioState>();
    let queue = match audio_state.0.lock() {
        Ok(audio) if audio.startup_queue_pending() => audio.get_queue(),
        Ok(_) => return,
        Err(_) => {
            eprintln!("[Audio] Failed to access queue for startup restoration.");
            return;
        }
    };
    let plex_client = match app.state::<PlexState>().0.lock() {
        Ok(plex) => plex.clone(),
        Err(_) => {
            eprintln!("[Audio] Failed to access Plex for startup restoration.");
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
                    eprintln!(
                        "[Audio] Failed to restore MPD queue: {}",
                        crate::diagnostics::error_summary(&error)
                    );
                }
            }
            Err(_) => eprintln!("[Audio] Failed to access MPD for startup restoration."),
        },
        Err(_) => {
            eprintln!("[Audio] Failed to resolve media for startup restoration.");
            if let Ok(mut audio) = audio_state.0.lock() {
                audio.fail_startup_queue_restore();
            }
        }
    }
}

fn start_local_watcher(app: &AppHandle) {
    let roots = match app.state::<ConfigState>().0.lock() {
        Ok(config) => config.local_folders.iter().map(PathBuf::from).collect(),
        Err(error) => {
            eprintln!(
                "[Watcher] Failed to access configured local sources: {}",
                crate::diagnostics::error_summary(&error)
            );
            return;
        }
    };
    let watcher_app = app.clone();
    if let Err(error) = app.state::<LocalWatcherState>().start(roots, move || {
        run_managed_mirror_refresh(&watcher_app, ManagedMirrorRefreshMode::Filesystem)?;
        watcher_app
            .emit(LOCAL_LIBRARY_RECONCILED_EVENT, ())
            .map_err(|error| format!("Falha ao notificar catálogo local reconciliado: {error}"))
    }) {
        eprintln!(
            "[Watcher] Failed to start local monitoring: {}",
            crate::diagnostics::error_summary(&error)
        );
    }
}

async fn complete_startup_after_ready(app: AppHandle) {
    restore_cached_queue_after_ready(app.clone()).await;
    if app
        .state::<ConfigTransactionState>()
        .shutdown_requested()
    {
        return;
    }
    start_local_watcher(&app);
    if app
        .state::<ConfigTransactionState>()
        .shutdown_requested()
    {
        app.state::<LocalWatcherState>().stop_and_join();
        return;
    }
    let reconciliation_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        run_managed_mirror_refresh(&reconciliation_app, ManagedMirrorRefreshMode::Startup)
    })
    .await
    {
        Ok(Ok(_)) => {
            if let Err(error) = app.emit(LOCAL_LIBRARY_RECONCILED_EVENT, ()) {
                eprintln!(
                    "[Mirror] Failed to notify reconciled local catalog: {}",
                    crate::diagnostics::error_summary(&error)
                );
            }
        }
        Ok(Err(error)) => {
            eprintln!(
                "[Mirror] Automatic startup reconciliation failed: {}",
                crate::diagnostics::error_summary(&error)
            )
        }
        Err(error) => eprintln!(
            "[Mirror] Automatic startup reconciliation task failed: {}",
            crate::diagnostics::error_summary(&error)
        ),
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
fn play_queue_index(
    index: usize,
    config_state: State<ConfigState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante seleção da fila: {e}"))?;
        is_shared_output(&config)
    };
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar reprodução durante seleção da fila: {e}"))?
        .play_index(index, is_shared)
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
async fn list_local_directory(
    path: String,
    state: State<'_, AudioState>,
) -> Result<Vec<audio::LocalItem>, String> {
    list_local_directory_from_audio(path, &state.0).await
}

async fn list_local_directory_from_audio(
    path: String,
    audio: &Mutex<AudioEngine>,
) -> Result<Vec<audio::LocalItem>, String> {
    let socket_path = audio
        .lock()
        .map_err(|e| format!("Falha ao acessar conexão da biblioteca local: {e}"))?
        .local_album_socket_path();
    tauri::async_runtime::spawn_blocking(move || {
        AudioEngine::list_directory_at(&socket_path, &path)
    })
    .await
    .map_err(|e| format!("Falha na tarefa de listagem da biblioteca local: {e}"))?
}

#[tauri::command]
async fn get_local_album_tracks(
    album_id: String,
    state: State<'_, AudioState>,
) -> Result<Vec<audio::LocalAlbumDiscTracks>, String> {
    let socket_path = state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar conexão da biblioteca local: {e}"))?
        .local_album_socket_path();
    tauri::async_runtime::spawn_blocking(move || {
        AudioEngine::get_local_album_tracks_at(&socket_path, &album_id)?
            .ok_or_else(|| "Álbum local não encontrado no catálogo atual.".to_string())
    })
    .await
    .map_err(|e| format!("Falha na tarefa de consulta das faixas do álbum local: {e}"))?
}

#[tauri::command]
async fn get_local_album_doctor(
    album_id: String,
    config_state: State<'_, ConfigState>,
    audio_state: State<'_, AudioState>,
) -> Result<library_doctor::Report, library_doctor::DoctorError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| library_doctor::DoctorError::StateUnavailable)?
        .local_folders
        .clone();
    let socket_path = audio_state
        .0
        .lock()
        .map_err(|_| library_doctor::DoctorError::StateUnavailable)?
        .local_album_socket_path();
    // Both shared locks are released before catalog and filesystem I/O.
    tauri::async_runtime::spawn_blocking(move || {
        library_doctor::inspect_album_at(
            &socket_path,
            &MpdSupervisor::library_dir(),
            &album_id,
            &local_folders,
        )
    })
    .await
    .map_err(|_| library_doctor::DoctorError::TaskFailed)?
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
async fn get_local_track_tags(
    path: String,
    config_state: State<'_, ConfigState>,
) -> Result<tags::LocalTrackTags, tags::LocalTrackTagsError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalTrackTagsError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    tauri::async_runtime::spawn_blocking(move || {
        tags::inspect_local_track_tags(&library_dir, &path, &local_folders)
    })
    .await
    .map_err(|_| tags::LocalTrackTagsError::task_failed())?
}

#[tauri::command]
async fn get_local_album_tag_inspection(
    album_id: String,
    config_state: State<'_, ConfigState>,
) -> Result<tags::LocalAlbumTagInspection, tags::LocalTrackTagsError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalTrackTagsError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let track_uris = AudioEngine::get_local_album_track_uris_at(&socket_path, &album_id)
            .map_err(|error| {
                eprintln!(
                    "[Tags] Failed to resolve local catalog for album metadata inspection: {}",
                    crate::diagnostics::error_summary(&error)
                );
                tags::LocalTrackTagsError::album_catalog_failed()
            })?
            .ok_or_else(tags::LocalTrackTagsError::album_not_found)?;
        Ok(tags::inspect_local_album_tags(
            &library_dir,
            &album_id,
            track_uris,
            &local_folders,
        ))
    })
    .await
    .map_err(|_| tags::LocalTrackTagsError::task_failed())?
}

#[tauri::command]
async fn preflight_local_album_tag_batch(
    request: tags::LocalAlbumTagBatchPreflightRequest,
    config_state: State<'_, ConfigState>,
) -> Result<tags::LocalAlbumTagBatchPlan, tags::LocalAlbumTagBatchPreflightError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let track_uris = AudioEngine::get_local_album_track_uris_at(
            &socket_path,
            &request.album_id,
        )
        .map_err(|error| {
            eprintln!(
                "[Tags] Failed to resolve local catalog for album metadata preflight: {}",
                crate::diagnostics::error_summary(&error)
            );
            tags::LocalAlbumTagBatchPreflightError::album_catalog_failed()
        })?
        .ok_or_else(tags::LocalAlbumTagBatchPreflightError::album_not_found)?;
        tags::preflight_local_album_tag_batch(
            &library_dir,
            request,
            track_uris,
            &local_folders,
        )
    })
    .await
    .map_err(|_| tags::LocalAlbumTagBatchPreflightError::task_failed())?
}

#[tauri::command]
async fn apply_local_album_tag_batch(
    request: tags::LocalAlbumTagBatchPreflightRequest,
    config_state: State<'_, ConfigState>,
) -> Result<LocalAlbumTagBatchResult, tags::LocalAlbumTagBatchPreflightError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    let write_result = tauri::async_runtime::spawn_blocking(move || {
        let track_uris = AudioEngine::get_local_album_track_uris_at(
            &socket_path,
            &request.album_id,
        )
        .map_err(|error| {
            eprintln!(
                "[Tags] Failed to resolve local catalog for album metadata application: {}",
                crate::diagnostics::error_summary(&error)
            );
            tags::LocalAlbumTagBatchPreflightError::album_catalog_failed()
        })?
        .ok_or_else(tags::LocalAlbumTagBatchPreflightError::album_not_found)?;
        tags::apply_local_album_tag_batch(
            &library_dir,
            request,
            track_uris,
            &local_folders,
        )
    })
    .await
    .map_err(|_| tags::LocalAlbumTagBatchPreflightError::task_failed())??;

    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    let fallback = write_result.clone();
    match tauri::async_runtime::spawn_blocking(move || {
        complete_local_album_tag_batch(write_result, |uri| {
            let result = AudioEngine::update_library_path(&socket_path, uri);
            if let Err(error) = result {
                eprintln!("[Tags] Targeted MPD refresh failed after album tag commit: {error:?}");
            }
            result
        })
    })
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            eprintln!(
                "[Tags] Targeted album MPD refresh task failed after commits: {}",
                crate::diagnostics::error_summary(&error)
            );
            Ok(complete_local_album_tag_batch(fallback, |_| {
                Err(TargetedLibraryUpdateErrorCode::RefreshTaskFailed)
            }))
        }
    }
}

#[tauri::command]
async fn update_local_track_tags(
    path: String,
    expected_revision: String,
    patch: tags::LocalTrackTagUpdate,
    config_state: State<'_, ConfigState>,
) -> Result<LocalTrackTagUpdateResult, tags::LocalTrackTagsError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalTrackTagsError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    let refresh_path = path.clone();
    let write_result = tauri::async_runtime::spawn_blocking(move || {
        tags::update_local_track_tags(
            &library_dir,
            &path,
            &local_folders,
            &expected_revision,
            &patch,
        )
    })
    .await
    .map_err(|_| tags::LocalTrackTagsError::task_failed())?;

    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    let refresh_result = if write_result
        .as_ref()
        .is_ok_and(|result| result.committed)
    {
        let refresh_uri = refresh_path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            AudioEngine::update_library_path(&socket_path, &refresh_uri)
        })
        .await
        .unwrap_or(Err(TargetedLibraryUpdateErrorCode::RefreshTaskFailed))
    } else {
        Err(TargetedLibraryUpdateErrorCode::RefreshTaskFailed)
    };
    complete_local_tag_update(&refresh_path, write_result, |_| refresh_result)
}

#[tauri::command]
fn get_favorites() -> Result<Vec<FavoriteAlbum>, String> {
    let config = match AppConfig::load() {
        Ok(config) => Some(config),
        Err(error) => {
            eprintln!(
                "[Favorites] Configuration unavailable; artwork preserved: {}",
                crate::diagnostics::error_summary(&error)
            );
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
                        eprintln!("[Playlist] Failed to verify local item {} in MPD.", item.id);
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
                            eprintln!("[Playlist] Failed to resolve Plex item {}.", item.id);
                            None
                        }
                    },
                    PlexMediaAvailability::Missing => None,
                    PlexMediaAvailability::Unavailable(_) => {
                        eprintln!(
                            "[Playlist] Plex item {} unavailable on this attempt.",
                            item.id
                        );
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
async fn get_local_cover(
    path: String,
    config_state: State<'_, ConfigState>,
) -> Result<Option<String>, String> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| "Configuração indisponível.".to_string())?
        .local_folders
        .clone();
    let lib_dir = supervisor::MpdSupervisor::library_dir();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move || {
        AudioEngine::get_local_cover_at(&socket_path, &lib_dir, &path, &local_folders)
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
async fn pick_directory() -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title("Selecionar Pasta de Músicas")
        .pick_folder()
        .await
        .map(|p| p.path().to_string_lossy().to_string())
}

#[tauri::command]
async fn check_local_library(app: AppHandle) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        run_managed_mirror_refresh(&app, ManagedMirrorRefreshMode::Check)
    })
    .await
    .map_err(|error| format!("Falha na tarefa de verificação da biblioteca: {error}"))?
}

#[tauri::command]
async fn rescan_library(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || run_managed_manual_rescan(&app))
        .await
        .map_err(|error| format!("Falha na tarefa de rescan da biblioteca: {error}"))?
}

#[tauri::command]
async fn get_config(app: AppHandle) -> Result<AppConfig, String> {
    tauri::async_runtime::spawn_blocking(move || {
        read_config_after_startup_reconciliation(
            &app.state::<ConfigState>(),
            &app.state::<ConfigTransactionState>(),
            &MpdSupervisor::sonante_config_dir(),
        )
    })
    .await
    .map_err(|error| format!("Falha na tarefa de leitura da configuração: {error}"))?
}

fn read_config_after_startup_reconciliation(
    state: &ConfigState,
    transactions: &ConfigTransactionState,
    config_dir: &Path,
) -> Result<AppConfig, String> {
    let startup_owned = transactions
        .startup_config_read_gate
        .lock()
        .map_err(|_| "Controle de leitura da configuração indisponível.".to_string())?;
    if transactions
        .operations
        .snapshot()?
        .is_some_and(|op| op.phase == config_save_operation::Phase::Uncertain)
    {
        return Err(
            "CONFIG_PERSISTENCE_UNCERTAIN: conclusão do Save não confirmada.".to_string(),
        );
    }
    if !transactions.operations.active() {
        if *startup_owned {
            managed_save_intent::check_active_startup_reconciliation(config_dir)?;
        } else {
            managed_save_intent::check_startup(config_dir)?;
        }
        AppConfig::load_from_path(&config_dir.join("config.json"))?;
    }
    Ok(state
        .0
        .lock()
        .map_err(|_| "Falha ao acessar a configuração atual.".to_string())?
        .clone())
}

#[tauri::command]
fn get_config_save_operation(
    state: State<ConfigTransactionState>,
) -> Result<Option<config_save_operation::Snapshot>, String> {
    state.operations.snapshot()
}

#[tauri::command]
async fn get_local_import_preview(
    operation_id: String,
    state: State<'_, ConfigTransactionState>,
) -> Result<local_import_preview::Discovery, local_import_preview::PreviewError> {
    // No source IDs or socket supplied by the caller. Admission also prevents
    // overlapping reads, including queued spawn_blocking tasks.
    let lease = state.operations.reserve_preview(&operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        local_import_preview::read(&operation_id, &lease)
    }).await.map_err(|_| local_import_preview::PreviewError::ReadFailed)?
}

#[tauri::command]
fn get_audio_devices() -> Vec<AudioDevice> {
    list_audio_devices()
}

fn check_direct_device_availability_with(
    device_id: &str,
    transactions: &ConfigTransactionState,
    config_state: &Mutex<AppConfig>,
    enumerate: impl FnOnce() -> Vec<String>,
    probe: impl FnMut(&str) -> Result<(), alsa_preflight::PreflightError>,
) -> Result<Vec<alsa_preflight::DeviceAvailability>, String> {
    // Serialize with Save/shutdown without holding a Mutex across ALSA I/O.
    let _guard = transactions
        .begin()
        .map_err(|_| "ALSA_AVAILABILITY_UNCONFIRMED")?;
    let config = config_state
        .lock()
        .map_err(|_| "ALSA_AVAILABILITY_UNCONFIRMED")?
        .clone();
    let current_direct =
        (!config.first_run && config.audio_output_type == "alsa" && !is_shared_output(&config))
            .then_some(config.alsa_device.as_str());
    // Enumerate server-side: IPC callers cannot request arbitrary PCM names.
    let devices = enumerate();
    if transactions.shutdown_requested() {
        return Err("ALSA_AVAILABILITY_UNCONFIRMED".into());
    }
    let result = alsa_preflight::availability_for_with(device_id, &devices, current_direct, probe)
        .map_err(str::to_string)?;
    if transactions.shutdown_requested() {
        return Err("ALSA_AVAILABILITY_UNCONFIRMED".into());
    }
    Ok(result)
}

#[tauri::command]
async fn check_direct_device_availability(
    device_id: String,
    app: AppHandle,
) -> Result<Vec<alsa_preflight::DeviceAvailability>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        check_direct_device_availability_with(
            &device_id,
            &app.state::<ConfigTransactionState>(),
            &app.state::<ConfigState>().0,
            || list_audio_devices().into_iter().map(|d| d.id).collect(),
            alsa_preflight::probe,
        )
    })
    .await
    .map_err(|_| "ALSA_AVAILABILITY_UNCONFIRMED".to_string())?
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
    queue_clear_authorization: Option<QueueClearAuthorization>,
    app: AppHandle,
    config_state: State<'_, ConfigState>,
    transaction_state: State<'_, ConfigTransactionState>,
    plex_state: State<'_, PlexState>,
    sup_state: State<'_, SupervisorState>,
    audio_state: State<'_, AudioState>,
    analyzer_state: State<'_, AnalyzerState>,
) -> Result<config_save_operation::SaveResponse, String> {
    transaction_state.operations.reject_active()?;
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    AppConfig::load()?;
    let observed_cfg = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a configuração atual: {}", e))?
        .clone();

    validate_config_save_scope(&observed_cfg, &new_config)?;

    if !observed_cfg.first_run
        && is_local_folders_only_change(&observed_cfg, &new_config)
    {
        let worker_app = app.clone();
        // The owning reservation fences other mutations and shutdown BEFORE
        // spawn/acceptance. Its Drop also closes a panicked/abandoned task as uncertain.
        let (accepted, operation) = transaction_state.operations.reserve()?;
        let task = tauri::async_runtime::spawn_blocking(move || {
            let (result, persistence_uncertain) = run_experimental_local_folders_save(
                &worker_app, &observed_cfg, &new_config, &operation,
                queue_clear_authorization.as_ref(),
            );
            let uncertain = persistence_uncertain || (result.is_err()
                && managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir()).is_err());
            operation.finish(result, uncertain);
        });
        tauri::async_runtime::spawn(async move {
            if task.await.is_err() {
                eprintln!("[MirrorSave] worker=interrupted outcome=uncertain");
            }
        });
        return Ok(config_save_operation::SaveResponse::Accepted { operation: accepted });
    }

    if queue_clear_authorization.is_some() {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
    }

    let _transaction_guard = transaction_state.begin()?;
    // Admission covers URI resolution as well as effects and rollback.
    let observed_finishing_first_run = is_finishing_first_run(&observed_cfg, &new_config);
    let output_change_expected = !observed_finishing_first_run
        && (observed_cfg.audio_output_type != new_config.audio_output_type
            || observed_cfg.alsa_device != new_config.alsa_device
            || observed_cfg.dop_enabled != new_config.dop_enabled
            || observed_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
            || observed_cfg.replay_gain != new_config.replay_gain);
    let switch_queue = if output_change_expected {
        let plex_client = plex_state
            .0
            .lock()
            .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
            .clone();
        Some(resolve_device_switch_queue(&audio_state.0, &plex_client).await?)
    } else {
        None
    };

    let current_cfg = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a configuração atual: {}", e))?
        .clone();
    // The transaction is admitted and current config validated before opening.
    // Refusal exits before preparation, analyzer/MPD changes or persistence.
    validate_config_change_preflight(
        &observed_cfg,
        &current_cfg,
        &new_config,
        alsa_preflight::probe,
    )?;

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
            .prepare_reserved_device_switch(
                switch_queue
                    .as_ref()
                    .ok_or("Fila da troca de saída indisponível.")?,
            );
        playback_snapshot = match preparation {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if error.snapshot.is_some() {
                    let rollback = restore_prepared_playback(
                        error.snapshot,
                        switch_queue.as_ref(),
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
                        "[Analyzer] Failed to suspend analyzer before output switch: {}",
                        crate::diagnostics::error_summary(&error)
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
                    switch_queue.as_ref(),
                    &audio_state,
                );
                return Err(error_with_rollback(original, rollback));
            }
        };
        library_changed_during_start = confirm_device_switch_start(
            switch_result.map(|sync| sync.changed),
            || {
                rollback_audio_switch(
                    &current_cfg,
                    playback_snapshot,
                    switch_queue.as_ref(),
                    &sup_state,
                    &audio_state,
                )
            },
        )?;

        // 3. Restaura fila/faixa/posição quando possível; Playing/Paused terminam pausados e Stopped permanece parado
        let restore_result = match audio_state.0.lock() {
            Ok(audio) => audio.restore_after_reserved_device_switch(
                playback_snapshot,
                switch_queue
                    .as_ref()
                    .ok_or("Fila da troca de saída indisponível.")?,
            ),
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
                    switch_queue.as_ref(),
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
                switch_queue.as_ref(),
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
                switch_queue.as_ref(),
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
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
            );
            return Err(error_with_rollback(original, rollback));
        }
    };

    if let Err(save_error) = new_config.save() {
        drop(plex_guard);
        drop(config_guard);
        let error = config_save_failure_with_rollback(save_error, || {
            rollback_applied_config_change(
                finishing_first_run,
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
            )
            .map_err(ConfigSaveError::BeforeRename)
        });
        return Err(config_save_error_message(&error));
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
                        "[Analyzer] Configuration applied, but analyzer could not resume: {}",
                        crate::diagnostics::error_summary(&error)
                    );
                }
            }
        }
    }
    Ok(config_save_operation::SaveResponse::Completed)
}

fn uncertain_config_save_message(error: &ConfigSaveError) -> String {
    format!(
        "{error} Rollback automático interrompido devido ao estado incerto. A configuração em memória pode divergir do arquivo e dos recursos já aplicados. Verifique a configuração antes de tentar novas alterações."
    )
}

fn config_save_error_message(error: &ConfigSaveError) -> String {
    match error {
        ConfigSaveError::BeforeRename(_) => error.to_string(),
        ConfigSaveError::AfterRenameUnconfirmed(_) => uncertain_config_save_message(error),
    }
}

fn config_save_failure_with_rollback(
    error: ConfigSaveError,
    rollback: impl FnOnce() -> Result<(), ConfigSaveError>,
) -> ConfigSaveError {
    match error {
        ConfigSaveError::BeforeRename(cause) => {
            let original = format!("Falha ao persistir configuração: {cause}");
            match rollback() {
                Ok(()) => ConfigSaveError::BeforeRename(error_with_rollback(original, Ok(()))),
                Err(ConfigSaveError::BeforeRename(cause)) => {
                    ConfigSaveError::BeforeRename(error_with_rollback(original, Err(cause)))
                }
                Err(ConfigSaveError::AfterRenameUnconfirmed(cause)) => {
                    ConfigSaveError::AfterRenameUnconfirmed(format!(
                        "{original}. A tentativa de restaurar config.json falhou após o rename: {cause}"
                    ))
                }
            }
        }
        ConfigSaveError::AfterRenameUnconfirmed(_) => error,
    }
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

fn confirm_device_switch_start(
    start_result: Result<bool, String>,
    rollback: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    start_result.map_err(|error| {
        error_with_rollback(format!("Falha ao iniciar nova saída: {}", error), rollback())
    })
}

fn restore_prepared_playback(
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    switch_queue: Option<&ResolvedDeviceSwitchQueue>,
    audio_state: &State<'_, AudioState>,
) -> Result<(), String> {
    audio_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
        .restore_after_reserved_device_switch(
            playback_snapshot,
            switch_queue.ok_or("Fila da troca de saída indisponível no rollback.")?,
        )
}

fn rollback_audio_switch(
    previous_config: &AppConfig,
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    switch_queue: Option<&ResolvedDeviceSwitchQueue>,
    sup_state: &State<'_, SupervisorState>,
    audio_state: &State<'_, AudioState>,
) -> Result<(), String> {
    rollback_audio_switch_with(
        playback_snapshot,
        switch_queue.ok_or("Fila da troca de saída indisponível no rollback.")?,
        &audio_state.0,
        || {
            sup_state
                .0
                .lock()
                .map_err(|e| format!("Falha ao acessar o supervisor durante rollback: {}", e))?
                .start(previous_config)
                .map(|_| ())
        },
    )
}

fn rollback_audio_switch_with(
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    switch_queue: &ResolvedDeviceSwitchQueue,
    audio: &Mutex<AudioEngine>,
    restart_previous: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    audio
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
        .validate_device_switch_queue(switch_queue)?;
    restart_previous()?;
    audio
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
        .restore_after_reserved_device_switch(playback_snapshot, switch_queue)
}

fn rollback_applied_config_change(
    finishing_first_run: bool,
    audio_hw_changed: bool,
    folders_changed: bool,
    previous_config: &AppConfig,
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    switch_queue: Option<&ResolvedDeviceSwitchQueue>,
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
                switch_queue,
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

#[derive(Serialize)]
struct PlayerPlexNavigation {
    media: audio::CurrentMedia,
    navigation: plex::PlexTrackNavigation,
}

fn queued_plex_navigation_locator(
    queue: &[audio::TrackMetadata],
    media: &audio::CurrentMedia,
) -> Result<MediaLocator, String> {
    if let audio::CurrentMedia::Plex {
        server_id,
        part_key,
        queue_index,
    } = media
    {
        if let Some(
            locator @ MediaLocator::Plex {
                server_id: queued_server,
                part_key: queued_part,
                ..
            },
        ) = queue
            .get(*queue_index)
            .and_then(|track| track.media_locator.as_ref())
        {
            if queued_server == server_id && queued_part == part_key {
                return Ok(locator.clone());
            }
        }
    }
    Err("PLAYER_NAVIGATION_UNAVAILABLE".to_string())
}

#[cfg(test)]
mod player_navigation_queue_tests {
    use super::*;

    #[test]
    fn queued_navigation_uses_exact_server_part_and_occurrence_without_mutating_queue() {
        let track = audio::TrackMetadata {
            title: "Same title".into(),
            artist: "Same artist".into(),
            album: "Same album".into(),
            thumb: None,
            plex_image: None,
            uri: String::new(),
            duration: None,
            media_locator: Some(MediaLocator::Plex {
                server_id: "server".into(),
                part_key: "/library/parts/1/file.flac".into(),
                rating_key: Some("track-1".into()),
                file_path: None,
            }),
        };
        let queue = vec![track.clone(), track];
        let before = serde_json::to_string(&queue).unwrap();
        let media = audio::CurrentMedia::Plex {
            server_id: "server".into(),
            part_key: "/library/parts/1/file.flac".into(),
            queue_index: 1,
        };
        assert_eq!(
            queued_plex_navigation_locator(&queue, &media).unwrap(),
            queue[1].media_locator.clone().unwrap()
        );
        for media in [
            audio::CurrentMedia::Plex {
                server_id: "other".into(),
                part_key: "/library/parts/1/file.flac".into(),
                queue_index: 1,
            },
            audio::CurrentMedia::Plex {
                server_id: "server".into(),
                part_key: "/library/parts/2/file.flac".into(),
                queue_index: 1,
            },
            audio::CurrentMedia::Plex {
                server_id: "server".into(),
                part_key: "/library/parts/1/file.flac".into(),
                queue_index: 2,
            },
            audio::CurrentMedia::Local {
                uri: "Source/Album/one.flac".into(),
                queue_index: 0,
            },
        ] {
            assert!(queued_plex_navigation_locator(&queue, &media).is_err());
        }
        assert_eq!(serde_json::to_string(&queue).unwrap(), before);
    }
}

#[tauri::command]
async fn resolve_player_plex_navigation(
    media: audio::CurrentMedia,
    audio_state: State<'_, AudioState>,
    state: State<'_, PlexState>,
) -> Result<PlayerPlexNavigation, String> {
    let locator = {
        let audio = audio_state
            .0
            .lock()
            .map_err(|_| "PLAYER_NAVIGATION_UNAVAILABLE")?;
        queued_plex_navigation_locator(&audio.get_queue(), &media)?
    };
    let client = state
        .0
        .lock()
        .map_err(|_| "PLAYER_NAVIGATION_UNAVAILABLE")?
        .clone();
    let navigation = client.resolve_track_navigation(&locator).await?;
    let current = {
        let audio = audio_state
            .0
            .lock()
            .map_err(|_| "PLAYER_NAVIGATION_UNAVAILABLE")?;
        queued_plex_navigation_locator(&audio.get_queue(), &media)?
    };
    if current != locator {
        return Err("PLAYER_NAVIGATION_UNAVAILABLE".to_string());
    }
    if let audio::CurrentMedia::Plex { server_id, .. } = &media {
        if !state
            .0
            .lock()
            .map_err(|_| "PLAYER_NAVIGATION_UNAVAILABLE")?
            .navigation_server_is_selected(server_id)
        {
            return Err("PLAYER_NAVIGATION_UNAVAILABLE".to_string());
        }
    }
    Ok(PlayerPlexNavigation { media, navigation })
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

fn shutdown_controlled_resources(app_handle: &AppHandle) {
    mpris::shutdown(app_handle);
    if let Some(audio_state) = app_handle.try_state::<AudioState>() {
        match audio_state.0.lock() {
            Ok(mut audio) => {
                if let Err(error) = audio.persist_shutdown_resume() {
                    eprintln!(
                        "[Audio] Failed to save position during shutdown: {}",
                        crate::diagnostics::error_summary(&error)
                    );
                }
            }
            Err(_) => eprintln!("[Audio] Failed to access position during shutdown."),
        }
    }
    if let Some(state) = app_handle.try_state::<AnalyzerState>() {
        if let Ok(mut analyzer) = state.0.lock() {
            if let Err(error) = analyzer.stop(Some(app_handle)) {
                eprintln!(
                    "[Analyzer] Application shutdown failed: {}",
                    crate::diagnostics::error_summary(&error)
                );
            }
        }
    }
    if let Some(sup_state) = app_handle.try_state::<SupervisorState>() {
        if let Ok(mut supervisor) = sup_state.0.lock() {
            if let Err(error) = supervisor.stop() {
                eprintln!(
                    "[Supervisor] Application shutdown failed: {}",
                    crate::diagnostics::error_summary(&error)
                );
            }
        }
    }
}

fn start_coordinated_shutdown(app_handle: &AppHandle, exit_code: i32) {
    if let Some(watcher_state) = app_handle.try_state::<LocalWatcherState>() {
        watcher_state.begin_shutdown();
    }
    let transaction_state = app_handle.state::<ConfigTransactionState>();
    transaction_state.request_shutdown();
    if !transaction_state.start_shutdown_worker() {
        return;
    }
    let shutdown_app = app_handle.clone();
    tauri::async_runtime::spawn_blocking(move || {
        {
            if let Some(watcher_state) = shutdown_app.try_state::<LocalWatcherState>() {
                watcher_state.stop_and_join();
            }
            let transaction_state = shutdown_app.state::<ConfigTransactionState>();
            transaction_state.begin_effective_shutdown();
            let _transaction_guard = transaction_state.wait_for_active_transaction();
            shutdown_controlled_resources(&shutdown_app);
            transaction_state.mark_shutdown_complete();
        }
        shutdown_app.exit(exit_code);
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().to_string();
    // Read the stop marker before loading configuration or starting the supervisor.
    // A pending/invalid record must never trigger rescan, reconcile or rollback.
    let mut managed_startup_allowed = false;
    let (initial_config, config_valid) = match managed_save_intent::guard_startup(
        &MpdSupervisor::sonante_config_dir(),
        || {
            managed_startup_allowed = true;
            AppConfig::load()
        },
    ) {
        Ok(config) => (config, true),
        Err(error) => {
            eprintln!(
                "[Persistence] Failed to load configuration: {}",
                crate::diagnostics::error_summary(&error)
            );
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
                    "[Audio] Failed to check MPD database at startup: {}",
                    crate::diagnostics::error_summary(&error)
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
                eprintln!(
                    "[Supervisor] Audio supervisor error: {}",
                    crate::diagnostics::error_summary(&error)
                );
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
                "[Audio] Failed to request initial library rescan: {}",
                crate::diagnostics::error_summary(&e)
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
        .manage(ConfigTransactionState::new())
        .manage(LocalWatcherState::new())
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
            get_local_album_tracks,
            get_local_album_doctor,
            resolve_local_library_path,
            get_local_track_tags,
            get_local_album_tag_inspection,
            preflight_local_album_tag_batch,
            apply_local_album_tag_batch,
            update_local_track_tags,
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
            check_local_library,
            rescan_library,
            get_config,
            save_config,
            get_config_save_operation,
            get_local_import_preview,
            get_audio_devices,
            check_direct_device_availability,
            get_plex_libraries,
            resolve_player_plex_navigation,
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
                                eprintln!(
                                    "[Analyzer] Window cleanup failed: {}",
                                    crate::diagnostics::error_summary(&error)
                                );
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
    app.run(move |app_handle, event| {
        match event {
            RunEvent::Ready => {
                if managed_startup_allowed {
                    mpris::start(app_handle.clone());
                }
                if mpd_started && !restore_started.swap(true, Ordering::SeqCst) {
                    let app = app_handle.clone();
                    tauri::async_runtime::spawn(complete_startup_after_ready(app));
                }
            }
            RunEvent::ExitRequested { api, code, .. } => {
                let transaction_state = app_handle.state::<ConfigTransactionState>();
                if !transaction_state.shutdown_complete() {
                    api.prevent_exit();
                    start_coordinated_shutdown(app_handle, code.unwrap_or(0));
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
    use std::cell::{Cell, RefCell};
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

    fn tag_write_result(committed: bool) -> tags::LocalTrackTagWriteResult {
        tags::LocalTrackTagWriteResult {
            committed,
            tags: tags::LocalTrackTags {
                title: Some("Título novo".into()),
                artist: Some("Artista".into()),
                album: Some("Álbum".into()),
                album_artist: None,
                genre: None,
                date: None,
                track_number: Some(1),
                track_total: None,
                disc_number: None,
                disc_total: None,
                format: "flac".into(),
                readable: true,
                writable: true,
                editable: true,
                unsupported_reason: None,
                revision: "v1.new-revision".into(),
            },
        }
    }

    fn tag_write_error(code: tags::LocalTrackTagsErrorCode) -> tags::LocalTrackTagsError {
        tags::LocalTrackTagsError {
            code,
            message: "falha simulada sem path físico".into(),
            format: None,
            editable: false,
            unsupported_reason: None,
            committed: false,
        }
    }

    fn album_batch_track(
        uri: &str,
        state: tags::LocalAlbumTagBatchTrackOutcomeState,
        error: Option<tags::LocalTrackTagsError>,
    ) -> tags::LocalAlbumTagBatchTrackOutcome {
        let tags = (state == tags::LocalAlbumTagBatchTrackOutcomeState::Committed
            && error.is_none())
        .then(|| tag_write_result(true).tags);
        tags::LocalAlbumTagBatchTrackOutcome {
            uri: uri.to_string(),
            state,
            expected_revision: "expected-revision".to_string(),
            tags,
            error,
        }
    }

    fn album_batch_write_result(
        tracks: Vec<tags::LocalAlbumTagBatchTrackOutcome>,
        partial_commit: bool,
    ) -> tags::LocalAlbumTagBatchApplyResult {
        let count = |state| tracks.iter().filter(|track| track.state == state).count();
        tags::LocalAlbumTagBatchApplyResult {
            album_id: "album-id".to_string(),
            applicable: true,
            blocked_reason: None,
            committed_count: count(tags::LocalAlbumTagBatchTrackOutcomeState::Committed),
            unchanged_count: count(tags::LocalAlbumTagBatchTrackOutcomeState::Unchanged),
            failed_count: count(tags::LocalAlbumTagBatchTrackOutcomeState::Failed),
            not_attempted_count: count(
                tags::LocalAlbumTagBatchTrackOutcomeState::NotAttempted,
            ),
            tracks,
            partial_commit,
        }
    }

    #[test]
    fn committed_tag_write_requests_one_targeted_refresh_and_preserves_job_id() {
        let calls = Cell::new(0);
        let result = complete_local_tag_update(
            "Música/Álbum/faixa.flac",
            Ok(tag_write_result(true)),
            |path| {
                calls.set(calls.get() + 1);
                assert_eq!(path, "Música/Álbum/faixa.flac");
                Ok(73)
            },
        )
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert!(result.committed);
        assert!(result.mpd_refresh.requested);
        assert!(result.mpd_refresh.success);
        assert_eq!(result.mpd_refresh.job_id, Some(73));
        assert_eq!(result.tags.revision, "v1.new-revision");
    }

    #[test]
    fn mpd_refresh_failure_keeps_committed_tags_and_revision_without_rollback() {
        let calls = Cell::new(0);
        let result = complete_local_tag_update(
            "Music/track.flac",
            Ok(tag_write_result(true)),
            |_| {
                calls.set(calls.get() + 1);
                Err(TargetedLibraryUpdateErrorCode::MpdUnavailable)
            },
        )
        .unwrap();

        assert_eq!(calls.get(), 1);
        assert!(result.committed);
        assert_eq!(result.tags.title.as_deref(), Some("Título novo"));
        assert_eq!(result.tags.revision, "v1.new-revision");
        assert!(result.mpd_refresh.requested);
        assert!(!result.mpd_refresh.success);
        assert_eq!(
            result.mpd_refresh.error_code,
            Some(TargetedLibraryUpdateErrorCode::MpdUnavailable)
        );
    }

    #[test]
    fn uncommitted_or_rejected_tag_writes_never_request_mpd_refresh() {
        let no_op_calls = Cell::new(0);
        let no_op = complete_local_tag_update(
            "Music/track.flac",
            Ok(tag_write_result(false)),
            |_| {
                no_op_calls.set(no_op_calls.get() + 1);
                Ok(1)
            },
        )
        .unwrap();
        assert_eq!(no_op_calls.get(), 0);
        assert!(!no_op.committed);
        assert!(!no_op.mpd_refresh.requested);

        for code in [
            tags::LocalTrackTagsErrorCode::TransactionFailed,
            tags::LocalTrackTagsErrorCode::RevisionConflict,
            tags::LocalTrackTagsErrorCode::CueVirtual,
            tags::LocalTrackTagsErrorCode::UnsupportedFormat,
        ] {
            let calls = Cell::new(0);
            let error = complete_local_tag_update(
                "Music/track.flac",
                Err(tag_write_error(code.clone())),
                |_| {
                    calls.set(calls.get() + 1);
                    Ok(1)
                },
            )
            .unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(calls.get(), 0);
        }
    }

    #[test]
    fn album_batch_refresh_is_not_needed_without_a_commit() {
        let calls = Cell::new(0);
        let result = complete_local_album_tag_batch(
            album_batch_write_result(
                vec![
                    album_batch_track(
                        "Music/a.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Unchanged,
                        None,
                    ),
                    album_batch_track(
                        "Music/b.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Failed,
                        Some(tag_write_error(
                            tags::LocalTrackTagsErrorCode::TransactionFailed,
                        )),
                    ),
                    album_batch_track(
                        "Music/c.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::NotAttempted,
                        None,
                    ),
                ],
                false,
            ),
            |_| {
                calls.set(calls.get() + 1);
                Ok(1)
            },
        );

        assert_eq!(calls.get(), 0);
        assert!(result.tracks.iter().all(|track| {
            track.mpd_refresh.state == LocalAlbumTagMpdRefreshState::NotNeeded
                && track.mpd_refresh.job_id.is_none()
                && track.mpd_refresh.error_code.is_none()
        }));
    }

    #[test]
    fn album_batch_refresh_uses_committed_state_even_for_post_rename_error() {
        let mut post_rename_error =
            tag_write_error(tags::LocalTrackTagsErrorCode::DurabilityFailed);
        post_rename_error.committed = true;
        let calls = Cell::new(0);
        let result = complete_local_album_tag_batch(
            album_batch_write_result(
                vec![
                    album_batch_track(
                        "Music/a.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                        None,
                    ),
                    album_batch_track(
                        "Music/b.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                        Some(post_rename_error),
                    ),
                ],
                false,
            ),
            |_| {
                calls.set(calls.get() + 1);
                Ok(70 + calls.get())
            },
        );

        assert_eq!(calls.get(), 2);
        assert_eq!(result.committed_count, 2);
        assert_eq!(
            result
                .tracks
                .iter()
                .map(|track| track.write.state)
                .collect::<Vec<_>>(),
            vec![
                tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
            ]
        );
        assert_eq!(result.tracks[0].mpd_refresh.job_id, Some(71));
        assert_eq!(result.tracks[1].mpd_refresh.job_id, Some(72));
        assert!(result.tracks.iter().all(|track| {
            track.mpd_refresh.state == LocalAlbumTagMpdRefreshState::Accepted
        }));
        assert!(result.tracks[1]
            .write
            .error
            .as_ref()
            .is_some_and(|error| error.committed));
    }

    #[test]
    fn album_batch_targeted_refresh_continues_after_failure_and_sends_only_uris() {
        let (dir, socket_path, _) = mpd_health_test_paths("album-tag-targeted-refresh");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let expected = [
            "update \"Music/a.flac\"\n",
            "update \"Music/c.flac\"\n",
            "update \"Music/Álbum com espaço/b.flac\"\n",
        ];
        let responses: [&[u8]; 3] = [
            b"ACK [5@0] {update} failure\n",
            b"updating_db: 82\nOK\n",
            b"updating_db: 83\nOK\n",
        ];
        let server = thread::spawn(move || {
            for (expected_command, response) in expected.into_iter().zip(responses) {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.23.15\n").unwrap();
                stream.flush().unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut command)
                    .unwrap();
                assert_eq!(command, expected_command);
                stream.write_all(response).unwrap();
                stream.flush().unwrap();
            }
        });
        let socket = socket_path.to_string_lossy().into_owned();
        let result = complete_local_album_tag_batch(
            album_batch_write_result(
                vec![
                    album_batch_track(
                        "Music/c.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                        None,
                    ),
                    album_batch_track(
                        "Music/Álbum com espaço/b.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                        None,
                    ),
                    album_batch_track(
                        "Music/a.flac",
                        tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                        None,
                    ),
                ],
                false,
            ),
            |uri| AudioEngine::update_library_path(&socket, uri),
        );

        server.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
        assert_eq!(result.committed_count, 3);
        assert!(!result.partial_commit);
        assert_eq!(
            result
                .tracks
                .iter()
                .map(|track| track.mpd_refresh.state)
                .collect::<Vec<_>>(),
            vec![
                LocalAlbumTagMpdRefreshState::Failed,
                LocalAlbumTagMpdRefreshState::Accepted,
                LocalAlbumTagMpdRefreshState::Accepted,
            ]
        );
        assert_eq!(
            result.tracks[0].mpd_refresh.error_code,
            Some(TargetedLibraryUpdateErrorCode::UpdateCommandFailed)
        );
        assert_eq!(result.tracks[1].mpd_refresh.job_id, Some(82));
        assert_eq!(result.tracks[2].mpd_refresh.job_id, Some(83));
    }

    #[test]
    fn album_batch_refresh_failures_preserve_write_counts_and_partial_commit() {
        let calls = Cell::new(0);
        let original = album_batch_write_result(
            vec![
                album_batch_track(
                    "Music/a.flac",
                    tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                    None,
                ),
                album_batch_track(
                    "Music/b.flac",
                    tags::LocalAlbumTagBatchTrackOutcomeState::Failed,
                    Some(tag_write_error(
                        tags::LocalTrackTagsErrorCode::RevisionConflict,
                    )),
                ),
                album_batch_track(
                    "Music/c.flac",
                    tags::LocalAlbumTagBatchTrackOutcomeState::NotAttempted,
                    None,
                ),
            ],
            true,
        );
        let result = complete_local_album_tag_batch(original.clone(), |_| {
            calls.set(calls.get() + 1);
            Err(TargetedLibraryUpdateErrorCode::MpdUnavailable)
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(result.committed_count, original.committed_count);
        assert_eq!(result.failed_count, original.failed_count);
        assert_eq!(result.not_attempted_count, original.not_attempted_count);
        assert_eq!(result.partial_commit, original.partial_commit);
        assert_eq!(
            result.tracks[0].write.state,
            tags::LocalAlbumTagBatchTrackOutcomeState::Committed
        );
        assert_eq!(
            result.tracks[0].mpd_refresh.state,
            LocalAlbumTagMpdRefreshState::Failed
        );
        assert_eq!(
            result.tracks[0].mpd_refresh.error_code,
            Some(TargetedLibraryUpdateErrorCode::MpdUnavailable)
        );
        assert!(result.tracks[1..].iter().all(|track| {
            track.mpd_refresh.state == LocalAlbumTagMpdRefreshState::NotNeeded
        }));
    }

    #[test]
    fn all_album_batch_refreshes_can_fail_without_losing_commits() {
        let calls = Cell::new(0);
        let original = album_batch_write_result(
            ["Music/a.flac", "Music/b.flac", "Music/c.flac"]
                .into_iter()
                .map(|uri| {
                    album_batch_track(
                        uri,
                        tags::LocalAlbumTagBatchTrackOutcomeState::Committed,
                        None,
                    )
                })
                .collect(),
            false,
        );
        let result = complete_local_album_tag_batch(original, |_| {
            calls.set(calls.get() + 1);
            Err(TargetedLibraryUpdateErrorCode::MpdUnavailable)
        });

        assert_eq!(calls.get(), 3);
        assert_eq!(result.committed_count, 3);
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.not_attempted_count, 0);
        assert!(!result.partial_commit);
        assert!(result.tracks.iter().all(|track| {
            track.write.state == tags::LocalAlbumTagBatchTrackOutcomeState::Committed
                && track.mpd_refresh.state == LocalAlbumTagMpdRefreshState::Failed
                && track.mpd_refresh.error_code
                    == Some(TargetedLibraryUpdateErrorCode::MpdUnavailable)
        }));
    }

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
        fs::write(first.join("Album/track.flac"), b"track").unwrap();
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
        assert_eq!(
            resolve_local_library_path_in(
                &fixture.library,
                "Music/Album/track.flac",
                &folders,
            )
            .unwrap(),
            first.join("Album/track.flac")
        );
    }

    #[test]
    fn local_library_path_resolves_managed_files_and_directories() {
        let fixture = LocalPathFixture::new();
        let root = fixture.root.join("Music");
        fs::create_dir_all(root.join("Album")).unwrap();
        fs::write(root.join("Album/track.flac"), b"track").unwrap();
        symlink(&root, fixture.library.join("Music")).unwrap();
        let staging = fixture.root.join("library.next");
        local_media::mirror::build_filtered_staging(
            &staging,
            &fixture.library,
            std::slice::from_ref(&root),
            "open-location-managed",
        )
        .unwrap();
        fs::remove_file(fixture.library.join("Music")).unwrap();
        fs::remove_dir(&fixture.library).unwrap();
        fs::rename(staging, &fixture.library).unwrap();
        let folders = vec![root.to_string_lossy().into_owned()];

        assert_eq!(
            resolve_local_library_path_in(&fixture.library, "Music/Album", &folders).unwrap(),
            root.join("Album")
        );
        assert_eq!(
            resolve_local_library_path_in(
                &fixture.library,
                "Music/Album/track.flac",
                &folders,
            )
            .unwrap(),
            root.join("Album/track.flac")
        );
    }

    #[test]
    fn local_library_path_rejects_absolute_parent_and_empty_paths() {
        let fixture = LocalPathFixture::new();
        let root = fixture.root.join("Music");
        fs::create_dir(&root).unwrap();
        symlink(&root, fixture.library.join("Music")).unwrap();
        let folders = vec![root.to_string_lossy().into_owned()];

        for invalid in [
            "",
            "/Music",
            ".",
            "../Music",
            "Music/../Music",
            "Music/./Album",
        ] {
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
    fn alsa_preflight_selects_only_known_distinct_physical_endpoints() {
        for (old_type, old_device, new_type, new_device, expected) in [
            ("pipewire", "default", "alsa", "hw:CARD=Test,DEV=0", true),
            ("shared", "default", "alsa", "hw:0,0", true),
            ("alsa", "default", "alsa", "hw:0,0", true),
            ("pipewire", "hw:0,0", "alsa", "hw:0,0", true),
            ("pipewire", "default", "alsa", "hw:0,0,0", true),
            ("alsa", "hw:0,0", "alsa", "hw:1,0", true),
            (
                "alsa",
                "hw:CARD=Old,DEV=0",
                "alsa",
                "hw:CARD=New,DEV=0",
                true,
            ),
            ("alsa", "hw:0,0", "alsa", "hw:00,00", false),
            ("alsa", "hw:CARD=Old,DEV=0", "alsa", "hw:Old", false),
            ("alsa", "hw:0,0", "alsa", "hw:CARD=Old,DEV=0", false),
            ("alsa", "hw:CARD=Old,DEV=0", "alsa", "hw:0,0", false),
            ("alsa", "hw:0,0", "alsa", "hw:0,0", false),
            ("alsa", "custom", "alsa", "hw:0,0", false),
            ("pipewire", "default", "pipewire", "hw:0,0", false),
            ("pipewire", "default", "shared", "hw:0,0", false),
            ("pipewire", "default", "alsa", "default", false),
            ("pipewire", "default", "alsa", "plughw:0,0", false),
            ("pipewire", "default", "alsa", "null", false),
            ("pipewire", "default", "alsa", "custom", false),
            ("pipewire", "default", "alsa", "hw:", false),
            ("pipewire", "default", "unknown", "hw:0,0", false),
        ] {
            let current = AppConfig {
                first_run: false,
                audio_output_type: old_type.into(),
                alsa_device: old_device.into(),
                ..AppConfig::default()
            };
            let mut proposed = current.clone();
            proposed.audio_output_type = new_type.into();
            proposed.alsa_device = new_device.into();
            let calls = Cell::new(0);
            validate_config_change_preflight(&current, &current, &proposed, |device| {
                calls.set(calls.get() + 1);
                assert_eq!(device, new_device);
                Ok(())
            })
            .unwrap();
            assert_eq!(
                calls.get(),
                usize::from(expected),
                "{old_type}/{old_device} -> {new_type}/{new_device}"
            );
        }
    }

    #[test]
    fn alsa_preflight_checks_current_configuration_before_probing() {
        let observed = AppConfig::default();
        let mut current = observed.clone();
        current.audio_buffer_size_kb += 1;
        let mut proposed = observed.clone();
        proposed.audio_output_type = "alsa".into();
        proposed.alsa_device = "hw:0,0".into();
        let error = validate_config_change_preflight(&observed, &current, &proposed, |_| {
            panic!("stale configuration must not open ALSA")
        })
        .unwrap_err();
        assert!(error.contains("A configuração mudou"));
    }

    #[test]
    fn alsa_preflight_refusal_aborts_admitted_transaction_before_effects() {
        for error in [
            alsa_preflight::PreflightError::Busy,
            alsa_preflight::PreflightError::Unavailable,
            alsa_preflight::PreflightError::Unknown,
        ] {
            for playback_state in ["stop", "pause", "play"] {
                for uri in ["Música/Álbum/faixa.flac", "https://plex.invalid/stream"] {
                    for (old_type, old_device) in
                        [("pipewire", "default"), ("alsa", "hw:CARD=Old,DEV=0")]
                    {
                        let transactions = ConfigTransactionState::new();
                        let current = AppConfig {
                            first_run: false,
                            audio_output_type: old_type.into(),
                            alsa_device: old_device.into(),
                            ..AppConfig::default()
                        };
                        let mut proposed = current.clone();
                        proposed.audio_output_type = "alsa".into();
                        proposed.alsa_device = "hw:CARD=Test,DEV=0".into();
                        // Simulated downstream effects, never an MPD process or real ALSA.
                        let mut queue = vec![uri.to_string()];
                        let mut cursor = (playback_state, 0, 16.5);
                        let mut confirmed = current.clone();
                        let mut persisted = current.clone();
                        let mut effects = Vec::new();
                        let result: Result<(), String> = (|| {
                            let _guard = transactions.begin()?;
                            validate_config_change_preflight(
                                &current,
                                &current,
                                &proposed,
                                |_| Err(error),
                            )?;
                            effects
                                .extend(["prepare", "analyzer", "restart", "persist", "publish"]);
                            queue.clear();
                            cursor = ("stop", 0, 0.0);
                            confirmed = proposed.clone();
                            persisted = proposed;
                            Ok(())
                        })();
                        assert_eq!(result, Err(error.code().to_string()));
                        assert!(effects.is_empty());
                        assert_eq!(queue, [uri.to_string()]);
                        assert_eq!(cursor, (playback_state, 0, 16.5));
                        assert_eq!(confirmed, current);
                        assert_eq!(persisted, current);
                        // Failure drops the reservation; the next Save can be admitted.
                        drop(transactions.begin().unwrap());
                    }
                }
            }
        }
    }

    #[test]
    fn alsa_preflight_current_device_processing_changes_do_not_open_pcm() {
        let current = AppConfig {
            first_run: false,
            audio_output_type: "alsa".into(),
            alsa_device: "hw:CARD=Owned,DEV=0".into(),
            ..AppConfig::default()
        };
        let mut proposed = current.clone();
        proposed.audio_buffer_size_kb = 32768;
        proposed.replay_gain = "album".into();
        proposed.dop_enabled = !current.dop_enabled;
        validate_config_change_preflight(&current, &current, &proposed, |_| {
            panic!("saving processing settings must not open the owned PCM")
        })
        .unwrap();
    }

    #[test]
    fn directed_alsa_query_releases_config_mutex_and_reserves_against_save() {
        let transactions = ConfigTransactionState::new();
        let config = Mutex::new(AppConfig {
            first_run: false,
            audio_output_type: "alsa".into(),
            alsa_device: "hw:CARD=Owned,DEV=0".into(),
            ..AppConfig::default()
        });
        let result = check_direct_device_availability_with(
            "hw:CARD=Other,DEV=0",
            &transactions,
            &config,
            || vec!["hw:CARD=Other,DEV=0".into()],
            |_| {
                assert!(
                    config.try_lock().is_ok(),
                    "configuration remains readable during probe"
                );
                assert!(
                    transactions.operations.reserve().is_err(),
                    "Save cannot start during probe"
                );
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result[0].state, alsa_preflight::Availability::Available);
        assert!(
            transactions.begin().is_ok(),
            "completed query releases admission"
        );
    }

    #[test]
    fn directed_alsa_query_rejects_pending_save_shutdown_and_stale_shutdown_results() {
        for stage in 0..3 {
            let transactions = ConfigTransactionState::new();
            let config = Mutex::new(AppConfig::default());
            if stage == 0 {
                transactions.request_shutdown();
            }
            let result = check_direct_device_availability_with(
                "hw:CARD=Other,DEV=0",
                &transactions,
                &config,
                || {
                    if stage == 1 {
                        transactions.request_shutdown();
                    }
                    vec!["hw:CARD=Other,DEV=0".into()]
                },
                |_| {
                    assert_eq!(stage, 2, "shutdown before probing must not open PCM");
                    transactions.request_shutdown();
                    Ok(())
                },
            );
            assert_eq!(result.unwrap_err(), "ALSA_AVAILABILITY_UNCONFIRMED");
        }
        let transactions = ConfigTransactionState::new();
        let (_, operation) = transactions.operations.reserve().unwrap();
        let config = Mutex::new(AppConfig::default());
        let result = check_direct_device_availability_with(
            "hw:CARD=Other,DEV=0",
            &transactions,
            &config,
            || panic!("active Save must reject before enumeration"),
            |_| panic!("active Save must not probe"),
        );
        assert_eq!(result.unwrap_err(), "ALSA_AVAILABILITY_UNCONFIRMED");
        operation.finish(Ok(()), false);
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
    fn managed_folders_route_accepts_only_local_folders_changes() {
        let mut current = AppConfig::default();
        current.first_run = false;
        let mut folders_only = current.clone();
        folders_only.local_folders.push("/music".to_string());
        assert!(is_local_folders_only_change(&current, &folders_only));

        let mut folders_and_plex = folders_only.clone();
        folders_and_plex.plex_url = "https://plex.example.invalid".to_string();
        assert!(!is_local_folders_only_change(&current, &folders_and_plex));

        let mut folders_and_output = folders_only.clone();
        folders_and_output.alsa_device = "hw:CARD=Test,DEV=0".to_string();
        assert!(!is_local_folders_only_change(&current, &folders_and_output));

        let mut output_only = current.clone();
        output_only.alsa_device = "hw:CARD=Test,DEV=0".to_string();
        assert!(!is_local_folders_only_change(&current, &output_only));
        assert!(!is_local_folders_only_change(&current, &current));
    }

    #[test]
    fn config_save_scope_keeps_folders_only_audio_only_and_first_setup() {
        let current = AppConfig {
            first_run: false,
            ..AppConfig::default()
        };
        let mut folders = current.clone();
        folders.local_folders.push("/fixture/new".into());
        assert!(validate_config_save_scope(&current, &folders).is_ok());
        assert!(is_local_folders_only_change(&current, &folders));
        let mut audio = current.clone();
        audio.alsa_device = "hw:CARD=Fixture,DEV=0".into();
        assert!(validate_config_save_scope(&current, &audio).is_ok());
        assert!(!is_local_folders_only_change(&current, &audio));
        assert!(validate_config_save_scope(&current, &current).is_ok());
        let mut initial = current.clone();
        initial.first_run = true;
        audio.local_folders = folders.local_folders;
        assert!(validate_config_save_scope(&initial, &audio).is_ok());
        assert!(is_finishing_first_run(&initial, &audio));
    }

    #[test]
    fn config_save_scope_rejects_mixed_changes_before_any_downstream_effect() {
        let current = AppConfig {
            first_run: false,
            ..AppConfig::default()
        };
        // Full-config equality covers every preference, not only audio fields.
        let original = serde_json::to_value(&current).unwrap();
        for (field, value) in original.as_object().unwrap() {
            if field == "local_folders" {
                continue;
            }
            let mut proposed = original.clone();
            proposed["local_folders"] = serde_json::json!(["/fixture/new"]);
            proposed[field] = match value {
                serde_json::Value::Bool(value) => serde_json::json!(!value),
                serde_json::Value::Number(_) => serde_json::json!(8192),
                serde_json::Value::Null => serde_json::json!("fixture"),
                serde_json::Value::String(_) => serde_json::json!("fixture"),
                _ => panic!("uncovered config field: {field}"),
            };
            let proposed: AppConfig = serde_json::from_value(proposed).unwrap();
            let effects = Cell::new(0);
            let result: Result<(), String> = (|| {
                validate_config_save_scope(&current, &proposed)?;
                // Stands for admission, queue resolution, probing, prepare,
                // MPD stop/start, publication and persistence. None may run.
                effects.set(effects.get() + 1);
                Ok(())
            })();
            assert_eq!(
                result,
                Err("LOCAL_FOLDERS_MIXED_CONFIG_CHANGE".into()),
                "{field}"
            );
            assert_eq!(effects.get(), 0, "{field}");
            assert_eq!(serde_json::to_value(&current).unwrap(), original);
        }
    }

    #[test]
    fn equivalent_generation_skips_mpd_except_explicit_full_rescan() {
        let missing_socket = format!(
            "/tmp/sonante-startup-equivalent-{}-missing.sock",
            std::process::id()
        );
        assert_eq!(
            finish_unchanged_managed_generation(
                ManagedMirrorRefreshMode::Startup,
                &missing_socket,
            ),
            Ok(false)
        );
        assert_eq!(
            finish_unchanged_managed_generation(
                ManagedMirrorRefreshMode::Filesystem,
                &missing_socket,
            ),
            Ok(false)
        );
        assert_eq!(
            finish_unchanged_managed_generation(
                ManagedMirrorRefreshMode::Check,
                &missing_socket,
            ),
            Ok(false)
        );
        assert!(finish_unchanged_managed_generation(
            ManagedMirrorRefreshMode::Manual,
            &missing_socket,
        )
        .is_err());

        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 96\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Album/faixa.flac\nOK\n"),
        ]);
        assert_eq!(
            finish_unchanged_managed_generation(ManagedMirrorRefreshMode::Manual, &socket),
            Ok(true)
        );
        server.join().unwrap();
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
            playback_error: None,
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

    fn mpd_health_test_paths(_test_name: &str) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::DirBuilderExt;

        // Keep Unix socket paths short regardless of worktree, TMPDIR or test name.
        // Exclusive creation prevents adopting or cleaning up another test's files.
        loop {
            let id = NEXT_MPD_HEALTH_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let dir = PathBuf::from("/tmp").join(format!("sn-mpd-{}-{}", std::process::id(), id));
            match fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    let socket_path = dir.join("mpd.socket");
                    let pid_path = dir.join("mpd.pid");
                    return (dir, socket_path, pid_path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("Failed to create private MPD test directory: {error}"),
            }
        }
    }

    #[test]
    fn mpd_health_fixture_paths_are_short_private_and_parallel_safe() {
        use std::os::unix::fs::PermissionsExt;

        let workers: Vec<_> = (0..8)
            .map(|_| {
                thread::spawn(|| {
                    let (dir, socket_path, _) = mpd_health_test_paths(&"long-name".repeat(32));
                    assert_eq!(
                        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                        0o700
                    );
                    assert!(socket_path.as_os_str().len() < 108);
                    let listener = UnixListener::bind(&socket_path).unwrap();
                    (dir, listener)
                })
            })
            .collect();
        let fixtures: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let unique: std::collections::HashSet<_> = fixtures.iter().map(|(dir, _)| dir).collect();
        assert_eq!(unique.len(), fixtures.len());
        for (dir, listener) in fixtures {
            drop(listener);
            fs::remove_dir_all(dir).unwrap();
        }
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

    #[test]
    fn list_local_directory_delayed_response_does_not_lock_status_or_controls() {
        let (dir, socket_path, _) = mpd_health_test_paths("ld");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let (requested_tx, requested_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut listing, _) = listener.accept().unwrap();
            listing.write_all(b"OK MPD 0.24.15\n").unwrap();
            let mut command = String::new();
            BufReader::new(listing.try_clone().unwrap()).read_line(&mut command).unwrap();
            assert_eq!(command, "lsinfo \"Music\"\n");
            requested_tx.send(()).unwrap();
            // Keep lsinfo pending while independent status/control requests complete.
            for (expected, response) in [
                ("status\n", "state: stop\nvolume: 50\nOK\n"),
                ("setvol 50\n", "OK\n"),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.24.15\n").unwrap();
                command.clear();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut command).unwrap();
                assert_eq!(command, expected);
                stream.write_all(response.as_bytes()).unwrap();
            }
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            listing.write_all(b"file: Music/one.flac\nTitle: One\nOK\n").unwrap();
        });
        let audio = Arc::new(Mutex::new(AudioEngine::new(&socket_path.to_string_lossy(), "")));
        let worker_audio = Arc::clone(&audio);
        let worker = thread::spawn(move || {
            tauri::async_runtime::block_on(list_local_directory_from_audio("Music".into(), &worker_audio))
        });
        requested_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        {
            let audio = audio.try_lock().expect("lsinfo must release AudioState before waiting");
            assert_eq!(audio.get_status().unwrap().state, "stop");
            audio.set_volume(50).unwrap();
        }
        release_tx.send(()).unwrap();
        let items = worker.join().unwrap().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, "Music/one.flac");
        assert_eq!(items[0].title.as_deref(), Some("One"));
        server.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
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
    fn playback_error_keeps_mpd_status_snapshot_available() {
        let (dir, socket_path, pid_path) = mpd_health_test_paths("playback-error");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.23.15\n").unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command, "status\n");
            stream.write_all(b"state: stop\nerror: Failed to open ALSA device \"hw:0\": Device or resource busy\nOK\n").unwrap();
        });
        let supervisor = Mutex::new(test_supervisor(&socket_path, &pid_path, sleeping_child()));
        let audio = Mutex::new(AudioEngine::new(
            &socket_path.to_string_lossy(),
            &dir.to_string_lossy(),
        ));
        let snapshot = collect_mpd_status_snapshot(&supervisor, &audio).unwrap();
        assert_eq!(snapshot.health, MpdHealth::Available);
        assert_eq!(
            snapshot.playback.unwrap().playback_error,
            Some(audio::PlaybackDiagnostic::OutputBusy)
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
    fn config_read_during_startup_reconciliation_preserves_orphan_guard() {
        // Both the cleaned-up and incomplete startup use the same production
        // admission/read helpers. No process, MPD socket or personal config.
        for cleanup_completed in [true, false] {
            let fixture = LocalPathFixture::new();
            let source = fixture.root.join("Initial");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("track.flac"), b"fixture").unwrap();
            install_test_managed_generation(&fixture.library, std::slice::from_ref(&source));
            let config = AppConfig {
                first_run: false,
                local_folders: vec![source.to_string_lossy().into_owned()],
                ..AppConfig::default()
            };
            let config_bytes = serde_json::to_vec(&config).unwrap();
            let config_path = fixture.root.join("config.json");
            persistence::atomic_write_private_synced(&config_path, &config_bytes, "config.json")
                .unwrap();
            let state = Arc::new(ConfigState(Mutex::new(config.clone())));
            let transactions = Arc::new(ConfigTransactionState::new());
            let mut startup_guard = transactions
                .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
                .unwrap()
                .unwrap();
            let transaction_guard = transactions.begin().unwrap();
            startup_guard.allow_reads(&fixture.root).unwrap();
            let staging = fixture.root.join("library.next");
            // Pause at the staging-build step of prepare_managed_generation,
            // before comparison/cleanup of an otherwise unchanged generation.
            local_media::mirror::build_filtered_staging(
                &staging,
                &fixture.library,
                &[source],
                "startup-config-read",
            )
            .unwrap();
            assert!(!transactions.operations.active());
            assert!(transactions.operations.snapshot().unwrap().is_none());
            assert!(managed_save_intent::check_startup(&fixture.root)
                .unwrap_err()
                .contains("MANAGED_FOLDERS_SAVE_PENDING"));

            let reader_state = Arc::clone(&state);
            let reader_transactions = Arc::clone(&transactions);
            let root = fixture.root.clone();
            let (attempt_tx, attempt_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let reader = thread::spawn(move || {
                attempt_tx.send(()).unwrap();
                result_tx
                    .send(read_config_after_startup_reconciliation(
                        &reader_state,
                        &reader_transactions,
                        &root,
                    ))
                    .unwrap();
            });
            attempt_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            // Must finish while reconciliation and its transaction are still
            // paused with staging present, rather than waiting for cleanup.
            assert_eq!(
                result_rx
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap()
                    .unwrap(),
                config
            );
            reader.join().unwrap();
            assert!(staging.exists());
            assert!(transactions.operations.reserve().is_err());
            assert!(transactions
                .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
                .is_err());
            if cleanup_completed {
                local_media::mirror::cleanup_managed_generation(&staging).unwrap();
            }
            drop(startup_guard);
            drop(transaction_guard);
            let result =
                read_config_after_startup_reconciliation(&state, &transactions, &fixture.root);
            if cleanup_completed {
                assert_eq!(result.unwrap(), config);
                assert!(!staging.exists());
            } else {
                assert!(result.unwrap_err().contains("MANAGED_FOLDERS_SAVE_PENDING"));
                assert!(staging.exists());
                // A new session cannot inherit the in-memory startup gate.
                assert!(read_config_after_startup_reconciliation(
                    &state,
                    &ConfigTransactionState::new(),
                    &fixture.root,
                )
                .unwrap_err()
                .contains("MANAGED_FOLDERS_SAVE_PENDING"));
            }
            assert_eq!(fs::read(config_path).unwrap(), config_bytes);
            assert!(fixture.library.exists());
        }
    }

    #[test]
    fn config_read_waits_only_for_startup_admission_and_shutdown_waits_for_transaction() {
        let fixture = LocalPathFixture::new();
        let config = AppConfig::default();
        fs::write(
            fixture.root.join("config.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let state = Arc::new(ConfigState(Mutex::new(config.clone())));
        let transactions = Arc::new(ConfigTransactionState::new());
        let mut startup = transactions
            .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
            .unwrap()
            .unwrap();
        let transaction = transactions.begin().unwrap();
        let reader_state = Arc::clone(&state);
        let reader_transactions = Arc::clone(&transactions);
        let root = fixture.root.clone();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            result_tx
                .send(read_config_after_startup_reconciliation(
                    &reader_state,
                    &reader_transactions,
                    &root,
                ))
                .unwrap();
        });
        attempt_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(result_rx.recv_timeout(Duration::from_millis(50)).is_err());
        startup.allow_reads(&fixture.root).unwrap();
        assert_eq!(
            result_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap(),
            config
        );
        reader.join().unwrap();

        transactions.request_shutdown();
        assert!(transactions.begin().is_err());
        let shutdown_transactions = Arc::clone(&transactions);
        let (finished_tx, finished_rx) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            let _guard = shutdown_transactions.wait_for_active_transaction();
            finished_tx.send(()).unwrap();
        });
        assert!(finished_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(startup);
        drop(transaction);
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        shutdown.join().unwrap();
    }

    #[test]
    fn config_read_active_startup_rechecks_config_and_journal_and_keeps_confirmed_config() {
        let fixture = LocalPathFixture::new();
        let config = AppConfig {
            first_run: false,
            ..AppConfig::default()
        };
        let config_path = fixture.root.join("config.json");
        let bytes = serde_json::to_vec(&config).unwrap();
        fs::write(&config_path, &bytes).unwrap();
        let state = ConfigState(Mutex::new(config.clone()));
        let transactions = ConfigTransactionState::new();
        let mut startup = transactions
            .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
            .unwrap()
            .unwrap();
        let transaction = transactions.begin().unwrap();
        startup.allow_reads(&fixture.root).unwrap();
        // Reading a valid file cannot publish a different, uncommitted config.
        fs::write(
            &config_path,
            serde_json::to_vec(&AppConfig::default()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_config_after_startup_reconciliation(&state, &transactions, &fixture.root).unwrap(),
            config
        );
        fs::write(&config_path, b"invalid config").unwrap();
        assert!(
            read_config_after_startup_reconciliation(&state, &transactions, &fixture.root).is_err()
        );
        assert_eq!(fs::read(&config_path).unwrap(), b"invalid config");
        fs::write(&config_path, &bytes).unwrap();
        let journal = fixture.root.join(managed_save_intent::NAME);
        fs::write(&journal, b"unexpected intent").unwrap();
        assert!(
            read_config_after_startup_reconciliation(&state, &transactions, &fixture.root)
                .unwrap_err()
                .contains("MANAGED_FOLDERS_SAVE_PENDING")
        );
        assert_eq!(fs::read(journal).unwrap(), b"unexpected intent");
        drop(startup);
        drop(transaction);
    }

    #[test]
    fn config_read_startup_admission_rejects_invalid_config_and_revokes_on_unwind() {
        let fixture = LocalPathFixture::new();
        let config_path = fixture.root.join("config.json");
        fs::write(&config_path, b"invalid config").unwrap();
        let transactions = ConfigTransactionState::new();
        {
            let mut startup = transactions
                .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
                .unwrap()
                .unwrap();
            assert!(startup.allow_reads(&fixture.root).is_err());
        }
        assert!(!*transactions.startup_config_read_gate.lock().unwrap());
        fs::write(
            &config_path,
            serde_json::to_vec(&AppConfig::default()).unwrap(),
        )
        .unwrap();
        assert!(std::panic::catch_unwind(|| {
            let mut startup = transactions
                .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
                .unwrap()
                .unwrap();
            let _transaction = transactions.begin().unwrap();
            startup.allow_reads(&fixture.root).unwrap();
            fs::create_dir(fixture.root.join("library.next")).unwrap();
            panic!("simulated reconciliation failure");
        })
        .is_err());
        assert!(!*transactions.startup_config_read_gate.lock().unwrap());
        let state = ConfigState(Mutex::new(AppConfig::default()));
        assert!(
            read_config_after_startup_reconciliation(&state, &transactions, &fixture.root)
                .unwrap_err()
                .contains("MANAGED_FOLDERS_SAVE_PENDING")
        );
        assert!(fixture.root.join("library.next").exists());
    }

    #[test]
    fn config_read_blocks_orphaned_staging_and_journal_without_modifying_them() {
        for name in ["library.next", managed_save_intent::NAME] {
            let fixture = LocalPathFixture::new();
            let marker = fixture.root.join(name);
            if name == "library.next" {
                fs::create_dir(&marker).unwrap();
                fs::write(marker.join("unknown"), b"preserve").unwrap();
            } else {
                fs::write(&marker, b"incomplete intent").unwrap();
            }
            let state = ConfigState(Mutex::new(AppConfig::default()));
            let transactions = ConfigTransactionState::new();
            // The mere presence of an active startup gate is not permission to
            // overlook a pre-existing stop marker: startup's guard still fails.
            {
                let mut startup = transactions
                    .lock_startup_config_reads(ManagedMirrorRefreshMode::Startup)
                    .unwrap()
                    .unwrap();
                assert!(startup.allow_reads(&fixture.root).is_err());
            }
            assert!(
                read_config_after_startup_reconciliation(&state, &transactions, &fixture.root,)
                    .unwrap_err()
                    .contains("MANAGED_FOLDERS_SAVE_PENDING")
            );
            let preserved = if name == "library.next" {
                marker.join("unknown")
            } else {
                marker
            };
            assert_eq!(
                fs::read(preserved).unwrap(),
                if name == "library.next" {
                    b"preserve".as_slice()
                } else {
                    b"incomplete intent".as_slice()
                }
            );
            assert!(!fixture.root.join("config.json").exists());
        }
    }

    #[test]
    fn config_read_preserves_existing_active_save_and_uncertain_behavior() {
        let fixture = LocalPathFixture::new();
        let config = AppConfig {
            first_run: false,
            ..AppConfig::default()
        };
        let state = ConfigState(Mutex::new(config.clone()));
        let transactions = ConfigTransactionState::new();
        let (_, operation) = transactions.operations.reserve().unwrap();
        fs::write(
            fixture.root.join(managed_save_intent::NAME),
            b"fixture pending intent",
        )
        .unwrap();
        assert_eq!(
            read_config_after_startup_reconciliation(&state, &transactions, &fixture.root,)
                .unwrap(),
            config
        );
        operation.finish(Err("fixture uncertainty".to_string()), true);
        assert!(
            read_config_after_startup_reconciliation(&state, &transactions, &fixture.root,)
                .unwrap_err()
                .contains("CONFIG_PERSISTENCE_UNCERTAIN")
        );
        assert!(!fixture.root.join("config.json").exists());
    }

    #[test]
    fn config_transaction_state_serializes_concurrent_changes() {
        let state = Arc::new(ConfigTransactionState::new());
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
    fn shutdown_rejects_new_transactions_and_waits_for_active_one() {
        let state = Arc::new(ConfigTransactionState::new());
        let active_guard = state.begin().unwrap();
        let preparation = match state.preparation.begin(
            managed_preparation::PreparationRequest::Automatic,
            Path::new("/unused/library"),
        ) {
            managed_preparation::PreparationAdmission::Started(preparation) => preparation,
            _ => panic!("preparação deveria ter sido admitida"),
        };
        state.request_shutdown();
        assert!(!preparation.is_current());
        assert!(state.begin().unwrap_err().contains("sendo encerrada"));

        let worker_state = Arc::clone(&state);
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _guard = worker_state.wait_for_active_transaction();
            acquired_tx.send(()).unwrap();
        });

        assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(active_guard);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn shutdown_prevents_restart_that_has_not_already_started() {
        let state = Arc::new(ConfigTransactionState::new());
        let permit = state.try_begin_restart().unwrap();
        state.request_shutdown();
        assert!(!state.shutdown_started());

        let shutdown_state = Arc::clone(&state);
        let (started_tx, started_rx) = mpsc::channel();
        let shutdown = thread::spawn(move || {
            shutdown_state.begin_effective_shutdown();
            started_tx.send(()).unwrap();
        });
        assert!(started_rx.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(!state.shutdown_started());

        drop(permit);
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        shutdown.join().unwrap();
        assert!(state.shutdown_started());
        assert!(state.try_begin_restart().is_none());
        assert!(state.start_shutdown_worker());
        assert!(!state.start_shutdown_worker());
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

    #[test]
    fn config_save_failure_before_rename_runs_existing_rollback() {
        let rollback_calls = Cell::new(0);
        let error = config_save_failure_with_rollback(
            ConfigSaveError::BeforeRename("simulated sync failure".to_string()),
            || {
                rollback_calls.set(rollback_calls.get() + 1);
                Ok(())
            },
        );
        assert_eq!(rollback_calls.get(), 1);
        assert!(matches!(error, ConfigSaveError::BeforeRename(_)));
        assert!(config_save_error_message(&error).contains("Estado anterior restaurado"));
    }

    #[test]
    fn config_save_failure_after_rename_never_runs_consumers_rollback() {
        for cause in [
            "simulated validation failure",
            "simulated directory sync failure",
        ] {
            let error = config_save_failure_with_rollback(
                ConfigSaveError::AfterRenameUnconfirmed(cause.to_string()),
                || panic!("não deve regravar config antiga nem reverter áudio/biblioteca"),
            );
            assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
            let message = config_save_error_message(&error);
            assert!(message.contains(cause));
            assert!(message.contains("Estado incerto de config.json"));
            assert!(message.contains("commit e durabilidade não confirmados"));
            assert!(message.starts_with("CONFIG_PERSISTENCE_UNCERTAIN:"));
            assert!(message.contains("Rollback automático interrompido"));
            assert!(!message.contains("Estado anterior restaurado"));
        }
    }

    #[test]
    fn config_save_failure_preserves_uncertainty_during_config_restoration() {
        let error = config_save_failure_with_rollback(
            ConfigSaveError::BeforeRename("initial sync failure".to_string()),
            || {
                Err(ConfigSaveError::AfterRenameUnconfirmed(
                    "restoration validation failure".to_string(),
                ))
            },
        );
        assert!(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
        let message = config_save_error_message(&error);
        assert!(message.contains("initial sync failure"));
        assert!(message.contains("restoration validation failure"));
        assert!(!message.contains("Estado anterior restaurado"));
    }

    #[test]
    fn experimental_folders_uncertain_config_save_preserves_mirror_and_mpd() {
        let fixture = LocalPathFixture::new();
        let old = fixture.root.join("Old");
        let added = fixture.root.join("Added");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&added).unwrap();
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&old));
        let publication = prepare_test_managed_change(
            &fixture.library,
            &[old, added],
            "uncertain-config-save",
        );
        let published = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let error = complete_published_mirror_refresh(
            || Ok(()),
            || -> Result<(), String> { panic!("não deve capturar reprodução") },
            || panic!("não deve parar MPD"),
            || panic!("não deve reverter mirror"),
            |_| panic!("não deve restaurar reprodução"),
            || panic!("não deve limpar geração rejeitada"),
            || {
                Err(config_save_failure_with_rollback(
                    ConfigSaveError::AfterRenameUnconfirmed(
                        "simulated directory sync failure".to_string(),
                    ),
                    || panic!("não deve regravar config antiga"),
                ))
            },
            || panic!("não deve confirmar transação nem limpar geração anterior"),
        )
        .unwrap_err();

        assert!(error.contains("Estado incerto de config.json"));
        assert!(!error.contains("mirror revertido"));
        assert_eq!(
            local_media::mirror::read_manifest(&fixture.library).unwrap(),
            published
        );
        // Somente o fixture é limpo, depois de provar que o consumidor não fez rollback.
        publication.cleanup_previous().unwrap();
    }

    fn logged_rescan_action(
        events: &Arc<Mutex<Vec<&'static str>>>,
        name: &'static str,
        result: Result<(), &'static str>,
    ) -> impl FnMut() -> Result<(), String> {
        let events = Arc::clone(events);
        move || {
            events.lock().unwrap().push(name);
            result.map_err(str::to_string)
        }
    }

    fn logged_config_commit(
        events: &Arc<Mutex<Vec<&'static str>>>,
        name: &'static str,
        result: Result<(), &'static str>,
    ) -> impl FnMut() -> Result<(), ConfigSaveError> {
        let mut action = logged_rescan_action(events, name, result);
        move || action().map_err(ConfigSaveError::BeforeRename)
    }

    fn logged_restore_action(
        events: &Arc<Mutex<Vec<&'static str>>>,
        name: &'static str,
        result: Result<RestorePreviousOutcome, &'static str>,
    ) -> impl FnMut(Result<(), String>) -> Result<RestorePreviousOutcome, String> {
        let events = Arc::clone(events);
        move |_| {
            events.lock().unwrap().push(name);
            result.clone().map_err(str::to_string)
        }
    }

    pub(crate) fn fake_restorative_rescan_mpd(
        responses: Vec<(impl AsRef<str> + Send + 'static, impl AsRef<str> + Send + 'static)>,
    ) -> (String, thread::JoinHandle<()>) {
        let id = NEXT_LOCAL_PATH_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("/tmp/smr{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            for (expected, response) in responses {
                let expected = expected.as_ref();
                let response = response.as_ref();
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.23.5\n").unwrap();
                let mut command = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut command).unwrap();
                if expected.contains('\n') {
                    loop {
                        let mut line = String::new();
                        assert!(reader.read_line(&mut line).unwrap() > 0);
                        command.push_str(&line);
                        if line == "command_list_end\n" { break; }
                    }
                }
                assert_eq!(command, format!("{expected}\n"));
                stream.write_all(response.as_bytes()).unwrap();
            }
            fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    #[test]
    fn provisional_preview_job_is_scoped_to_managed_refresh_wait() {
        let fixture = LocalPathFixture::new();
        let socket = fixture.root.join("preview.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(config_save_operation::Coordinator::new());
        let (accepted, op) = state.reserve().unwrap();
        op.enable_addition_preview(socket.to_str().unwrap(), &["Nova".into()]).unwrap();
        assert!(matches!(state.reserve_preview(&accepted.id), Err(local_import_preview::PreviewError::Unavailable)));
        let (ready, received) = mpsc::channel();
        let (proceed, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.24.15\n").unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut command).unwrap();
            assert_eq!(command, "status\n");
            ready.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            stream.write_all(b"state: play\nOK\n").unwrap();
        });
        let worker = thread::spawn(move || {
            let result = wait_saved_folders_job(socket.to_str().unwrap(), 7, Some(&op));
            (op, result)
        });
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        let lease = state.reserve_preview(&accepted.id).unwrap();
        assert_eq!(lease.scope.job, Some(7));
        assert_eq!(lease.scope.sources, ["Nova"]);
        assert!(lease.ensure_active().is_ok());
        proceed.send(()).unwrap();
        let (op, result) = worker.join().unwrap();
        assert_eq!(result, Ok(MpdRefreshCompletion::Completed));
        // Even before validation changes phase, the finished job disables reads.
        assert_eq!(state.snapshot().unwrap().unwrap().phase, config_save_operation::Phase::Indexing);
        assert_eq!(lease.ensure_active(), Err(local_import_preview::PreviewError::Unavailable));
        assert!(matches!(state.reserve_preview(&accepted.id), Err(local_import_preview::PreviewError::Unavailable)));
        drop(lease); op.finish(Ok(()), false); server.join().unwrap();
    }

    fn install_test_managed_generation(library: &Path, roots: &[PathBuf]) {
        let staging = library.with_file_name("library.next");
        let prepared = match local_media::mirror::prepare_managed_generation(
            library,
            &staging,
            roots,
            "folders-test-initial",
        )
        .unwrap()
        {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            local_media::mirror::ManagedGenerationPreparation::Unchanged => {
                panic!("a fixture inicial deveria preparar uma geração")
            }
        };
        let publication =
            local_media::mirror::publish_prepared_managed_generation(library, &prepared).unwrap();
        publication.cleanup_previous().unwrap();
    }

    fn prepare_test_managed_change(
        library: &Path,
        roots: &[PathBuf],
        generation_id: &str,
    ) -> local_media::mirror::ManagedMirrorPublication {
        let staging = library.with_file_name("library.next");
        let prepared = match local_media::mirror::prepare_managed_generation(
            library,
            &staging,
            roots,
            generation_id,
        )
        .unwrap()
        {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            local_media::mirror::ManagedGenerationPreparation::Unchanged => {
                panic!("a alteração da fixture deveria preparar uma geração")
            }
        };
        local_media::mirror::publish_prepared_managed_generation(library, &prepared).unwrap()
    }

    fn assert_settings_save_added_cue_uses_targeted_refresh(multiple_files: bool) {
        let fixture = LocalPathFixture::new();
        let transactions = Arc::new(config_save_operation::Coordinator::new());
        let (accepted, operation) = transactions.reserve().unwrap();
        assert_eq!(accepted.phase, config_save_operation::Phase::Preparing);
        let old = fixture.root.join("Old");
        let added = fixture.root.join("Áudio Novo");
        let album = added.join("Álbum com espaço");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&album).unwrap();
        fs::write(old.join("old.flac"), b"old").unwrap();
        fs::write(album.join("imagem única.FLAC"), b"image").unwrap();
        let cue = if multiple_files {
            fs::write(album.join("segunda faixa.flac"), b"second").unwrap();
            "FILE \"imagem única.FLAC\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE \"segunda faixa.flac\" WAVE\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n"
        } else {
            "FILE \"imagem única.FLAC\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    INDEX 01 01:00:00\n"
        };
        fs::write(album.join("álbum.CUE"), cue).unwrap();
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&old));
        let original = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let original_link = fs::read_link(fixture.library.join("Old/old.flac")).unwrap();
        let previous_config = AppConfig {
            first_run: false,
            local_folders: vec![old.to_string_lossy().into_owned()],
            ..AppConfig::default()
        };
        let mut next_config = previous_config.clone();
        next_config
            .local_folders
            .push(added.to_string_lossy().into_owned());
        let persist = |config: &AppConfig| {
            persistence::atomic_write_private_synced(
                &fixture.root.join("config.json"),
                &serde_json::to_vec(config).unwrap(),
                "config.json",
            )
        };
        persist(&previous_config).unwrap();
        fs::write(fixture.root.join("mpd-v2.db"), b"fixture catalog").unwrap();
        let previous =
            local_media::resolve_local_sources(&fixture.library, &previous_config.local_folders)
                .unwrap();
        let staging = fixture.root.join("library.next");
        let prepared = match local_media::mirror::prepare_managed_generation(
            &fixture.library,
            &staging,
            &[old.clone(), added.clone()],
            "cue-addition",
        )
        .unwrap()
        {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            _ => panic!("a nova fonte deveria ser preparada"),
        };
        let diagnostics = SettingsSaveDiagnostics::new();
        let (plan, reason) =
            local_media::mirror::plan_added_sources_refresh_diagnostic(&fixture.library, &prepared)
                .unwrap();
        let plan = plan.unwrap();
        assert_eq!(reason, "none");
        assert_eq!(
            plan,
            local_media::mirror::ManagedRefreshPlan::Targeted {
                directories: vec!["Áudio Novo".to_string()],
                changed_files: if multiple_files { 3 } else { 2 },
            }
        );
        assert_eq!(
            added_source_save_needs_targeted_refresh(&previous, &prepared, &plan),
            Ok(true)
        );
        diagnostics.strategy.set("update_targeted");
        diagnostics.rejection.set(reason);
        let catalog = if multiple_files {
            "file: Old/old.flac\nfile: Áudio Novo/Álbum com espaço/imagem única.FLAC\nfile: Áudio Novo/Álbum com espaço/segunda faixa.flac\nfile: Áudio Novo/Álbum com espaço/álbum.CUE/track0001\nfile: Áudio Novo/Álbum com espaço/álbum.CUE/track0002\nOK\n"
        } else {
            "file: Old/old.flac\nfile: Áudio Novo/Álbum com espaço/imagem única.FLAC\nfile: Áudio Novo/Álbum com espaço/álbum.CUE/track0001\nfile: Áudio Novo/Álbum com espaço/álbum.CUE/track0002\nOK\n"
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("update \"Áudio Novo\"", "updating_db: 105\nOK\n"),
            ("status", "updating_db: 105\nstate: play\nOK\n"),
            ("status", "updating_db: 105\nstate: play\nOK\n"),
            ("status", "state: play\nOK\n"),
            ("listall", catalog),
        ]);
        assert_eq!(
            AudioEngine::wait_for_managed_mpd_update(
                &socket,
                None,
                MANUAL_RESCAN_POLL_INTERVAL,
                || operation.shutdown_requested(),
            ),
            Ok(MpdRefreshCompletion::Completed)
        );
        let intent = managed_save_intent::SaveIntent::create(
            &fixture.root,
            &previous_config,
            &next_config,
            &prepared,
        )
        .unwrap();
        let publication = intent.publish(&prepared, Some(&plan)).unwrap();
        assert!(fixture.root.join(managed_save_intent::NAME).exists());
        assert_eq!(
            local_media::mirror::read_manifest(&staging).unwrap(),
            original
        );
        let completed = Cell::new(None);
        let result = complete_published_mirror_refresh(
            || {
                request_refresh_and_audit_traced(
                    &socket,
                    &plan,
                    Some(&diagnostics),
                    Some(&operation),
                )
            },
            || -> Result<(), String> { panic!("sucesso não deve capturar reprodução") },
            || panic!("sucesso não deve parar o MPD"),
            || panic!("sucesso não deve reverter o mirror"),
            |_| panic!("sucesso não deve restaurar reprodução"),
            || panic!("sucesso não deve limpar geração rejeitada"),
            || {
                assert_eq!(diagnostics.mpd_last_confirmed.get(), "available_at_audit");
                assert_eq!(
                    transactions.snapshot().unwrap().unwrap().phase,
                    config_save_operation::Phase::Validating
                );
                assert_eq!(
                    transactions.begin().unwrap_err(),
                    config_save_operation::BUSY
                );
                assert!(fixture.root.join(managed_save_intent::NAME).exists());
                persist(&next_config)
            },
            || {
                publication.cleanup_previous()?;
                completed.set(Some(managed_save_intent::Completed::Committed));
                Ok(())
            },
        );
        let (result, uncertain) = intent.conclude_transaction(result, completed.get());
        assert_eq!(result, Ok(()));
        assert!(!uncertain);
        operation.finish(result, uncertain);
        assert_eq!(
            transactions.snapshot().unwrap().unwrap().phase,
            config_save_operation::Phase::Completed
        );
        assert!(diagnostics
            .summary(&Ok(()))
            .contains("final_strategy=update_targeted update_rejection=none outcome=ok"));
        assert!(!staging.exists());
        assert!(!fixture.root.join(managed_save_intent::NAME).exists());
        managed_save_intent::check_startup(&fixture.root).unwrap();
        let live = local_media::mirror::read_manifest(&fixture.library).unwrap();
        assert_eq!(live.sources.get("Old"), Some(&old));
        assert_eq!(live.sources.get("Áudio Novo"), Some(&added));
        assert_eq!(
            fs::read_link(fixture.library.join("Old/old.flac")).unwrap(),
            original_link
        );
        assert_eq!(
            fs::read_link(
                fixture
                    .library
                    .join("Áudio Novo/Álbum com espaço/álbum.CUE")
            )
            .unwrap(),
            album.join("álbum.CUE")
        );
        assert_eq!(
            serde_json::from_slice::<AppConfig>(
                &fs::read(fixture.root.join("config.json")).unwrap(),
            )
            .unwrap(),
            next_config
        );
        server.join().unwrap();
    }

    #[test]
    fn settings_save_added_single_image_cue_uses_targeted_refresh() {
        assert_settings_save_added_cue_uses_targeted_refresh(false);
    }

    #[test]
    fn settings_save_added_multiple_file_cue_uses_targeted_refresh() {
        assert_settings_save_added_cue_uses_targeted_refresh(true);
    }

    #[test]
    fn settings_save_rejects_legacy_addition_and_reports_cleanup_failure() {
        let fixture = LocalPathFixture::new();
        let old = fixture.root.join("Old");
        let added = fixture.root.join("Added");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&added).unwrap();
        fs::write(old.join("old.flac"), b"old").unwrap();
        fs::write(added.join("new.flac"), b"new").unwrap();
        symlink(&old, fixture.library.join("Old")).unwrap();
        let previous = local_media::resolve_local_sources(
            &fixture.library,
            &[old.to_string_lossy().into_owned()],
        ).unwrap();
        assert_eq!(previous.topology(), local_media::LocalLibraryTopology::Legacy);
        let staging = fixture.root.join("library.next");
        let prepared = match local_media::mirror::prepare_managed_generation(
            &fixture.library, &staging, &[old, added], "legacy-addition",
        ).unwrap() {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            _ => panic!("a nova fonte deveria ser preparada"),
        };
        let (plan, reason) = local_media::mirror::plan_added_sources_refresh_diagnostic(
            &fixture.library, &prepared,
        ).unwrap();
        assert_eq!(reason, "legacy");
        assert_eq!(added_source_save_needs_targeted_refresh(
            &previous, &prepared, &plan.unwrap_or(local_media::mirror::ManagedRefreshPlan::Full),
        ), Err(ADDED_SOURCE_FULL_RESCAN_REQUIRED));
        assert_eq!(reject_added_source_save_with_cleanup(ADDED_SOURCE_FULL_RESCAN_REQUIRED, || {
            Err("falha simulada de limpeza".to_string())
        }), format!("{ADDED_SOURCE_FULL_RESCAN_REQUIRED};{ADDED_SOURCE_STAGING_CLEANUP_FAILED}"));
        assert!(staging.exists());
        local_media::mirror::cleanup_prepared_managed_generation(&prepared).unwrap();
        assert!(fixture.library.join("Old").is_symlink());
    }

    #[test]
    fn settings_save_allows_safe_targeted_addition_without_changing_removals() {
        let fixture = LocalPathFixture::new();
        let old = fixture.root.join("Old");
        let added = fixture.root.join("Added");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&added).unwrap();
        fs::write(old.join("old.flac"), b"old").unwrap();
        fs::write(added.join("new.flac"), b"new").unwrap();
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&old));
        let previous = local_media::resolve_local_sources(
            &fixture.library,
            &[old.to_string_lossy().into_owned()],
        ).unwrap();
        let staging = fixture.root.join("library.next");
        let prepared = match local_media::mirror::prepare_managed_generation(
            &fixture.library, &staging, &[old.clone(), added], "targeted-addition",
        ).unwrap() {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            _ => panic!("a nova fonte deveria ser preparada"),
        };
        let (plan, _) = local_media::mirror::plan_added_sources_refresh_diagnostic(
            &fixture.library, &prepared,
        ).unwrap();
        let plan = plan.unwrap();
        assert_eq!(added_source_save_needs_targeted_refresh(&previous, &prepared, &plan), Ok(true));
        let publication = local_media::mirror::publish_targeted_added_sources_generation(
            &fixture.library, &prepared, &plan,
        ).unwrap();
        assert_eq!(publication.previous_kind(), Some(local_media::mirror::ManagedMirrorPreviousKind::Managed));
        publication.cleanup_previous().unwrap();

        let removal = match local_media::mirror::prepare_managed_generation(
            &fixture.library, &staging, std::slice::from_ref(&old), "removal",
        ).unwrap() {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            _ => panic!("a remoção deveria ser preparada"),
        };
        assert_eq!(added_source_save_needs_targeted_refresh(
            &local_media::resolve_local_sources(
                &fixture.library,
                &local_media::mirror::read_manifest(&fixture.library).unwrap()
                    .sources.values().map(|root| root.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            ).unwrap(),
            &removal,
            &local_media::mirror::ManagedRefreshPlan::Full,
        ), Ok(false));
        local_media::mirror::cleanup_managed_generation(&staging).unwrap();
    }

    #[test]
    fn experimental_folders_addition_commits_and_cleans_previous_after_audit() {
        let fixture = LocalPathFixture::new();
        let existing = fixture.root.join("existing/Music");
        let added = fixture.root.join("added/Music");
        fs::create_dir_all(&existing).unwrap();
        fs::create_dir_all(&added).unwrap();
        fs::write(existing.join("old.flac"), b"old").unwrap();
        fs::write(added.join("new.dsf"), b"new").unwrap();
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&existing));
        let old_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let old_id = old_manifest
            .sources
            .iter()
            .find_map(|(id, root)| (root == &existing).then(|| id.clone()))
            .unwrap();
        let publication = prepare_test_managed_change(
            &fixture.library,
            &[existing.clone(), added.clone()],
            "folders-test-added",
        );
        let live_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        assert_eq!(live_manifest.sources.get(&old_id), Some(&existing));
        assert!(live_manifest.sources.values().any(|root| root == &added));

        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 81\nOK\n"),
            ("status", "state: stop\nOK\n"),
            (
                "listall",
                "file: Music/old.flac\nfile: Music (2)/new.dsf\nOK\n",
            ),
        ]);
        let committed = Cell::new(false);
        let result = complete_published_mirror_refresh(
            || request_rescan_wait_and_audit(&socket),
            || Ok(()),
            || panic!("o caminho de sucesso não deve parar o MPD"),
            || panic!("o caminho de sucesso não deve reverter o mirror"),
            |_| Err("restore inesperado".to_string()),
            || publication.cleanup_rejected_after_rollback(),
            || {
                committed.set(true);
                Ok(())
            },
            || publication.cleanup_previous(),
        );

        assert_eq!(result, Ok(()));
        assert!(committed.get());
        assert!(!fixture.root.join("library.next").exists());
        server.join().unwrap();
    }

    #[test]
    fn experimental_folders_cleanup_failure_reports_committed_config_without_recovery() {
        let fixture = LocalPathFixture::new();
        let existing = fixture.root.join("Existing");
        let added = fixture.root.join("Added");
        fs::create_dir_all(&existing).unwrap();
        fs::create_dir_all(&added).unwrap();
        fs::write(existing.join("old.flac"), b"old").unwrap();
        fs::write(added.join("new.flac"), b"new").unwrap();
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&existing));
        let previous_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let publication = prepare_test_managed_change(
            &fixture.library,
            &[existing.clone(), added.clone()],
            "folders-test-cleanup-failure",
        );
        let previous_path = fixture.root.join("library.next");
        let evidence = previous_path.join("unexpected-entry");
        fs::write(&evidence, b"preserve").unwrap();

        let committed = Cell::new(false);
        let error = complete_published_mirror_refresh(
            || Ok(()),
            || -> Result<(), String> {
                panic!("cleanup pós-commit não deve capturar reprodução")
            },
            || panic!("cleanup pós-commit não deve parar o MPD"),
            || panic!("cleanup pós-commit não deve reverter o mirror"),
            |_| panic!("cleanup pós-commit não deve restaurar reprodução"),
            || panic!("cleanup pós-commit não deve limpar geração rejeitada"),
            || {
                committed.set(true);
                Ok(())
            },
            || publication.cleanup_previous(),
        )
        .unwrap_err();

        assert!(committed.get());
        assert!(error.contains("A transação foi confirmada"));
        assert!(error.contains("geração anterior não pôde ser limpa"));
        assert!(!error.contains("Estado anterior restaurado"));
        assert_eq!(fs::read(&evidence).unwrap(), b"preserve");
        assert_eq!(
            local_media::mirror::read_manifest(&previous_path).unwrap(),
            previous_manifest
        );
        assert!(local_media::mirror::read_manifest(&fixture.library)
            .unwrap()
            .sources
            .values()
            .any(|root| root == &added));
        assert_eq!(fs::read(existing.join("old.flac")).unwrap(), b"old");
    }

    #[test]
    fn experimental_folders_removal_publishes_only_survivors() {
        let fixture = LocalPathFixture::new();
        let kept = fixture.root.join("kept/Keep");
        let removed = fixture.root.join("removed/Remove");
        fs::create_dir_all(&kept).unwrap();
        fs::create_dir_all(&removed).unwrap();
        fs::write(kept.join("keep.flac"), b"keep").unwrap();
        fs::write(removed.join("remove.flac"), b"remove").unwrap();
        install_test_managed_generation(&fixture.library, &[kept.clone(), removed.clone()]);
        let old_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let kept_id = old_manifest
            .sources
            .iter()
            .find_map(|(id, root)| (root == &kept).then(|| id.clone()))
            .unwrap();
        let publication = prepare_test_managed_change(
            &fixture.library,
            std::slice::from_ref(&kept),
            "folders-test-removed",
        );

        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 82\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Keep/keep.flac\nOK\n"),
        ]);
        complete_published_mirror_refresh(
            || request_rescan_wait_and_audit(&socket),
            || Ok(()),
            || panic!("o caminho de sucesso não deve parar o MPD"),
            || publication.rollback(),
            |_| Err("restore inesperado".to_string()),
            || publication.cleanup_rejected_after_rollback(),
            || Ok(()),
            || publication.cleanup_previous(),
        )
        .unwrap();

        let live_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        assert_eq!(live_manifest.sources.len(), 1);
        assert_eq!(live_manifest.sources.get(&kept_id), Some(&kept));
        assert!(removed.join("remove.flac").exists());
        server.join().unwrap();
    }

    #[test]
    fn experimental_folders_failure_rolls_back_and_never_commits_config() {
        let fixture = LocalPathFixture::new();
        let existing = fixture.root.join("Existing");
        let added = fixture.root.join("Added");
        fs::create_dir_all(&existing).unwrap();
        fs::create_dir_all(&added).unwrap();
        fs::write(existing.join("old.flac"), b"old").unwrap();
        fs::write(added.join("new.flac"), b"new").unwrap();
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&existing));
        let old_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let publication = prepare_test_managed_change(
            &fixture.library,
            &[existing.clone(), added.clone()],
            "folders-test-rollback",
        );
        let (socket, server) = fake_restorative_rescan_mpd(vec![(
            "rescan",
            "ACK [5@0] {rescan} simulated failure\n",
        )]);
        let committed = Cell::new(false);
        let restored = Cell::new(false);

        let error = complete_published_mirror_refresh(
            || request_rescan_wait_and_audit(&socket),
            || Ok(()),
            || Ok(()),
            || publication.rollback(),
            |_| {
                restored.set(true);
                Ok(RestorePreviousOutcome::Restored)
            },
            || publication.cleanup_rejected_after_rollback(),
            || {
                committed.set(true);
                Ok(())
            },
            || publication.cleanup_previous(),
        )
        .unwrap_err();

        assert!(error.contains("mirror revertido"));
        assert!(error.contains("MPD reiniciado"));
        assert!(error.contains("catálogo anterior atualizado"));
        assert!(error.contains("reprodução restaurada"));
        assert!(restored.get());
        assert!(!committed.get());
        assert_eq!(
            local_media::mirror::read_manifest(&fixture.library).unwrap(),
            old_manifest
        );
        assert!(!fixture.root.join("library.next").exists());
        assert_eq!(fs::read(existing.join("old.flac")).unwrap(), b"old");
        assert_eq!(fs::read(added.join("new.flac")).unwrap(), b"new");
        server.join().unwrap();
    }

    #[test]
    fn experimental_folders_persistence_failure_rolls_back_after_successful_refresh() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = complete_published_mirror_refresh(
            logged_rescan_action(&events, "refresh", Ok(())),
            logged_rescan_action(&events, "capture", Ok(())),
            logged_rescan_action(&events, "stop", Ok(())),
            logged_rescan_action(&events, "rollback", Ok(())),
            logged_restore_action(&events, "restore", Ok(RestorePreviousOutcome::Restored)),
            logged_rescan_action(&events, "cleanup-rejected", Ok(())),
            logged_config_commit(&events, "persist", Err("disco cheio")),
            logged_rescan_action(&events, "cleanup-previous", Ok(())),
        )
        .unwrap_err();

        assert!(result.contains("disco cheio"));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "refresh",
                "persist",
                "capture",
                "stop",
                "rollback",
                "restore",
                "cleanup-rejected"
            ]
        );
    }

    #[test]
    fn managed_save_before_rename_then_catalog_recovery_failure_remains_uncertain() {
        use std::os::unix::fs::MetadataExt;

        let fixture = LocalPathFixture::new();
        let old = fixture.root.join("Old");
        let added = fixture.root.join("Added");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&added).unwrap();
        let old_track = old.join("old.flac");
        let added_track = added.join("new.flac");
        fs::write(&old_track, b"old physical audio").unwrap();
        fs::write(&added_track, b"new physical audio").unwrap();
        let identity = |path: &Path| {
            let metadata = fs::symlink_metadata(path).unwrap();
            (metadata.dev(), metadata.ino())
        };
        let physical_snapshot = |path: &Path| {
            let metadata = fs::metadata(path).unwrap();
            (
                fs::read(path).unwrap(),
                identity(path),
                metadata.mtime(),
                metadata.mtime_nsec(),
            )
        };
        let old_audio = physical_snapshot(&old_track);
        let added_audio = physical_snapshot(&added_track);
        install_test_managed_generation(&fixture.library, std::slice::from_ref(&old));
        let old_manifest = local_media::mirror::read_manifest(&fixture.library).unwrap();
        let old_identity = identity(&fixture.library);
        let previous_config = AppConfig {
            first_run: false,
            local_folders: vec![old.to_string_lossy().into_owned()],
            ..AppConfig::default()
        };
        let mut next_config = previous_config.clone();
        next_config
            .local_folders
            .push(added.to_string_lossy().into_owned());
        let config_path = fixture.root.join("config.json");
        let old_config_bytes = serde_json::to_vec(&previous_config).unwrap();
        persistence::atomic_write_private_synced(&config_path, &old_config_bytes, "config.json")
            .unwrap();
        fs::write(fixture.root.join("mpd-v2.db"), b"fixture catalog").unwrap();
        let staging = fixture.root.join("library.next");
        let prepared = match local_media::mirror::prepare_managed_generation(
            &fixture.library,
            &staging,
            &[old.clone(), added.clone()],
            "incomplete-catalog-recovery",
        )
        .unwrap()
        {
            local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) => prepared,
            _ => panic!("a nova fonte deveria preparar uma geração"),
        };
        let new_manifest = local_media::mirror::read_manifest(&staging).unwrap();
        let new_identity = identity(&staging);
        let (plan, _) =
            local_media::mirror::plan_added_sources_refresh_diagnostic(&fixture.library, &prepared)
                .unwrap();
        let plan = plan.unwrap();
        let transactions = Arc::new(config_save_operation::Coordinator::new());
        let (_, operation) = transactions.reserve().unwrap();
        let intent = managed_save_intent::SaveIntent::create(
            &fixture.root,
            &previous_config,
            &next_config,
            &prepared,
        )
        .unwrap();
        let journal_path = fixture.root.join(managed_save_intent::NAME);
        let journal_bytes = fs::read(&journal_path).unwrap();
        let publication = intent.publish(&prepared, Some(&plan)).unwrap();
        assert_eq!(identity(&fixture.library), new_identity);
        assert_eq!(identity(&staging), old_identity);

        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("update \"Added\"", "updating_db: 111\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Old/old.flac\nfile: Added/new.flac\nOK\n"),
            ("rescan", "updating_db: 112\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Old/program.prg\nOK\n"),
        ]);
        let events = RefCell::new(Vec::new());
        let stopped = Cell::new(false);
        let completed = Cell::new(None);
        let result = complete_published_mirror_refresh(
            || {
                events.borrow_mut().push("refresh");
                request_refresh_and_audit_traced(&socket, &plan, None, Some(&operation))
            },
            || {
                events.borrow_mut().push("capture");
                Ok(())
            },
            || {
                // Simulate the supervisor boundary, without starting or stopping a process.
                events.borrow_mut().push("stop");
                stopped.set(true);
                Ok(())
            },
            || {
                events.borrow_mut().push("rollback");
                assert!(stopped.get());
                publication.rollback()
            },
            |snapshot| {
                events.borrow_mut().push("recover-catalog");
                assert_eq!(snapshot, Ok(()));
                assert!(stopped.get());
                assert_eq!(identity(&fixture.library), old_identity);
                request_rescan_wait_and_audit(&socket)?;
                Ok(RestorePreviousOutcome::Restored)
            },
            || {
                events.borrow_mut().push("cleanup-rejected");
                publication.cleanup_rejected_after_rollback()?;
                completed.set(Some(managed_save_intent::Completed::Recovered));
                Ok(())
            },
            || {
                events.borrow_mut().push("persist");
                assert_eq!(
                    transactions.snapshot().unwrap().unwrap().phase,
                    config_save_operation::Phase::Validating
                );
                assert_eq!(identity(&fixture.library), new_identity);
                assert_eq!(fs::read(&journal_path).unwrap(), journal_bytes);
                Err(ConfigSaveError::BeforeRename("simulated disk full".into()))
            },
            || {
                events.borrow_mut().push("cleanup-previous");
                publication.cleanup_previous()?;
                completed.set(Some(managed_save_intent::Completed::Committed));
                Ok(())
            },
        );
        assert_eq!(
            *events.borrow(),
            [
                "refresh",
                "persist",
                "capture",
                "stop",
                "rollback",
                "recover-catalog"
            ]
        );
        assert!(completed.get().is_none());
        let (result, uncertain) = intent.conclude_transaction(result, completed.get());
        let error = result.as_ref().unwrap_err();
        assert!(error.contains("simulated disk full"));
        assert!(error.contains("1 URI(s) local(is) inválida(s)"));
        assert!(error.contains("conclusão não validada"));
        assert!(error.contains(managed_save_intent::BLOCKED));
        assert!(uncertain);
        operation.finish(result, uncertain);
        let outcome = transactions.snapshot().unwrap().unwrap();
        assert_eq!(outcome.phase, config_save_operation::Phase::Uncertain);
        assert!(outcome.error.is_some());
        assert!(!transactions.active());
        assert!(matches!(
            transactions.reserve(),
            Err(error) if error == managed_save_intent::BLOCKED
        ));
        assert_eq!(
            transactions.begin().unwrap_err(),
            managed_save_intent::BLOCKED
        );

        let startup_called = Cell::new(false);
        let startup_error = managed_save_intent::guard_startup(&fixture.root, || {
            startup_called.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(startup_error.contains(managed_save_intent::BLOCKED));
        assert!(!startup_called.get());
        assert_eq!(fs::read(&config_path).unwrap(), old_config_bytes);
        assert_eq!(
            serde_json::from_slice::<AppConfig>(&fs::read(&config_path).unwrap()).unwrap(),
            previous_config
        );
        assert_eq!(identity(&fixture.library), old_identity);
        assert_eq!(
            local_media::mirror::read_manifest(&fixture.library).unwrap(),
            old_manifest
        );
        assert_eq!(identity(&staging), new_identity);
        assert_eq!(
            local_media::mirror::read_manifest(&staging).unwrap(),
            new_manifest
        );
        assert_eq!(fs::read(&journal_path).unwrap(), journal_bytes);
        assert_eq!(
            fs::read_link(fixture.library.join("Old/old.flac")).unwrap(),
            old_track
        );
        assert_eq!(
            fs::read_link(staging.join("Added/new.flac")).unwrap(),
            added_track
        );
        assert_eq!(physical_snapshot(&old_track), old_audio);
        assert_eq!(physical_snapshot(&added_track), added_audio);
        server.join().unwrap();
    }

    #[test]
    fn legacy_rollback_requires_completed_rescan_but_tolerates_preexisting_prg() {
        let legacy_audit = AudioEngine::audit_raw_local_catalog(&[
            "file: Legacy/Album/program.prg".to_string(),
        ])
        .unwrap();
        assert!(!legacy_audit.is_clean());

        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 71\nOK\n"),
            ("status", "state: stop\nOK\n"),
        ]);
        assert_eq!(request_rescan_and_wait(&socket), Ok(()));
        server.join().unwrap();
    }

    #[test]
    fn managed_rollback_still_rejects_invalid_catalog_after_rescan() {
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 72\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Managed/Album/program.prg\nOK\n"),
        ]);
        let error = request_rescan_wait_and_audit(&socket).unwrap_err();
        assert!(error.contains("1 URI(s) local(is) inválida(s)"));
        server.join().unwrap();
    }

    #[test]
    fn startup_targeted_refresh_updates_only_affected_directory() {
        let plan = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Music/Álbum com espaço".to_string()],
            changed_files: 1,
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            (
                "update \"Music/Álbum com espaço\"",
                "updating_db: 91\nOK\n",
            ),
            ("status", "state: stop\nOK\n"),
            (
                "listall",
                "file: Music/Álbum com espaço/faixa.flac\nOK\n",
            ),
        ]);

        assert_eq!(request_startup_refresh_and_audit(&socket, &plan), Ok(()));
        server.join().unwrap();
    }

    #[test]
    fn manual_check_uses_targeted_update_and_keeps_full_rescan_explicit() {
        let plan = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Music/Álbum com espaço".to_string()],
            changed_files: 1,
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            (
                "update \"Music/Álbum com espaço\"",
                "updating_db: 93\nOK\n",
            ),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Álbum com espaço/faixa.flac\nOK\n"),
        ]);
        assert_eq!(
            refresh_published_managed_generation(ManagedMirrorRefreshMode::Check, &socket, &plan),
            Ok(())
        );
        server.join().unwrap();

        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 94\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Álbum com espaço/faixa.flac\nOK\n"),
        ]);
        assert_eq!(
            refresh_published_managed_generation(ManagedMirrorRefreshMode::Manual, &socket, &plan),
            Ok(())
        );
        server.join().unwrap();
    }

    #[test]
    fn settings_save_updates_only_new_source_ids_and_audits() {
        let diagnostics = SettingsSaveDiagnostics::new();
        let plan = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Added One".to_string(), "Added \"Two\"".to_string()],
            changed_files: 2,
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("update \"Added One\"", "updating_db: 101\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("update \"Added \\\"Two\\\"\"", "updating_db: 102\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Added One/one.flac\nfile: Added \"Two\"/two.flac\nOK\n"),
        ]);

        assert_eq!(
            refresh_saved_folders(
                &socket,
                &plan,
                local_media::mirror::ManagedMirrorPreviousKind::Managed,
                Some(&diagnostics),
            ),
            Ok(())
        );
        assert_eq!(diagnostics.mpd_last_confirmed.get(), "available_at_audit");
        server.join().unwrap();
    }

    #[test]
    fn settings_save_full_fallback_keeps_rescan_and_audit() {
        let diagnostics = SettingsSaveDiagnostics::new();
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 103\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Old/old.flac\nOK\n"),
        ]);

        assert_eq!(
            refresh_saved_folders(
                &socket,
                &local_media::mirror::ManagedRefreshPlan::Full,
                local_media::mirror::ManagedMirrorPreviousKind::Managed,
                Some(&diagnostics),
            ),
            Ok(())
        );
        assert_eq!(diagnostics.mpd_last_confirmed.get(), "available_at_audit");
        server.join().unwrap();

        let targeted = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Added".to_string()],
            changed_files: 1,
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 104\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Old/old.flac\nOK\n"),
        ]);
        assert_eq!(
            refresh_saved_folders(
                &socket,
                &targeted,
                local_media::mirror::ManagedMirrorPreviousKind::Legacy,
                None,
            ),
            Ok(())
        );
        server.join().unwrap();
    }

    fn removal_test_plan() -> local_media::mirror::PureSourceRemoval {
        local_media::mirror::PureSourceRemoval {
            removed_ids: vec!["Remove".into()],
            remaining_ids: vec!["Keep".into()],
            refresh_plan: local_media::mirror::ManagedRefreshPlan::Targeted {
                directories: vec!["Remove".into()], changed_files: 2,
            },
        }
    }

    const REMOVAL_EMPTY_STATUS: &str = "playlist: 1\nplaylistlength: 0\nstate: stop\nOK\n";
    const REMOVAL_QUEUE: &str = "file: Keep/Álbum/one.flac\nPos: 0\nId: 7\nfile: Keep/Álbum/one.flac\nPos: 1\nId: 8\nfile: Keep/Álbum/Imagem.CUE/track0001\nPos: 2\nId: 9\nOK\n";
    const REMOVAL_QUEUE_CHANGED_ID: &str = "file: Keep/Álbum/one.flac\nPos: 0\nId: 7\nfile: Keep/Álbum/one.flac\nPos: 1\nId: 8\nfile: Keep/Álbum/Imagem.CUE/track0001\nPos: 2\nId: 99\nOK\n";
    const REMOVAL_QUEUE_PLAY: &str = "command_list_begin\nclear\nadd \"Keep/Álbum/one.flac\"\nadd \"Keep/Álbum/one.flac\"\nadd \"Keep/Álbum/Imagem.CUE/track0001\"\nplay 0\ncommand_list_end";
    const REMOVAL_QUEUE_PLAYING: &str = "playlist: 31\nplaylistlength: 3\nstate: play\nsong: 0\nelapsed: 12\nOK\n";
    const REMOVAL_QUEUE_PAUSED: &str = "playlist: 31\nplaylistlength: 3\nstate: pause\nsong: 1\nelapsed: 17\nOK\n";
    const REMOVAL_QUEUE_ENDED: &str = "playlist: 31\nplaylistlength: 3\nstate: stop\nOK\n";

    fn removal_queue_uris() -> Vec<String> {
        vec!["Keep/Álbum/one.flac".into(), "Keep/Álbum/one.flac".into(),
            "Keep/Álbum/Imagem.CUE/track0001".into()]
    }
    const REMOVAL_BEFORE: &str = "directory: Keep\ndirectory: Keep/Álbum\ndirectory: Keep/Álbum/Imagem.CUE\nplaylist: Keep/Álbum/Imagem.CUE\ndirectory: Remove\ndirectory: Remove/Imagem.cue\nplaylist: Remove/Imagem.cue\nfile: Keep/Álbum/one.flac\nfile: Keep/Álbum/Imagem.CUE/track0001\nfile: Keep/Álbum/Imagem.CUE/track0002\nfile: Remove/old.flac\nfile: Remove/Imagem.cue/track0001\nOK\n";
    const REMOVAL_AFTER: &str = "directory: Keep\ndirectory: Keep/Álbum\ndirectory: Keep/Álbum/Imagem.CUE\nplaylist: Keep/Álbum/Imagem.CUE\nfile: Keep/Álbum/one.flac\nfile: Keep/Álbum/Imagem.CUE/track0001\nfile: Keep/Álbum/Imagem.CUE/track0002\nOK\n";

    const CLEAR_PLAY: &str = "command_list_begin\nclear\nadd \"Keep/Álbum/one.flac\"\nadd \"Remove/old.flac\"\nadd \"Remove/old.flac\"\nadd \"Remove/Imagem.cue/track0001\"\nplay 0\ncommand_list_end";
    const CLEAR_RESTORE: &str = "command_list_begin\nclear\nadd \"Keep/Álbum/one.flac\"\nadd \"Remove/old.flac\"\nadd \"Remove/old.flac\"\nadd \"Remove/Imagem.cue/track0001\"\nplay 1\npause 1\ncommand_list_end";
    const CLEAR_RESTORE_STOP: &str = "command_list_begin\nclear\nadd \"Keep/Álbum/one.flac\"\nadd \"Remove/old.flac\"\nadd \"Remove/old.flac\"\nadd \"Remove/Imagem.cue/track0001\"\ncommand_list_end";
    const CLEAR_QUEUE: &str = "file: Keep/Álbum/one.flac\nPos: 0\nId: 71\nfile: Remove/old.flac\nPos: 1\nId: 72\nfile: Remove/old.flac\nPos: 2\nId: 73\nfile: Remove/Imagem.cue/track0001\nPos: 3\nId: 74\nOK\n";
    const CLEAR_NEW_ID: &str = "file: Keep/Álbum/one.flac\nPos: 0\nId: 171\nfile: Remove/old.flac\nPos: 1\nId: 72\nfile: Remove/old.flac\nPos: 2\nId: 73\nfile: Remove/Imagem.cue/track0001\nPos: 3\nId: 74\nOK\n";
    const CLEAR_PLAYING: &str =
        "playlist: 31\nplaylistlength: 4\nstate: play\nsong: 1\nelapsed: 17\nOK\n";
    const CLEAR_PAUSED: &str =
        "playlist: 31\nplaylistlength: 4\nstate: pause\nsong: 1\nelapsed: 17\nOK\n";
    const CLEAR_STOPPED: &str = "playlist: 31\nplaylistlength: 4\nstate: stop\nOK\n";
    const CLEAR_EMPTY: &str = "playlist: 32\nplaylistlength: 0\nstate: stop\nOK\n";
    const CLEAR_RESTORED_PAUSED: &str =
        "playlist: 34\nplaylistlength: 4\nstate: pause\nsong: 1\nelapsed: 17\nOK\n";
    const CLEAR_RESTORED_STOP: &str = "playlist: 34\nplaylistlength: 4\nstate: stop\nOK\n";
    const CLEAR_RESUMED: &str =
        "playlist: 34\nplaylistlength: 4\nstate: play\nsong: 1\nelapsed: 17.1\nOK\n";
    const CLEAR_PROGRESS: &str =
        "playlist: 34\nplaylistlength: 4\nstate: play\nsong: 1\nelapsed: 17.2\nOK\n";

    struct ConfirmedRemovalFixture {
        paths: LocalPathFixture,
        previous: AppConfig,
        next: AppConfig,
        prepared: local_media::mirror::MirrorBuildResult,
        plan: local_media::mirror::PureSourceRemoval,
    }

    impl ConfirmedRemovalFixture {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let paths = LocalPathFixture::new();
            fs::set_permissions(&paths.root, fs::Permissions::from_mode(0o700)).unwrap();
            let kept = paths.root.join("Keep");
            let removed = paths.root.join("Remove");
            fs::create_dir_all(kept.join("Álbum")).unwrap();
            fs::create_dir(&removed).unwrap();
            fs::write(kept.join("Álbum/one.flac"), b"kept fixture").unwrap();
            fs::write(removed.join("old.flac"), b"removed fixture").unwrap();
            fs::write(
                removed.join("Imagem.cue"),
                b"FILE \"old.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n",
            )
            .unwrap();
            install_test_managed_generation(&paths.library, &[kept.clone(), removed.clone()]);
            let previous = AppConfig {
                first_run: false,
                local_folders: vec![
                    kept.to_string_lossy().into(),
                    removed.to_string_lossy().into(),
                ],
                ..AppConfig::default()
            };
            let mut next = previous.clone();
            next.local_folders.pop();
            persistence::atomic_write_private_synced(
                &paths.root.join("config.json"),
                &serde_json::to_vec(&previous).unwrap(),
                "config.json",
            )
            .unwrap();
            fs::write(paths.root.join("mpd-v2.db"), b"simulated database").unwrap();
            let local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) =
                local_media::mirror::prepare_managed_generation(
                    &paths.library,
                    &paths.root.join("library.next"),
                    &[kept],
                    "confirmed-removal",
                )
                .unwrap()
            else {
                panic!("prepared removal")
            };
            let plan = local_media::mirror::plan_removed_sources_refresh(&paths.library, &prepared)
                .unwrap()
                .unwrap();
            Self {
                paths,
                previous,
                next,
                prepared,
                plan,
            }
        }

        fn audio(&self, socket: &str) -> Mutex<AudioEngine> {
            let mut audio = AudioEngine::new(socket, &self.paths.library.to_string_lossy());
            audio
                .play_uris(
                    vec![
                        "Keep/Álbum/one.flac".into(),
                        "Remove/old.flac".into(),
                        "Remove/old.flac".into(),
                        "Remove/Imagem.cue/track0001".into(),
                    ],
                    0,
                )
                .unwrap();
            Mutex::new(audio)
        }

        fn blocked(&self) {
            assert!(managed_save_intent::guard_startup(
                &self.paths.root,
                || -> Result<(), String> { panic!("pending intent must fence startup") }
            )
            .is_err());
            assert_eq!(
                fs::read(self.paths.root.join("Keep/Álbum/one.flac")).unwrap(),
                b"kept fixture"
            );
            assert_eq!(
                fs::read(self.paths.root.join("Remove/old.flac")).unwrap(),
                b"removed fixture"
            );
        }
    }

    fn clear_admission_script(
        script: &mut Vec<(&'static str, &'static str)>,
        status: &'static str,
        confirmed: bool,
    ) {
        script.extend([
            ("status", status),
            ("playlistinfo", CLEAR_QUEUE),
            ("status", status),
            ("listall", REMOVAL_BEFORE),
            ("status", status),
            ("playlistinfo", CLEAR_QUEUE),
            ("status", status),
        ]);
        if confirmed {
            script.extend([
                ("status", status),
                ("playlistinfo", CLEAR_QUEUE),
                ("status", status),
            ]);
        }
    }

    fn clear_effect_script(
        script: &mut Vec<(&'static str, &'static str)>,
        status: &'static str,
        ack: &'static str,
    ) {
        script.extend([
            ("status", status),
            ("playlistinfo", CLEAR_QUEUE),
            ("status", status),
            ("clear", ack),
        ]);
        if ack == "OK\n" {
            for _ in 0..2 {
                script.extend([
                    ("status", CLEAR_EMPTY),
                    ("playlistinfo", "OK\n"),
                    ("status", CLEAR_EMPTY),
                ]);
            }
        }
    }

    fn obtain_clear_authorization(
        fixture: &ConfirmedRemovalFixture,
        audio: &Mutex<AudioEngine>,
        socket: &str,
    ) -> QueueClearAuthorization {
        let error = admit_confirmed_queue_removal(
            audio,
            socket,
            &fixture.previous,
            &fixture.next,
            &fixture.plan,
            None,
        )
        .err()
        .unwrap();
        let binding = error
            .strip_prefix("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED:")
            .unwrap();
        assert_eq!(binding.len(), 64);
        assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        QueueClearAuthorization {
            confirmed: true,
            binding: binding.into(),
        }
    }

    #[test]
    fn confirmed_queue_removal_requires_explicit_fresh_authorization_and_checks_both_queues() {
        let fixture = ConfirmedRemovalFixture::new();
        let mut script = vec![(CLEAR_PLAY, "OK\n")];
        for _ in 0..3 {
            clear_admission_script(&mut script, CLEAR_PLAYING, false);
        }
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        let mut auth = obtain_clear_authorization(&fixture, &audio, &socket);
        auth.confirmed = false;
        assert_eq!(
            admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &fixture.next,
                &fixture.plan,
                Some(&auth)
            )
            .err()
            .unwrap(),
            "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE"
        );
        auth.confirmed = true;
        let mut changed = fixture.next.clone();
        changed.audio_buffer_size_kb += 1;
        assert_eq!(
            admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &changed,
                &fixture.plan,
                Some(&auth)
            )
            .err()
            .unwrap(),
            "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE"
        );
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        assert!(fixture.paths.library.join("Remove").exists());
        let logical = audio.lock().unwrap().get_queue();
        let empty = audio::MpdQueueStructure {
            entries: vec![],
            playlist_version: 0,
        };
        assert!(queue_references_removed_sources(
            &fixture.plan,
            &logical,
            &empty
        ));
        let only_mpd = audio::MpdQueueStructure {
            entries: vec![audio::MpdQueueEntry {
                uri: "Remove/Imagem.cue/track0001".into(),
                pos: 0,
                id: 1,
            }],
            playlist_version: 1,
        };
        assert!(queue_references_removed_sources(
            &fixture.plan,
            &[],
            &only_mpd
        ));
        let owner = audio.lock().unwrap().reserve_queue_structure().unwrap();
        assert_eq!(
            admit_confirmed_queue_removal(
                &audio, &socket, &fixture.previous, &fixture.next, &fixture.plan, Some(&auth),
            ).err().unwrap(),
            "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE"
        );
        drop(owner);
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        server.join().unwrap();
    }

    #[test]
    fn confirmed_queue_removal_binding_covers_ids_order_duplicates_and_removed_sources() {
        let fixture = ConfirmedRemovalFixture::new();
        let mut script = vec![(CLEAR_PLAY, "OK\n")];
        clear_admission_script(&mut script, CLEAR_PAUSED, false);
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        let auth = obtain_clear_authorization(&fixture, &audio, &socket);
        // Rebuild the same snapshot in memory to isolate the binding contract.
        let tracks = audio.lock().unwrap().get_queue();
        let queue = audio::MpdQueueStructure {
            entries: tracks
                .iter()
                .enumerate()
                .map(|(pos, track)| audio::MpdQueueEntry {
                    uri: track.uri.clone(),
                    pos,
                    id: 71 + pos as u64,
                })
                .collect(),
            playlist_version: 31,
        };
        let playback = MpdPlaybackSnapshot {
            uris: tracks.iter().map(|t| t.uri.clone()).collect(),
            current_index: Some(1),
            elapsed: 17.0,
            state: audio::PlaybackState::Paused,
            playlist_version: 31,
        };
        let backup =
            managed_save_intent::QueueBackup::new(&tracks, &playback.uris, &queue, &playback)
                .unwrap();
        assert_eq!(
            queue_clear_binding(&fixture.previous, &fixture.next, &fixture.plan, &backup).unwrap(),
            auth.binding
        );
        for mutation in 0..4 {
            let mut backup = backup.clone();
            let mut plan = fixture.plan.clone();
            match mutation {
                0 => backup.mpd[0].id += 100,
                1 => backup.tracks.swap(0, 1),
                2 => backup.playlist_version += 1,
                _ => plan.removed_ids.push("Other".into()),
            }
            assert_ne!(
                queue_clear_binding(&fixture.previous, &fixture.next, &plan, &backup).unwrap(),
                auth.binding
            );
        }
        server.join().unwrap();
    }

    #[test]
    fn confirmed_queue_removal_refuses_stopped_cursor_that_existing_restore_cannot_prove() {
        let fixture = ConfirmedRemovalFixture::new();
        let status = "playlist: 31\nplaylistlength: 4\nstate: stop\nsong: 1\nelapsed: 17\nOK\n";
        let mut script = vec![(CLEAR_PLAY, "OK\n")];
        clear_admission_script(&mut script, status, false);
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        assert_eq!(
            admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &fixture.next,
                &fixture.plan,
                None
            )
            .err()
            .unwrap(),
            "LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED"
        );
        assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
        assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        server.join().unwrap();
    }

    #[test]
    fn confirmed_queue_removal_preserves_separate_logical_resume_before_any_destructive_effect() {
        let fixture = ConfirmedRemovalFixture::new();
        let mut script = vec![(CLEAR_PLAY, "OK\n"), ("status", CLEAR_PLAYING)];
        clear_admission_script(&mut script, CLEAR_PLAYING, false);
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        audio.lock().unwrap().persist_shutdown_resume().unwrap();
        assert!(audio.lock().unwrap().queue_has_logical_resume());
        assert_eq!(
            admit_confirmed_queue_removal(
                &audio, &socket, &fixture.previous, &fixture.next, &fixture.plan, None,
            )
            .err()
            .unwrap(),
            "LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED"
        );
        assert!(audio.lock().unwrap().queue_has_logical_resume());
        assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
        assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        assert!(fixture.paths.library.join("Remove").exists());
        server.join().unwrap();
    }

    #[test]
    fn confirmed_queue_removal_old_binding_cannot_clear_changed_mpd_queue() {
        let fixture = ConfirmedRemovalFixture::new();
        let mut script = vec![(CLEAR_PLAY, "OK\n")];
        clear_admission_script(&mut script, CLEAR_PLAYING, false);
        let mut changed = Vec::new();
        clear_admission_script(&mut changed, CLEAR_PLAYING, false);
        for response in &mut changed {
            if response.0 == "playlistinfo" {
                response.1 = CLEAR_NEW_ID;
            }
        }
        script.extend(changed);
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        let auth = obtain_clear_authorization(&fixture, &audio, &socket);
        assert_eq!(
            admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &fixture.next,
                &fixture.plan,
                Some(&auth)
            )
            .err()
            .unwrap(),
            "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE"
        );
        assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        assert!(fixture.paths.library.join("Remove").exists());
        server.join().unwrap();
    }

    #[test]
    fn confirmed_queue_removal_cache_failure_and_ambiguous_clear_never_publish_or_release_fence() {
        for fault in ["cache", "ack", "eof", "ack_no_effect", "shutdown", "journal"] {
            let fixture = ConfirmedRemovalFixture::new();
            let mut script = vec![(CLEAR_PLAY, "OK\n")];
            clear_admission_script(&mut script, CLEAR_PAUSED, false);
            clear_admission_script(&mut script, CLEAR_PAUSED, true);
            if fault != "shutdown" && fault != "journal" {
                if fault == "ack_no_effect" {
                    script.extend([
                        ("status", CLEAR_PAUSED),
                        ("playlistinfo", CLEAR_QUEUE),
                        ("status", CLEAR_PAUSED),
                        ("clear", "OK\n"),
                    ]);
                    for _ in 0..2 {
                        script.extend([
                            ("status", CLEAR_PAUSED),
                            ("playlistinfo", CLEAR_QUEUE),
                            ("status", CLEAR_PAUSED),
                        ]);
                    }
                } else {
                    clear_effect_script(
                        &mut script,
                        CLEAR_PAUSED,
                        match fault {
                            "ack" => "ACK [5@0] {clear} simulated refusal\n",
                            "eof" => "",
                            _ => "OK\n",
                        },
                    );
                }
                if fault == "cache" {
                    script.extend([
                        ("listall", REMOVAL_BEFORE),
                        (CLEAR_RESTORE, "OK\n"),
                        ("seekcur 17.000", "OK\n"),
                        ("listall", REMOVAL_BEFORE),
                    ]);
                    for _ in 0..2 {
                        script.extend([
                            ("status", CLEAR_RESTORED_PAUSED),
                            ("playlistinfo", CLEAR_NEW_ID),
                            ("status", CLEAR_RESTORED_PAUSED),
                        ]);
                    }
                    script.push(("status", CLEAR_RESTORED_PAUSED));
                }
            }
            let (socket, server) = fake_restorative_rescan_mpd(script);
            let audio = fixture.audio(&socket);
            let auth = obtain_clear_authorization(&fixture, &audio, &socket);
            let mut confirmed = admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &fixture.next,
                &fixture.plan,
                Some(&auth),
            )
            .unwrap()
            .unwrap();
            let mut intent = managed_save_intent::SaveIntent::create_with_queue_backup(
                &fixture.paths.root,
                &fixture.previous,
                &fixture.next,
                &fixture.prepared,
                &confirmed.backup,
            )
            .unwrap();
            let coordinator = Arc::new(config_save_operation::Coordinator::new());
            let (_, operation) = coordinator.reserve().unwrap();
            if fault == "cache" {
                fs::create_dir(fixture.paths.root.join("queue_cache.json")).unwrap();
            }
            if fault == "shutdown" {
                coordinator.request_shutdown(|| {});
            }
            if fault == "journal" {
                fs::write(fixture.paths.root.join(managed_save_intent::NAME), b"tampered backup").unwrap();
            }
            let result =
                clear_confirmed_removal_queue(&mut confirmed, &audio, &mut intent, &operation);
            assert!(result.is_err());
            if fault == "cache" {
                assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
            }
            audio
                .lock()
                .unwrap()
                .retain_uncertain_queue_reservation(&mut confirmed.owner);
            operation.finish(result, true);
            drop(confirmed);
            assert!(audio.lock().unwrap().clear_queue().is_err());
            assert!(audio
                .lock()
                .unwrap()
                .play_uris(vec!["Keep/Álbum/one.flac".into()], 0)
                .is_err());
            assert!(audio.lock().unwrap().play_index(0, false).is_err());
            assert!(coordinator.reserve().is_err());
            if fault == "journal" {
                assert!(managed_save_intent::SaveIntent::read_queue_backup(&fixture.paths.root).is_err());
                assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
            } else {
                assert!(managed_save_intent::SaveIntent::read_queue_backup(&fixture.paths.root)
                    .unwrap().is_some());
            }
            assert!(fixture.paths.library.join("Remove").exists());
            assert!(fixture.paths.root.join("library.next").exists());
            assert_eq!(
                AppConfig::load_from_path(&fixture.paths.root.join("config.json")).unwrap(),
                fixture.previous
            );
            fixture.blocked();
            server.join().unwrap();
        }
    }

    #[test]
    fn confirmed_queue_removal_shutdown_after_audited_refresh_keeps_backup_and_generations() {
        assert_queue_removal_shutdown_after_audited_refresh(false);
    }

    #[test]
    fn logical_resume_removal_shutdown_after_audited_refresh_keeps_backup_and_generations() {
        assert_queue_removal_shutdown_after_audited_refresh(true);
    }

    fn assert_queue_removal_shutdown_after_audited_refresh(logical_resume: bool) {
        let fixture = ConfirmedRemovalFixture::new();
        let mut script = vec![(CLEAR_PLAY, "OK\n")];
        let status = if logical_resume {
            CLEAR_STOPPED
        } else {
            CLEAR_PLAYING
        };
        if logical_resume {
            script.push(("status", CLEAR_PAUSED));
        } else {
            clear_admission_script(&mut script, status, false);
        }
        clear_admission_script(&mut script, status, true);
        if logical_resume {
            script.extend([
                ("status", status),
                ("playlistinfo", CLEAR_QUEUE),
                ("status", status),
            ]);
        }
        clear_effect_script(&mut script, status, "OK\n");
        script.extend([
            ("update \"Remove\"", "updating_db: 142\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", REMOVAL_AFTER),
            ("listall", REMOVAL_AFTER),
            ("status", CLEAR_EMPTY),
            ("playlistinfo", "OK\n"),
            ("status", CLEAR_EMPTY),
        ]);
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        let auth = if logical_resume {
            audio.lock().unwrap().persist_shutdown_resume().unwrap();
            None
        } else {
            Some(obtain_clear_authorization(&fixture, &audio, &socket))
        };
        let mut confirmed = admit_confirmed_queue_removal(
            &audio,
            &socket,
            &fixture.previous,
            &fixture.next,
            &fixture.plan,
            auth.as_ref(),
        )
        .unwrap()
        .unwrap();
        let mut intent = managed_save_intent::SaveIntent::create_with_queue_backup(
            &fixture.paths.root,
            &fixture.previous,
            &fixture.next,
            &fixture.prepared,
            &confirmed.backup,
        )
        .unwrap();
        let coordinator = Arc::new(config_save_operation::Coordinator::new());
        let (_, operation) = coordinator.reserve().unwrap();
        clear_confirmed_removal_queue(&mut confirmed, &audio, &mut intent, &operation).unwrap();
        let publication = intent
            .publish_removal(&fixture.prepared, &fixture.plan)
            .unwrap();
        request_removal_refresh_and_validate(
            &socket,
            &fixture.plan,
            &RemovalRefreshExpectation {
                catalog: confirmed.catalog_after.clone(),
                queue: confirmed.queue.clone(),
            },
            None,
            Some(&operation),
        )
        .unwrap();
        coordinator.request_shutdown(|| {});
        let result = check_managed_save_shutdown(&operation);
        assert!(result.is_err());
        audio
            .lock()
            .unwrap()
            .retain_uncertain_queue_reservation(&mut confirmed.owner);
        operation.finish(result, true);
        drop(confirmed);
        drop(publication);
        fixture.blocked();
        assert!(fixture.paths.root.join("library.next/Remove").exists());
        assert!(!fixture.paths.library.join("Remove").exists());
        assert_eq!(
            AppConfig::load_from_path(&fixture.paths.root.join("config.json")).unwrap(),
            fixture.previous
        );
        assert!(audio.lock().unwrap().reserve_queue_structure().is_none());
        if logical_resume {
            assert!(coordinator.reserve().is_err());
            assert_eq!(
                coordinator.snapshot().unwrap().unwrap().phase,
                config_save_operation::Phase::Uncertain
            );
            assert!(audio.lock().unwrap().play(false).is_err());
            assert!(audio.lock().unwrap().seek(2.0, false).is_err());
            let backup = managed_save_intent::SaveIntent::read_queue_backup(&fixture.paths.root)
                .unwrap()
                .unwrap();
            assert_eq!(backup.logical_resume.unwrap().queue_index, 1);
        }
        server.join().unwrap();
    }

    #[test]
    fn confirmed_queue_removal_pending_job_allows_queries_and_fences_structural_commands_through_shutdown(
    ) {
        for shutdown in [false, true] {
            let fixture = ConfirmedRemovalFixture::new();
            let socket_path = fixture.paths.root.join("controlled.sock");
            let listener = UnixListener::bind(&socket_path).unwrap();
            let socket = socket_path.to_string_lossy().into_owned();
            let release = Arc::new(AtomicBool::new(false));
            let released = Arc::clone(&release);
            let (poll_tx, poll_rx) = mpsc::channel();
            let mut prefix = vec![(CLEAR_PLAY, "OK\n")];
            clear_admission_script(&mut prefix, CLEAR_PLAYING, false);
            clear_admission_script(&mut prefix, CLEAR_PLAYING, true);
            clear_effect_script(&mut prefix, CLEAR_PLAYING, "OK\n");
            let server = thread::spawn(move || {
                let mut prefix = prefix.into_iter();
                let mut indexing = false;
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    stream.write_all(b"OK MPD 0.24.15\n").unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut command = String::new();
                    reader.read_line(&mut command).unwrap();
                    if command == "command_list_begin\n" {
                        loop {
                            let mut line = String::new();
                            reader.read_line(&mut line).unwrap();
                            command.push_str(&line);
                            if line == "command_list_end\n" {
                                break;
                            }
                        }
                    }
                    let response = if let Some((expected, response)) = prefix.next() {
                        assert_eq!(command.trim_end(), expected);
                        response
                    } else {
                        match command.trim_end() {
                            "update \"Remove\"" => {
                                assert!(!indexing);
                                indexing = true;
                                "updating_db: 150\nOK\n"
                            }
                            "status" if indexing && !released.load(Ordering::SeqCst) => {
                                poll_tx.send(()).unwrap();
                                "playlist: 32\nplaylistlength: 0\nstate: stop\nupdating_db: 150\nOK\n"
                            }
                            "status" => CLEAR_EMPTY,
                            "playlistinfo" => "OK\n",
                            "listall" => REMOVAL_AFTER,
                            "test_finished" => break,
                            other => panic!("unexpected structural or playback command: {other}"),
                        }
                    };
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });
            let audio = Arc::new(fixture.audio(&socket));
            let auth = obtain_clear_authorization(&fixture, &audio, &socket);
            let mut confirmed = admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &fixture.next,
                &fixture.plan,
                Some(&auth),
            )
            .unwrap()
            .unwrap();
            let mut intent = managed_save_intent::SaveIntent::create_with_queue_backup(
                &fixture.paths.root,
                &fixture.previous,
                &fixture.next,
                &fixture.prepared,
                &confirmed.backup,
            )
            .unwrap();
            let coordinator = Arc::new(config_save_operation::Coordinator::new());
            let (_, operation) = coordinator.reserve().unwrap();
            clear_confirmed_removal_queue(&mut confirmed, &audio, &mut intent, &operation).unwrap();
            let publication = intent
                .publish_removal(&fixture.prepared, &fixture.plan)
                .unwrap();
            let worker_audio = Arc::clone(&audio);
            let worker_socket = socket.clone();
            let plan = fixture.plan.clone();
            let config_dir = fixture.paths.root.clone();
            let next = fixture.next.clone();
            let worker = thread::spawn(move || {
                let result = request_removal_refresh_and_validate(
                    &worker_socket,
                    &plan,
                    &RemovalRefreshExpectation {
                        catalog: confirmed.catalog_after.clone(),
                        queue: confirmed.queue.clone(),
                    },
                    None,
                    Some(&operation),
                )
                .and_then(|_| check_managed_save_shutdown(&operation));
                let (result, uncertain) = if result.is_ok() {
                    let result = persistence::atomic_write_private_synced(
                        &config_dir.join("config.json"),
                        &serde_json::to_vec(&next).unwrap(),
                        "config.json",
                    )
                    .map_err(|e| e.to_string())
                    .and_then(|_| publication.cleanup_previous());
                    let completed = result
                        .is_ok()
                        .then_some(managed_save_intent::Completed::Committed);
                    intent.conclude_transaction(result, completed)
                } else {
                    intent.conclude_transaction(result, None)
                };
                if uncertain {
                    worker_audio
                        .lock()
                        .unwrap()
                        .retain_uncertain_queue_reservation(&mut confirmed.owner);
                } else {
                    confirmed.owner.release_after_confirmed_completion();
                }
                operation.finish(result.clone(), uncertain);
                result
            });
            for _ in 0..3 {
                poll_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            let reader_audio = Arc::clone(&audio);
            let (read_tx, read_rx) = mpsc::channel();
            let reader = thread::spawn(move || {
                let mut audio = reader_audio.lock().unwrap();
                assert!(audio.get_mpris_status().unwrap().is_updating);
                assert!(audio.get_queue().is_empty());
                assert!(audio.clear_queue().is_err());
                assert!(audio
                    .play_tracks(vec![], vec![], 0)
                    .unwrap_err()
                    .starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
                assert!(audio
                    .play_uris(vec!["Keep/Álbum/one.flac".into()], 0)
                    .is_err());
                audio.pause().unwrap(); // Nonstructural, stopped/empty; no forced pause command.
                read_tx.send(()).unwrap();
            });
            read_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            reader.join().unwrap();
            assert!(coordinator.reserve().is_err());
            fixture.blocked();
            let waiting = if shutdown {
                let coordinator = Arc::clone(&coordinator);
                let audio = Arc::clone(&audio);
                let (done_tx, done_rx) = mpsc::channel();
                let waiter = thread::spawn(move || {
                    let _transaction = coordinator.wait_for_idle();
                    assert!(audio.lock().unwrap().get_queue().is_empty());
                    done_tx.send(()).unwrap();
                });
                assert!(done_rx.try_recv().is_err());
                Some((waiter, done_rx))
            } else {
                None
            };
            if shutdown {
                coordinator.request_shutdown(|| {});
            } else {
                release.store(true, Ordering::SeqCst);
            }
            let result = worker.join().unwrap();
            if shutdown {
                assert!(result.is_err());
                fixture.blocked();
                assert!(audio.lock().unwrap().reserve_queue_structure().is_none());
                let (waiter, done_rx) = waiting.unwrap();
                done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                waiter.join().unwrap();
            } else {
                result.unwrap();
                assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
                assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
            }
            let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            stream.write_all(b"test_finished\n").unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn confirmed_queue_removal_clears_durably_then_targets_source_in_all_playback_states() {
        for status in [CLEAR_PLAYING, CLEAR_PAUSED, CLEAR_STOPPED] {
            assert_confirmed_queue_transaction(status, "success");
        }
    }

    #[test]
    fn confirmed_queue_removal_validated_rollback_restores_queue_cursor_and_state() {
        for status in [CLEAR_PLAYING, CLEAR_PAUSED, CLEAR_STOPPED] {
            assert_confirmed_queue_transaction(status, "refresh_failure");
        }
    }

    #[test]
    fn confirmed_queue_removal_config_failure_preserves_recovered_queue_and_uncertain_intent() {
        assert_confirmed_queue_transaction(CLEAR_PAUSED, "config_failure");
    }

    #[test]
    fn confirmed_queue_removal_uncertain_config_never_rolls_back_or_releases_reservation() {
        assert_confirmed_queue_transaction(CLEAR_PLAYING, "config_uncertain");
    }

    #[test]
    fn confirmed_queue_removal_incomplete_catalog_recovery_keeps_every_resource() {
        assert_confirmed_queue_transaction(CLEAR_PAUSED, "recovery_failure");
    }

    #[test]
    fn logical_resume_removal_clears_without_confirmation_and_recovers_without_audio() {
        for resume in [CLEAR_PLAYING, CLEAR_PAUSED] {
            for outcome in [
                "success",
                "refresh_failure",
                "config_failure",
                "config_uncertain",
                "recovery_failure",
            ] {
                assert_confirmed_queue_transaction_with_resume(
                    CLEAR_STOPPED,
                    outcome,
                    Some(resume),
                );
            }
        }
    }

    #[test]
    fn logical_resume_removal_refuses_remaining_cursor_and_non_neutral_mpd() {
        const KEPT_CURSOR: &str =
            "playlist: 31\nplaylistlength: 4\nstate: pause\nsong: 0\nelapsed: 17\nOK\n";
        for (seed, actual) in [
            (KEPT_CURSOR, CLEAR_STOPPED),
            (CLEAR_PAUSED, CLEAR_PLAYING),
            (CLEAR_PAUSED, CLEAR_PAUSED),
        ] {
            let fixture = ConfirmedRemovalFixture::new();
            let mut script = vec![(CLEAR_PLAY, "OK\n"), ("status", seed)];
            if seed == KEPT_CURSOR {
                script.extend([
                    ("status", actual),
                    ("playlistinfo", CLEAR_QUEUE),
                    ("status", actual),
                ]);
            } else {
                clear_admission_script(&mut script, actual, false);
            }
            let (socket, server) = fake_restorative_rescan_mpd(script);
            let audio = fixture.audio(&socket);
            audio.lock().unwrap().persist_shutdown_resume().unwrap();
            assert_eq!(
                admit_confirmed_queue_removal(
                    &audio,
                    &socket,
                    &fixture.previous,
                    &fixture.next,
                    &fixture.plan,
                    None
                )
                .err()
                .unwrap(),
                "LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED"
            );
            assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
            assert!(audio.lock().unwrap().queue_has_logical_resume());
            audio.lock().unwrap().pause().unwrap(); // No leftover cursor fence after refusal.
            assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
            assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
            assert!(fixture.paths.library.join("Remove").exists());
            server.join().unwrap();
        }
    }

    #[test]
    fn logical_resume_removal_keeps_unaffected_queue_controls_and_existing_refresh_admission() {
        let fixture = ConfirmedRemovalFixture::new();
        let mut script = vec![
            (REMOVAL_QUEUE_PLAY, "OK\n"),
            ("status", REMOVAL_QUEUE_PAUSED),
        ];
        script.extend([
            ("status", REMOVAL_QUEUE_ENDED),
            ("playlistinfo", REMOVAL_QUEUE),
            ("status", REMOVAL_QUEUE_ENDED),
            ("listall", REMOVAL_BEFORE),
            ("status", REMOVAL_QUEUE_ENDED),
            ("playlistinfo", REMOVAL_QUEUE),
            ("status", REMOVAL_QUEUE_ENDED),
        ]);
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let mut engine = AudioEngine::new(&socket, &fixture.paths.library.to_string_lossy());
        engine.play_uris(removal_queue_uris(), 0).unwrap();
        engine.persist_shutdown_resume().unwrap();
        let audio = Mutex::new(engine);
        let guard = admit_removal_refresh(&audio, &socket, &fixture.plan)
            .unwrap()
            .unwrap();
        assert!(audio.lock().unwrap().queue_has_logical_resume());
        audio.lock().unwrap().pause().unwrap();
        audio.lock().unwrap().seek(18.0, false).unwrap();
        assert_eq!(audio.lock().unwrap().get_queue().len(), 3);
        assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
        drop(guard);
        assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
        server.join().unwrap();
    }

    #[test]
    fn logical_resume_removal_revalidates_backup_cursor_mpd_and_shutdown_before_clear() {
        for fault in ["cursor", "journal", "mpd", "mpd_ack", "shutdown", "cache"] {
            let fixture = ConfirmedRemovalFixture::new();
            let mut script = vec![(CLEAR_PLAY, "OK\n"), ("status", CLEAR_PAUSED)];
            clear_admission_script(&mut script, CLEAR_STOPPED, true);
            if matches!(fault, "mpd" | "cache") {
                let actual = if fault == "mpd" {
                    CLEAR_PLAYING
                } else {
                    CLEAR_STOPPED
                };
                script.extend([
                    ("status", actual),
                    ("playlistinfo", CLEAR_QUEUE),
                    ("status", actual),
                ]);
            }
            if fault == "mpd_ack" {
                script.push(("status", "ACK [5@0] {status} /private/SECRET\n"));
            }
            if fault == "cache" {
                clear_effect_script(&mut script, CLEAR_STOPPED, "OK\n");
                script.extend([
                    ("listall", REMOVAL_BEFORE),
                    (CLEAR_RESTORE_STOP, "OK\n"),
                    ("listall", REMOVAL_BEFORE),
                ]);
                for _ in 0..2 {
                    script.extend([
                        ("status", CLEAR_RESTORED_STOP),
                        ("playlistinfo", CLEAR_NEW_ID),
                        ("status", CLEAR_RESTORED_STOP),
                    ]);
                }
                script.push(("status", CLEAR_RESTORED_STOP));
            }
            let (socket, server) = fake_restorative_rescan_mpd(script);
            let audio = fixture.audio(&socket);
            audio.lock().unwrap().persist_shutdown_resume().unwrap();
            let mut confirmed = admit_confirmed_queue_removal(
                &audio,
                &socket,
                &fixture.previous,
                &fixture.next,
                &fixture.plan,
                None,
            )
            .unwrap()
            .unwrap();
            let mut intent = managed_save_intent::SaveIntent::create_with_queue_backup(
                &fixture.paths.root,
                &fixture.previous,
                &fixture.next,
                &fixture.prepared,
                &confirmed.backup,
            )
            .unwrap();
            let coordinator = Arc::new(config_save_operation::Coordinator::new());
            let (_, operation) = coordinator.reserve().unwrap();
            match fault {
                "cursor" => {
                    confirmed
                        .logical_resume
                        .as_mut()
                        .unwrap()
                        .resume
                        .queue_index = 4
                }
                "journal" => fs::write(
                    fixture.paths.root.join(managed_save_intent::NAME),
                    b"tampered",
                )
                .unwrap(),
                "cache" => fs::create_dir(fixture.paths.root.join("queue_cache.json")).unwrap(),
                "shutdown" => coordinator.request_shutdown(|| {}),
                _ => {}
            }
            let result =
                clear_confirmed_removal_queue(&mut confirmed, &audio, &mut intent, &operation);
            assert!(result.is_err());
            if fault == "mpd_ack" {
                assert_eq!(
                    result.as_ref().unwrap_err(),
                    "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED"
                );
            }
            assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
            assert!(audio.lock().unwrap().queue_has_logical_resume());
            audio
                .lock()
                .unwrap()
                .retain_uncertain_queue_reservation(&mut confirmed.owner);
            operation.finish(result, true);
            drop(confirmed);
            assert!(coordinator.reserve().is_err());
            assert!(audio.lock().unwrap().play(false).is_err());
            assert!(audio.lock().unwrap().seek(2.0, false).is_err());
            assert!(audio.lock().unwrap().reserve_queue_structure().is_none());
            fixture.blocked();
            assert!(fixture.paths.library.join("Remove").exists());
            assert!(fixture.paths.root.join("library.next").exists());
            assert_eq!(
                AppConfig::load_from_path(&fixture.paths.root.join("config.json")).unwrap(),
                fixture.previous
            );
            assert!(fixture.paths.root.join(managed_save_intent::NAME).exists());
            server.join().unwrap();
        }
    }

    fn assert_confirmed_queue_transaction(status: &'static str, outcome: &str) {
        assert_confirmed_queue_transaction_with_resume(status, outcome, None);
    }

    fn assert_confirmed_queue_transaction_with_resume(
        status: &'static str,
        outcome: &str,
        logical_resume: Option<&'static str>,
    ) {
        let fixture = ConfirmedRemovalFixture::new();
        let uncertain_config = outcome == "config_uncertain";
        let recover = outcome != "success" && !uncertain_config;
        let config_failure = outcome == "config_failure";
        let recovery_failure = outcome == "recovery_failure";
        let mut script = vec![(CLEAR_PLAY, "OK\n")];
        if let Some(resume) = logical_resume {
            script.push(("status", resume));
        } else {
            clear_admission_script(&mut script, status, false);
        }
        clear_admission_script(&mut script, status, true);
        if logical_resume.is_some() {
            // Revalidate MPD's neutral stopped snapshot after durable backup.
            script.extend([
                ("status", status),
                ("playlistinfo", CLEAR_QUEUE),
                ("status", status),
            ]);
        }
        clear_effect_script(&mut script, status, "OK\n");
        script.extend([
            ("update \"Remove\"", "updating_db: 140\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", REMOVAL_AFTER),
            (
                "listall",
                if recover && !config_failure {
                    REMOVAL_BEFORE
                } else {
                    REMOVAL_AFTER
                },
            ),
        ]);
        if !recover || config_failure {
            script.extend([
                ("status", CLEAR_EMPTY),
                ("playlistinfo", "OK\n"),
                ("status", CLEAR_EMPTY),
            ]);
        }
        if recover {
            script.extend([
                ("rescan", "updating_db: 141\nOK\n"),
                ("status", "state: stop\nOK\n"),
                ("listall", REMOVAL_BEFORE),
            ]);
            if !recovery_failure {
                script.push((
                    if status == CLEAR_STOPPED {
                        CLEAR_RESTORE_STOP
                    } else {
                        CLEAR_RESTORE
                    },
                    "OK\n",
                ));
                if status != CLEAR_STOPPED {
                    script.push(("seekcur 17.000", "OK\n"));
                }
                script.push(("listall", REMOVAL_BEFORE));
                let restored = if status == CLEAR_STOPPED {
                    CLEAR_RESTORED_STOP
                } else {
                    CLEAR_RESTORED_PAUSED
                };
                for _ in 0..2 {
                    script.extend([
                        ("status", restored),
                        ("playlistinfo", CLEAR_NEW_ID),
                        ("status", restored),
                    ]);
                }
                script.push(("status", restored));
                if status == CLEAR_PLAYING {
                    script.extend([
                        ("pause 0", "OK\n"),
                        ("status", CLEAR_RESUMED),
                        ("status", CLEAR_PROGRESS),
                        ("status", CLEAR_PROGRESS),
                        ("playlistinfo", CLEAR_QUEUE),
                        ("status", CLEAR_PROGRESS),
                        ("status", CLEAR_PROGRESS),
                    ]);
                }
            }
        }
        let (socket, server) = fake_restorative_rescan_mpd(script);
        let audio = fixture.audio(&socket);
        let auth = if logical_resume.is_some() {
            audio.lock().unwrap().persist_shutdown_resume().unwrap();
            None
        } else {
            Some(obtain_clear_authorization(&fixture, &audio, &socket))
        };
        let mut confirmed = admit_confirmed_queue_removal(
            &audio,
            &socket,
            &fixture.previous,
            &fixture.next,
            &fixture.plan,
            auth.as_ref(),
        )
        .unwrap()
        .unwrap();
        if logical_resume.is_some() {
            assert!(confirmed.logical_resume.is_some());
            assert_eq!(
                confirmed
                    .backup
                    .logical_resume
                    .as_ref()
                    .unwrap()
                    .queue_index,
                1
            );
            thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let mut audio = audio.lock().unwrap();
                        assert!(audio
                            .play(false)
                            .unwrap_err()
                            .starts_with("LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_RESERVED"));
                        assert!(audio
                            .seek(2.0, false)
                            .unwrap_err()
                            .starts_with("LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_RESERVED"));
                    })
                    .join()
                    .unwrap();
            });
        }
        let mut intent = managed_save_intent::SaveIntent::create_with_queue_backup(
            &fixture.paths.root,
            &fixture.previous,
            &fixture.next,
            &fixture.prepared,
            &confirmed.backup,
        )
        .unwrap();
        assert!(
            managed_save_intent::SaveIntent::read_queue_backup(&fixture.paths.root)
                .unwrap()
                .unwrap()
                == confirmed.backup
        );
        if logical_resume.is_some() {
            let record: serde_json::Value = serde_json::from_slice(
                &fs::read(fixture.paths.root.join(managed_save_intent::NAME)).unwrap(),
            )
            .unwrap();
            assert_eq!(record["intent"]["version"], 2);
            assert_eq!(record["intent"]["queue_backup"]["version"], 2);
            assert_eq!(audio.lock().unwrap().get_queue().len(), 4); // Backup before any clear.
        }
        fixture.blocked();
        let coordinator = Arc::new(config_save_operation::Coordinator::new());
        let (_, operation) = coordinator.reserve().unwrap();
        clear_confirmed_removal_queue(&mut confirmed, &audio, &mut intent, &operation).unwrap();
        assert!(audio.lock().unwrap().get_queue().is_empty());
        assert!(!audio.lock().unwrap().queue_has_logical_resume());
        assert!(audio.lock().unwrap().clear_queue().is_err());
        assert!(audio.try_lock().is_ok());
        let cache: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.paths.root.join("queue_cache.json")).unwrap())
                .unwrap();
        assert!(cache["tracks"].as_array().unwrap().is_empty());
        let publication = intent
            .publish_removal(&fixture.prepared, &fixture.plan)
            .unwrap();
        let refresh = request_removal_refresh_and_validate(
            &socket,
            &fixture.plan,
            &RemovalRefreshExpectation {
                catalog: confirmed.catalog_after.clone(),
                queue: confirmed.queue.clone(),
            },
            None,
            None,
        );
        let completed = Cell::new(None);
        let recovered = Cell::new(false);
        let persisted = Cell::new(false);
        let snapshot = confirmed.playback.clone();
        let mut refresh = Some(refresh);
        let result = complete_published_mirror_refresh(
            || refresh.take().unwrap(),
            || Ok(snapshot.clone()),
            || Ok(()),
            || publication.rollback(),
            |snapshot| {
                request_rescan_wait_and_audit(&socket)?;
                if recovery_failure {
                    return Err("simulated catalog recovery failure".into());
                }
                AudioEngine::restore_mpd_playback_snapshot(&socket, &snapshot?)?;
                confirm_removed_queue_recovery(&mut confirmed, &audio, &socket, &mut intent, true)?;
                recovered.set(true);
                Ok(RestorePreviousOutcome::Restored)
            },
            || {
                require_confirmed_queue_recovery_for_cleanup(recovered.get(), persisted.get())?;
                publication.cleanup_rejected_after_rollback()?;
                completed.set(Some(managed_save_intent::Completed::Recovered));
                Ok(())
            },
            || {
                if config_failure {
                    persisted.set(true);
                    return Err(ConfigSaveError::BeforeRename("simulated disk full".into()));
                }
                persistence::atomic_write_private_synced(
                    &fixture.paths.root.join("config.json"),
                    &serde_json::to_vec(&fixture.next).unwrap(),
                    "config.json",
                )?;
                if uncertain_config {
                    return Err(ConfigSaveError::AfterRenameUnconfirmed(
                        "simulated config directory fsync failure".into(),
                    ));
                }
                Ok(())
            },
            || {
                publication.cleanup_previous()?;
                completed.set(Some(managed_save_intent::Completed::Committed));
                Ok(())
            },
        );
        let (result, uncertain) = intent.conclude_transaction(result, completed.get());
        assert_eq!(
            uncertain,
            config_failure || recovery_failure || uncertain_config
        );
        if uncertain {
            audio
                .lock()
                .unwrap()
                .retain_uncertain_queue_reservation(&mut confirmed.owner);
        } else {
            confirmed.owner.release_after_confirmed_completion();
        }
        operation.finish(result, uncertain);
        drop(confirmed);
        assert_eq!(
            AppConfig::load_from_path(&fixture.paths.root.join("config.json")).unwrap(),
            if recover {
                fixture.previous.clone()
            } else {
                fixture.next.clone()
            }
        );
        if uncertain {
            fixture.blocked();
            assert!(coordinator.reserve().is_err());
            assert_eq!(
                coordinator.snapshot().unwrap().unwrap().phase,
                config_save_operation::Phase::Uncertain
            );
            assert!(fixture.paths.root.join("library.next").exists());
            assert!(audio.lock().unwrap().clear_queue().is_err());
        } else {
            assert!(!fixture.paths.root.join(managed_save_intent::NAME).exists());
            assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
        }
        if recover && !recovery_failure {
            assert_eq!(audio.lock().unwrap().get_queue().len(), 4);
            let cache: serde_json::Value = serde_json::from_slice(
                &fs::read(fixture.paths.root.join("queue_cache.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(cache["tracks"].as_array().unwrap().len(), 4);
            if let Some(resume) = logical_resume {
                assert!(audio.lock().unwrap().queue_has_logical_resume());
                assert_eq!(cache["resume"]["queue_index"], 1);
                assert_eq!(cache["resume"]["elapsed"], 17.0);
                assert_eq!(
                    cache["resume"]["state"],
                    if resume == CLEAR_PLAYING {
                        "playing"
                    } else {
                        "paused"
                    }
                );
                assert_eq!(cache["tracks"][1]["uri"], cache["tracks"][2]["uri"]);
            } else if status != CLEAR_STOPPED {
                assert_eq!(
                    cache["resume"]["state"],
                    if status == CLEAR_PLAYING {
                        "playing"
                    } else {
                        "paused"
                    }
                );
                assert_eq!(cache["resume"]["queue_index"], 1);
            }
        }
        assert_eq!(fixture.paths.library.join("Remove").exists(), recover);
        assert_eq!(
            fs::read(fixture.paths.root.join("Remove/old.flac")).unwrap(),
            b"removed fixture"
        );
        server.join().unwrap();
    }

    fn assert_settings_save_removal_transaction(validation_failure: bool) {
        assert_settings_save_removal_transaction_with_queue(validation_failure, None, false);
    }

    fn assert_settings_save_removal_transaction_with_queue(
        catalog_failure: bool, queue_status: Option<&'static str>, queue_failure: bool,
    ) {
        let validation_failure = catalog_failure || queue_failure;
        use std::os::unix::fs::PermissionsExt;
        let fixture = LocalPathFixture::new();
        fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o700)).unwrap();
        let kept = fixture.root.join("Keep");
        let removed = fixture.root.join("Remove");
        fs::create_dir_all(kept.join("Álbum")).unwrap();
        fs::create_dir(&removed).unwrap();
        fs::write(kept.join("Álbum/one.flac"), b"original-kept").unwrap();
        fs::write(kept.join("Álbum/Imagem.CUE"),
            b"FILE \"one.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:02:00\n").unwrap();
        fs::write(removed.join("old.flac"), b"original-removed").unwrap();
        fs::write(removed.join("Imagem.cue"),
            b"FILE \"old.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n").unwrap();
        install_test_managed_generation(&fixture.library, &[kept.clone(), removed.clone()]);
        let previous = AppConfig { first_run: false,
            local_folders: vec![kept.to_string_lossy().into(), removed.to_string_lossy().into()],
            ..AppConfig::default() };
        let mut next = previous.clone();
        next.local_folders.pop();
        persistence::atomic_write_private_synced(&fixture.root.join("config.json"),
            &serde_json::to_vec(&previous).unwrap(), "config.json").unwrap();
        fs::write(fixture.root.join("mpd-v2.db"), b"fake catalog").unwrap();
        let local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) =
            local_media::mirror::prepare_managed_generation(&fixture.library,
                &fixture.root.join("library.next"), &[kept.clone()], "removed-save").unwrap()
            else { panic!("expected removal") };
        let plan = local_media::mirror::plan_removed_sources_refresh(&fixture.library, &prepared)
            .unwrap().unwrap();
        let before_status = queue_status.unwrap_or(REMOVAL_EMPTY_STATUS);
        let playlist = if queue_status.is_some() { REMOVAL_QUEUE } else { "OK\n" };
        let mut responses = vec![
            ("status", before_status), ("playlistinfo", playlist),
            ("status", before_status), ("listall", REMOVAL_BEFORE),
            ("status", before_status), ("playlistinfo", playlist),
            ("status", before_status),
            ("update \"Remove\"", "updating_db: 121\nOK\n"),
            ("status", "updating_db: 121\nstate: stop\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", REMOVAL_AFTER),
            ("listall", if catalog_failure { REMOVAL_BEFORE } else { REMOVAL_AFTER }),
        ];
        if !catalog_failure {
            let after_status = if queue_status.is_some() { REMOVAL_QUEUE_ENDED } else { REMOVAL_EMPTY_STATUS };
            responses.extend([
                ("status", after_status),
                ("playlistinfo", if queue_failure { REMOVAL_QUEUE_CHANGED_ID } else { playlist }),
                ("status", after_status),
            ]);
        }
        if queue_status.is_some() {
            responses.insert(0, (REMOVAL_QUEUE_PLAY, "OK\n"));
        }
        let (socket, server) = fake_restorative_rescan_mpd(responses);
        let audio = Arc::new(Mutex::new(AudioEngine::new(&socket, &fixture.library.to_string_lossy())));
        if queue_status.is_some() {
            audio.lock().unwrap().play_uris(removal_queue_uris(), 0).unwrap();
        }
        let (guard, proof) = admit_removal_refresh(&audio, &socket, &plan).unwrap().unwrap();
        assert!(audio.try_lock().is_ok());
        let concurrent_audio = Arc::clone(&audio);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let contender = thread::spawn(move || {
            let mut audio = concurrent_audio.lock().unwrap();
            assert_eq!(audio.get_queue().len(), if queue_status.is_some() { 3 } else { 0 });
            assert!(audio.clear_queue().unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
            let error = audio.play_uris(vec!["Remove/old.flac".into()], 0).unwrap_err();
            entered_tx.send(error).unwrap();
        });
        assert!(entered_rx.recv_timeout(Duration::from_secs(2)).unwrap()
            .starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        contender.join().unwrap();
        let intent = managed_save_intent::SaveIntent::create(&fixture.root, &previous, &next, &prepared).unwrap();
        let publication = intent.publish_removal(&prepared, &plan).unwrap();
        assert!(!fixture.library.join("Remove").exists());
        assert!(fixture.root.join(managed_save_intent::NAME).exists());
        let refresh = request_removal_refresh_and_validate(&socket, &plan, &proof, None, None);
        assert_eq!(refresh.is_err(), validation_failure);
        drop(guard);
        assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
        let completed = Cell::new(None);
        let recovery_attempted = Cell::new(false);
        let coordinator = Arc::new(config_save_operation::Coordinator::new());
        let (_, operation) = coordinator.reserve().unwrap();
        let mut refresh = Some(refresh);
        let result = complete_published_mirror_refresh(
            || refresh.take().unwrap(),
            || -> Result<(), String> { assert!(validation_failure); Ok(()) },
            || { assert!(validation_failure); Ok(()) },
            || { assert!(validation_failure); publication.rollback() },
            |_| { recovery_attempted.set(true); Err("simulated previous catalog unavailable".into()) },
            || panic!("cleanup rejected"),
            || {
                assert!(!validation_failure);
                assert!(fixture.root.join(managed_save_intent::NAME).exists());
                persistence::atomic_write_private_synced(&fixture.root.join("config.json"),
                    &serde_json::to_vec(&next).unwrap(), "config.json")
            },
            || {
                assert!(!validation_failure);
                publication.cleanup_previous()?;
                completed.set(Some(managed_save_intent::Completed::Committed));
                Ok(())
            },
        );
        let (result, uncertain) = intent.conclude_transaction(result, completed.get());
        assert_eq!(uncertain, validation_failure);
        assert_eq!(result.is_err(), validation_failure);
        operation.finish(result, uncertain);
        assert_eq!(AppConfig::load_from_path(&fixture.root.join("config.json")).unwrap(),
            if validation_failure { previous } else { next });
        assert_eq!(fixture.root.join(managed_save_intent::NAME).exists(), validation_failure);
        assert_eq!(fixture.root.join("library.next").exists(), validation_failure);
        if validation_failure {
            assert!(recovery_attempted.get());
            assert!(fixture.library.join("Remove/old.flac").exists());
            assert!(!fixture.root.join("library.next/Remove").exists());
            assert_eq!(coordinator.snapshot().unwrap().unwrap().phase, config_save_operation::Phase::Uncertain);
            assert!(coordinator.reserve().is_err());
            assert!(managed_save_intent::guard_startup(&fixture.root,
                || -> Result<(), String> { panic!("startup must stay blocked") }).is_err());
        }
        assert_eq!(fs::read(kept.join("Álbum/one.flac")).unwrap(), b"original-kept");
        assert_eq!(fs::read(removed.join("old.flac")).unwrap(), b"original-removed");
        server.join().unwrap();
    }

    #[test]
    fn settings_save_removal_updates_absent_source_and_keeps_durable_contract() {
        assert_settings_save_removal_transaction(false);
    }

    #[test]
    fn settings_save_removal_validation_failure_preserves_uncertain_recovery() {
        assert_settings_save_removal_transaction(true);
    }

    #[test]
    fn settings_save_removal_preserves_unaffected_queue_in_play_pause_stop() {
        for status in [REMOVAL_QUEUE_PLAYING, REMOVAL_QUEUE_PAUSED, REMOVAL_QUEUE_ENDED] {
            assert_settings_save_removal_transaction_with_queue(false, Some(status), false);
        }
    }

    #[test]
    fn settings_save_removal_changed_queue_identity_preserves_uncertain_recovery() {
        assert_settings_save_removal_transaction_with_queue(false, Some(REMOVAL_QUEUE_PLAYING), true);
    }

    #[test]
    fn settings_save_removal_rejects_post_refresh_uri_order_position_and_version_changes() {
        for (status, playlist) in [
            (REMOVAL_QUEUE_PLAYING, "file: Keep/Álbum/one.flac\nPos: 0\nId: 7\nfile: Keep/Álbum/one.flac\nPos: 1\nId: 8\nfile: Keep/Álbum/Imagem.CUE/track0002\nPos: 2\nId: 9\nOK\n"),
            (REMOVAL_QUEUE_PLAYING, "file: Keep/Álbum/Imagem.CUE/track0001\nPos: 0\nId: 9\nfile: Keep/Álbum/one.flac\nPos: 1\nId: 7\nfile: Keep/Álbum/one.flac\nPos: 2\nId: 8\nOK\n"),
            (REMOVAL_QUEUE_PLAYING, "file: Keep/Álbum/one.flac\nPos: 0\nId: 7\nfile: Keep/Álbum/one.flac\nPos: 1\nId: 8\nfile: Keep/Álbum/Imagem.CUE/track0001\nPos: 3\nId: 9\nOK\n"),
            ("playlist: 32\nplaylistlength: 3\nstate: play\nsong: 0\nelapsed: 18\nOK\n", REMOVAL_QUEUE),
        ] {
            let mut responses = vec![
                (REMOVAL_QUEUE_PLAY, "OK\n"),
                ("status", REMOVAL_QUEUE_PLAYING), ("playlistinfo", REMOVAL_QUEUE),
                ("status", REMOVAL_QUEUE_PLAYING), ("listall", REMOVAL_BEFORE),
                ("status", REMOVAL_QUEUE_PLAYING), ("playlistinfo", REMOVAL_QUEUE),
                ("status", REMOVAL_QUEUE_PLAYING),
                ("update \"Remove\"", "updating_db: 121\nOK\n"),
                ("status", REMOVAL_QUEUE_PAUSED),
                ("listall", REMOVAL_AFTER), ("listall", REMOVAL_AFTER),
                ("status", status), ("playlistinfo", playlist),
            ];
            if !playlist.contains("Pos: 3") { responses.push(("status", status)); }
            let (socket, server) = fake_restorative_rescan_mpd(responses);
            let audio = Mutex::new(AudioEngine::new(&socket, "/discardable-test"));
            audio.lock().unwrap().play_uris(removal_queue_uris(), 0).unwrap();
            let plan = removal_test_plan();
            let (reservation, proof) = admit_removal_refresh(&audio, &socket, &plan).unwrap().unwrap();
            let error = request_removal_refresh_and_validate(&socket, &plan, &proof, None, None).unwrap_err();
            assert!(error.starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_"));
            assert!(audio.lock().unwrap().clear_queue().is_err());
            drop(reservation);
            assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
            server.join().unwrap();
        }
    }

    #[test]
    fn settings_save_removal_pending_job_allows_status_and_shutdown_without_queue_population() {
        assert_settings_save_removal_pending_job(false);
    }

    #[test]
    fn settings_save_removal_pending_job_keeps_unaffected_queue_and_playback_controls_available() {
        assert_settings_save_removal_pending_job(true);
    }

    fn assert_settings_save_removal_pending_job(with_queue: bool) {
        // Keep the managed job pending until explicitly released, independent
        // of elapsed time. Exercise both completion and shutdown cancellation.
        for shutdown in [false, true] {
            let fixture = LocalPathFixture::new();
            let socket = fixture.root.join("mpd.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let socket = socket.to_string_lossy().into_owned();
            let release = Arc::new(AtomicBool::new(false));
            let server_release = Arc::clone(&release);
            let (poll_tx, poll_rx) = mpsc::channel();
            let server = thread::spawn(move || {
                let mut updating = false;
                let mut catalog_reads = 0;
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    stream.write_all(b"OK MPD 0.24.15\n").unwrap();
                    let mut command = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    reader.read_line(&mut command).unwrap();
                    if command == "command_list_begin\n" {
                        loop {
                            let mut line = String::new();
                            assert!(reader.read_line(&mut line).unwrap() > 0);
                            command.push_str(&line);
                            if line == "command_list_end\n" { break; }
                        }
                        assert_eq!(command.trim_end(), REMOVAL_QUEUE_PLAY);
                        assert!(with_queue && !updating);
                        stream.write_all(b"OK\n").unwrap();
                        continue;
                    }
                    let response = match command.trim_end() {
                        "status" if updating && !server_release.load(Ordering::SeqCst) => {
                            poll_tx.send(()).unwrap();
                            if with_queue {
                                "playlist: 31\nplaylistlength: 3\nstate: play\nsong: 0\nelapsed: 12\nupdating_db: 121\nOK\n"
                            } else { "playlist: 1\nplaylistlength: 0\nstate: stop\nupdating_db: 121\nOK\n" }
                        }
                        "status" => if with_queue {
                            if updating { REMOVAL_QUEUE_PAUSED } else { REMOVAL_QUEUE_PLAYING }
                        } else { REMOVAL_EMPTY_STATUS },
                        "playlistinfo" => if with_queue { REMOVAL_QUEUE } else { "OK\n" },
                        "currentsong" if with_queue => "file: Keep/Álbum/one.flac\nPos: 0\nOK\n",
                        "play 1" | "seekcur 4.0" | "pause 1" if with_queue => "OK\n",
                        "listall" => {
                            catalog_reads += 1;
                            if updating { REMOVAL_AFTER } else { REMOVAL_BEFORE }
                        }
                        "update \"Remove\"" => {
                            assert!(!updating);
                            updating = true;
                            "updating_db: 121\nOK\n"
                        }
                        "test_finished" => {
                            assert_eq!(catalog_reads, if shutdown { 1 } else { 3 });
                            break;
                        }
                        other => panic!("unexpected MPD command: {other}"),
                    };
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });
            let audio = Arc::new(Mutex::new(AudioEngine::new(&socket, "")));
            if with_queue { audio.lock().unwrap().play_uris(removal_queue_uris(), 0).unwrap(); }
            let plan = removal_test_plan();
            let (reservation, proof) = admit_removal_refresh(&audio, &socket, &plan).unwrap().unwrap();
            let coordinator = Arc::new(config_save_operation::Coordinator::new());
            let (_, operation) = coordinator.reserve().unwrap();
            let worker_socket = socket.clone();
            let worker = thread::spawn(move || {
                let result = request_removal_refresh_and_validate(
                    &worker_socket, &plan, &proof, None, Some(&operation),
                );
                drop(reservation); // Same boundary as the production Save.
                operation.finish(result.clone(), result.is_err());
                result
            });
            for _ in 0..3 {
                poll_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            let concurrent_audio = Arc::clone(&audio);
            let (read_tx, read_rx) = mpsc::channel();
            let reader = thread::spawn(move || {
                let mut audio = concurrent_audio.lock().unwrap();
                let status = audio.get_mpris_status().unwrap();
                assert!(status.is_updating);
                assert_eq!(status.state, if with_queue { "play" } else { "stop" });
                assert_eq!(audio.get_queue().len(), if with_queue { 3 } else { 0 });
                assert!(audio.clear_queue().unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
                if with_queue {
                    audio.play_index(1, false).unwrap();
                    audio.seek(4.0, false).unwrap();
                    audio.pause().unwrap();
                }
                for uri in ["Remove/old.flac", "https://example.invalid/stream"] {
                    assert!(audio.play_uris(vec![uri.into()], 0).unwrap_err()
                        .starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
                }
                read_tx.send(()).unwrap();
            });
            read_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            reader.join().unwrap();
            assert!(coordinator.reserve().is_err());
            let shutdown_worker = shutdown.then(|| {
                let coordinator = Arc::clone(&coordinator);
                let audio = Arc::clone(&audio);
                let (done_tx, done_rx) = mpsc::channel();
                let handle = thread::spawn(move || {
                    let _transaction = coordinator.wait_for_idle();
                    assert_eq!(audio.lock().unwrap().get_queue().len(), if with_queue { 3 } else { 0 });
                    done_tx.send(()).unwrap();
                });
                assert!(done_rx.try_recv().is_err());
                (handle, done_rx)
            });
            if shutdown {
                coordinator.request_shutdown(|| {});
            } else {
                release.store(true, Ordering::SeqCst);
            }
            let result = worker.join().unwrap();
            if shutdown {
                assert!(result.unwrap_err().contains(config_save_operation::INTERRUPTED));
                assert_eq!(coordinator.snapshot().unwrap().unwrap().phase,
                    config_save_operation::Phase::Uncertain);
                // A waiting shutdown worker acquires the transaction and
                // AudioState only after the Save finishes, without a lock cycle.
                let (handle, done_rx) = shutdown_worker.unwrap();
                done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                handle.join().unwrap();
            } else {
                result.unwrap();
            }
            assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
            let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            stream.write_all(b"test_finished\n").unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn settings_save_removal_rejects_catalog_loss_stale_uris_duplicates_and_invalid_entries() {
        let plan = removal_test_plan();
        let before = REMOVAL_BEFORE.lines().filter(|line| *line != "OK")
            .map(str::to_string).collect();
        let expected = RemovalCatalogExpectation::from_before(&plan, before).unwrap();
        for invalid in [
            "file: Keep/Álbum/one.flac\nOK\n", // Lost virtual CUE tracks.
            "file: Keep/Álbum/one.flac\nfile: Keep/Álbum/Imagem.CUE/track0001\nfile: Keep/Álbum/Imagem.CUE/track0002\nfile: Remove/old.flac\nOK\n",
            "file: Keep/Álbum/one.flac\nfile: Keep/Álbum/one.flac\nOK\n",
            "file: Keep/../../secret.flac\nOK\n",
            "directory: Remove/../../secret\nOK\n",
            "playlist: Remove/Imagem.cue\nOK\n",
            "ACK [5@0] {listall} refused\n",
        ] {
            let (socket, server) = fake_restorative_rescan_mpd(vec![("listall", invalid)]);
            assert!(expected.validate(&socket).is_err());
            server.join().unwrap();
        }
        assert!(RemovalCatalogExpectation::from_before(&plan,
            ["file: Unknown/one.flac".to_string()].into_iter().collect()).is_none());
        // Prefix collisions are not treated as belonging to a removed source.
        assert!(RemovalCatalogExpectation::from_before(&plan,
            ["file: Remove Other/one.flac".to_string()].into_iter().collect()).is_none());
    }

    #[test]
    fn settings_save_removal_queue_protection_is_conservative() {
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("status", "playlist: 2\nplaylistlength: 1\nstate: pause\nsong: 0\nelapsed: 12\nOK\n"),
            ("playlistinfo", "file: Keep/one.flac\nPos: 0\nId: 17\nOK\n"),
            ("status", "playlist: 2\nplaylistlength: 1\nstate: pause\nsong: 0\nelapsed: 12\nOK\n"),
        ]);
        let audio = Mutex::new(AudioEngine::new(&socket, "/discardable-test"));
        assert!(admit_removal_refresh(&audio, &socket, &removal_test_plan()).unwrap().is_none());
        assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
        assert!(audio.try_lock().is_ok());
        server.join().unwrap(); // No catalog/update/queue mutation was requested.

        // Baseline refusal/error must also release queue admission, without
        // publishing a generation or requesting any refresh.
        for catalog in ["file: Unknown/track.flac\nOK\n", "ACK [5@0] {listall} failed\n"] {
            let (socket, server) = fake_restorative_rescan_mpd(vec![
                ("status", REMOVAL_EMPTY_STATUS), ("playlistinfo", "OK\n"),
                ("status", REMOVAL_EMPTY_STATUS), ("listall", catalog),
            ]);
            let audio = Mutex::new(AudioEngine::new(&socket, "/discardable-test"));
            let admission = admit_removal_refresh(&audio, &socket, &removal_test_plan());
            if catalog.starts_with("ACK") {
                assert!(admission.is_err());
            } else {
                assert!(admission.unwrap().is_none());
            }
            assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
            server.join().unwrap();
        }
    }

    #[test]
    fn settings_save_removal_eligibility_requires_proven_remaining_queue_entries() {
        let plan = removal_test_plan();
        let catalog = RemovalCatalogExpectation::from_before(&plan,
            REMOVAL_BEFORE.lines().filter(|line| *line != "OK").map(str::to_string).collect()).unwrap();
        let make_track = |uri: &str| TrackMetadata {
            uri: uri.into(), media_locator: Some(MediaLocator::Local { uri: uri.into() }),
            title: String::new(), artist: String::new(), album: String::new(), thumb: None,
            plex_image: None, duration: None,
        };
        let logical = removal_queue_uris().iter().map(|uri| make_track(uri)).collect::<Vec<_>>();
        let queue = audio::MpdQueueStructure {
            playlist_version: 31,
            entries: removal_queue_uris().into_iter().enumerate().map(|(pos, uri)|
                audio::MpdQueueEntry { uri, pos, id: 7 + pos as u64 }).collect(),
        };
        assert!(removal_queue_matches_remaining_sources(&plan, &logical, &queue, &catalog));
        let mut legacy_local = logical.clone();
        legacy_local[0].media_locator = None;
        assert!(removal_queue_matches_remaining_sources(&plan, &legacy_local, &queue, &catalog));
        for uri in ["Remove/old.flac", "Keep Other/a.flac", "Unknown/a.flac",
            "/physical/Keep/a.flac", "https://plex.invalid/stream", "Keep/../Remove/old.flac", "Keep/missing.flac"] {
            let mut logical = logical.clone();
            logical[0] = make_track(uri);
            let mut queue = queue.clone();
            queue.entries[0].uri = uri.into();
            assert!(!removal_queue_matches_remaining_sources(&plan, &logical, &queue, &catalog));
        }
        let mut ambiguous = logical.clone();
        ambiguous[0].media_locator = Some(MediaLocator::Local { uri: "Remove/old.flac".into() });
        assert!(!removal_queue_matches_remaining_sources(&plan, &ambiguous, &queue, &catalog));
        ambiguous[0].media_locator = Some(MediaLocator::Plex {
            server_id: "server".into(), part_key: "/library/parts/1/file.flac".into(),
            rating_key: None, file_path: Some("Keep/Álbum/one.flac".into()),
        });
        assert!(!removal_queue_matches_remaining_sources(&plan, &ambiguous, &queue, &catalog));
        let mut reordered = queue.clone();
        reordered.entries.swap(0, 2);
        assert!(!removal_queue_matches_remaining_sources(&plan, &logical, &reordered, &catalog));
        assert!(!removal_queue_matches_remaining_sources(&plan, &logical[..2], &queue, &catalog));
    }

    #[test]
    fn settings_save_removal_divergence_before_publication_releases_reservation_for_fallback() {
        let cases = vec![
            vec![("status", REMOVAL_QUEUE_PLAYING), ("playlistinfo", REMOVAL_QUEUE),
                ("status", "playlist: 32\nplaylistlength: 3\nstate: play\nsong: 0\nelapsed: 12\nOK\n")],
            vec![("status", REMOVAL_QUEUE_PLAYING), ("playlistinfo", REMOVAL_QUEUE),
                ("status", REMOVAL_QUEUE_PLAYING), ("listall", REMOVAL_BEFORE),
                ("status", REMOVAL_QUEUE_PLAYING), ("playlistinfo", REMOVAL_QUEUE_CHANGED_ID),
                ("status", REMOVAL_QUEUE_PLAYING)],
        ];
        for mut responses in cases {
            responses.insert(0, (REMOVAL_QUEUE_PLAY, "OK\n"));
            let (socket, server) = fake_restorative_rescan_mpd(responses);
            let audio = Mutex::new(AudioEngine::new(&socket, "/discardable-test"));
            audio.lock().unwrap().play_uris(removal_queue_uris(), 0).unwrap();
            assert!(admit_removal_refresh(&audio, &socket, &removal_test_plan()).unwrap().is_none());
            assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
            server.join().unwrap(); // No refresh or structural command after admission.
        }
    }

    fn assert_settings_save_targeted_failure_enters_rollback(
        responses: Vec<(&'static str, &'static str)>,
        expected_error: &str,
    ) {
        let diagnostics = SettingsSaveDiagnostics::new();
        let plan = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Added".to_string()],
            changed_files: 1,
        };
        let (socket, server) = fake_restorative_rescan_mpd(responses);
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = complete_published_mirror_refresh(
            || {
                refresh_saved_folders(
                    &socket,
                    &plan,
                    local_media::mirror::ManagedMirrorPreviousKind::Managed,
                    Some(&diagnostics),
                )
            },
            logged_rescan_action(&events, "capture", Ok(())),
            logged_rescan_action(&events, "stop", Ok(())),
            logged_rescan_action(&events, "rollback", Ok(())),
            logged_restore_action(&events, "restore", Ok(RestorePreviousOutcome::Restored)),
            logged_rescan_action(&events, "cleanup", Ok(())),
            logged_config_commit(&events, "commit", Ok(())),
            logged_rescan_action(&events, "cleanup-previous", Ok(())),
        )
        .unwrap_err();
        assert!(result.contains(expected_error));
        assert_eq!(diagnostics.mpd_last_confirmed.get(), "unknown");
        assert_eq!(
            *events.lock().unwrap(),
            ["capture", "stop", "rollback", "restore", "cleanup"]
        );
        server.join().unwrap();
    }

    #[test]
    fn settings_save_targeted_update_failure_still_enters_rollback() {
        assert_settings_save_targeted_failure_enters_rollback(
            vec![("update \"Added\"", "ACK [5@0] {update} simulated failure\n")],
            "update MPD direcionado",
        );
    }

    #[test]
    fn settings_save_targeted_cue_catalog_audit_failure_still_enters_rollback() {
        assert_settings_save_targeted_failure_enters_rollback(
            vec![
                ("update \"Added\"", "updating_db: 106\nOK\n"),
                ("status", "state: stop\nOK\n"),
                // A virtual track is accepted, but a CUE sidecar as a song is not.
                (
                    "listall",
                    "file: Added/album.CUE/track0001\nfile: Added/album.CUE\nOK\n",
                ),
            ],
            "auditoria do catálogo encontrou 1 URI(s)",
        );
    }

    #[test]
    fn settings_save_diagnostic_summary_uses_only_coded_values() {
        let diagnostics = SettingsSaveDiagnostics::new();
        let skipped = diagnostics.summary(&Ok(()));
        assert!(skipped.contains("final_strategy=skip update_rejection=not_evaluated"));
        assert!(skipped.contains("mpd_state=unknown mpd_availability=unconfirmed"));
        diagnostics.strategy.set("rescan");
        diagnostics.rejection.set("previous_mapping_changed");
        diagnostics.mpd_last_confirmed.set("stopped");
        let summary = diagnostics.summary(&Err("/private/NAS?X-Plex-Token=secret".to_string()));
        assert!(summary.contains("final_strategy=rescan update_rejection=previous_mapping_changed outcome=error"));
        assert!(summary.contains("mpd_state=stopped mpd_availability=unavailable"));
        assert!(!summary.contains("private"));
        assert!(!summary.contains("secret"));

        diagnostics.mpd_last_confirmed.set("available_at_audit");
        let summary = diagnostics.summary(&Ok(()));
        assert!(summary.contains("mpd_state=running_last_confirmed mpd_availability=last_confirmed"));
    }

    #[test]
    fn settings_save_refresh_completion_codes_do_not_expose_mpd_errors() {
        assert_eq!(refresh_completion_code(&MpdRefreshCompletion::Completed), "ok");
        assert_eq!(refresh_completion_code(&MpdRefreshCompletion::Timeout), "timeout");
        assert_eq!(
            refresh_completion_code(&MpdRefreshCompletion::ProtocolError(
                "private URI".to_string()
            )),
            "protocol_error"
        );
        assert_eq!(
            refresh_completion_code(&MpdRefreshCompletion::TransportError(
                "private path".to_string()
            )),
            "transport_error"
        );
        assert_eq!(
            refresh_completion_code(&MpdRefreshCompletion::Unavailable(
                "private socket".to_string()
            )),
            "unavailable"
        );
    }

    #[test]
    fn manual_check_full_fallback_and_refresh_failure_are_explicit() {
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 95\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Album/faixa.flac\nOK\n"),
        ]);
        assert_eq!(
            refresh_published_managed_generation(
                ManagedMirrorRefreshMode::Check,
                &socket,
                &local_media::mirror::ManagedRefreshPlan::Full,
            ),
            Ok(())
        );
        server.join().unwrap();

        let plan = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Music/Album".to_string()],
            changed_files: 1,
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![(
            "update \"Music/Album\"",
            "ACK [5@0] {update} simulated failure\n",
        )]);
        assert!(refresh_published_managed_generation(
            ManagedMirrorRefreshMode::Check,
            &socket,
            &plan,
        )
        .is_err());
        server.join().unwrap();
    }

    #[test]
    fn startup_full_refresh_fallback_keeps_rescan_and_audit() {
        let (socket, server) = fake_restorative_rescan_mpd(vec![
            ("rescan", "updating_db: 92\nOK\n"),
            ("status", "state: stop\nOK\n"),
            ("listall", "file: Music/Album/faixa.flac\nOK\n"),
        ]);

        assert_eq!(
            request_startup_refresh_and_audit(
                &socket,
                &local_media::mirror::ManagedRefreshPlan::Full,
            ),
            Ok(())
        );
        server.join().unwrap();
    }

    #[test]
    fn startup_targeted_refresh_error_is_explicit_and_stops_before_audit() {
        let plan = local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: vec!["Music/Album".to_string()],
            changed_files: 1,
        };
        let (socket, server) = fake_restorative_rescan_mpd(vec![(
            "update \"Music/Album\"",
            "ACK [5@0] {update} simulated failure\n",
        )]);

        let error = request_startup_refresh_and_audit(&socket, &plan).unwrap_err();
        assert!(error.contains("update MPD direcionado"));
        assert!(error.contains("UpdateCommandFailed"));
        server.join().unwrap();
    }

    #[test]
    fn manual_rescan_commits_only_after_refresh_and_audit_success() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = complete_published_mirror_refresh(
            logged_rescan_action(&events, "refresh", Ok(())),
            logged_rescan_action(&events, "capture", Ok(())),
            logged_rescan_action(&events, "stop", Ok(())),
            logged_rescan_action(&events, "rollback", Ok(())),
            logged_restore_action(&events, "restore", Ok(RestorePreviousOutcome::Restored)),
            logged_rescan_action(&events, "cleanup-rejected", Ok(())),
            logged_config_commit(&events, "commit", Ok(())),
            logged_rescan_action(&events, "commit-cleanup", Ok(())),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            *events.lock().unwrap(),
            ["refresh", "commit", "commit-cleanup"]
        );
    }

    #[test]
    fn manual_rescan_timeout_and_ack_trigger_complete_rollback() {
        for failure in ["timeout do rescan", "ACK do rescan"] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let result = complete_published_mirror_refresh(
                logged_rescan_action(&events, "refresh", Err(failure)),
                logged_rescan_action(&events, "capture", Ok(())),
                logged_rescan_action(&events, "stop", Ok(())),
                logged_rescan_action(&events, "rollback", Ok(())),
                logged_restore_action(&events, "restore", Ok(RestorePreviousOutcome::Restored)),
                logged_rescan_action(&events, "cleanup-rejected", Ok(())),
                logged_config_commit(&events, "commit", Ok(())),
                logged_rescan_action(&events, "commit-cleanup", Ok(())),
            )
            .unwrap_err();
            assert!(result.contains(failure));
            assert!(result.contains("mirror revertido"));
            assert!(result.contains("MPD reiniciado"));
            assert!(result.contains("catálogo anterior atualizado"));
            assert!(result.contains("reprodução restaurada"));
            assert!(!result.contains("Estado anterior restaurado"));
            assert_eq!(
                *events.lock().unwrap(),
                [
                    "refresh",
                    "capture",
                    "stop",
                    "rollback",
                    "restore",
                    "cleanup-rejected"
                ]
            );
        }
    }

    #[test]
    fn failed_catalog_recovery_reports_mpd_unavailable_only_after_confirmed_stop() {
        let timeout = "Tempo esgotado aguardando o rescan do MPD.".to_string();
        let stopped = report_failed_previous_catalog_refresh(timeout.clone(), Ok(()));
        assert!(stopped.contains(&timeout));
        assert!(stopped.contains("catálogo anterior não pôde ser atualizado e validado"));
        assert!(stopped.contains("MPD de recuperação foi encerrado e está indisponível"));
        assert!(stopped.contains("reprodução não restaurada"));
        assert!(!stopped.contains("Estado anterior restaurado"));

        let uncertain = report_failed_previous_catalog_refresh(
            timeout.clone(),
            Err("stop recusado".to_string()),
        );
        assert!(uncertain.contains(&timeout));
        assert!(uncertain.contains("stop recusado"));
        assert!(uncertain.contains("disponibilidade do motor indeterminada"));
        assert!(!uncertain.contains("está indisponível"));
        assert!(!uncertain.contains("Estado anterior restaurado"));
    }

    #[test]
    fn managed_recovery_failure_keeps_original_timeout_and_does_not_claim_playback_restored() {
        let result = complete_published_mirror_refresh(
            || Err("timeout do rescan original".to_string()),
            || Ok(()),
            || Ok(()),
            || Ok(()),
            |_| Err(report_failed_previous_catalog_refresh(
                "timeout do rescan restaurador".to_string(),
                Ok(()),
            )),
            || Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap_err();

        assert!(result.contains("timeout do rescan original"));
        assert!(result.contains("timeout do rescan restaurador"));
        assert!(result.contains("mirror anterior foi revertido"));
        assert!(result.contains("MPD de recuperação foi encerrado e está indisponível"));
        assert!(!result.contains("reprodução restaurada"));
    }

    #[test]
    fn recovery_uses_playback_captured_after_refresh_failure() {
        let current_queue = Cell::new("fila anterior");
        let restored_queue = RefCell::new(String::new());

        let result = complete_published_mirror_refresh(
            || {
                current_queue.set("fila alterada durante refresh");
                Err("timeout".to_string())
            },
            || Ok(current_queue.get().to_string()),
            || Ok(()),
            || Ok(()),
            |snapshot| {
                *restored_queue.borrow_mut() = snapshot.unwrap();
                Ok(RestorePreviousOutcome::Restored)
            },
            || Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap_err();

        assert!(result.contains("mirror revertido"));
        assert!(result.contains("MPD reiniciado"));
        assert_eq!(
            restored_queue.borrow().as_str(),
            "fila alterada durante refresh"
        );
    }

    #[test]
    fn capture_failure_rolls_back_mirror_without_replacing_playback() {
        let playback_restore_called = Cell::new(false);
        let mirror_rolled_back = Cell::new(false);
        let rejected_cleaned = Cell::new(false);

        let result = complete_published_mirror_refresh(
            || Err("timeout".to_string()),
            || Err::<(), _>("captura indisponível".to_string()),
            || Ok(()),
            || {
                mirror_rolled_back.set(true);
                Ok(())
            },
            |snapshot| match snapshot {
                Ok(()) => {
                    playback_restore_called.set(true);
                    Ok(RestorePreviousOutcome::Restored)
                }
                Err(error) => Ok(RestorePreviousOutcome::PlaybackIncomplete(error)),
            },
            || {
                rejected_cleaned.set(true);
                Ok(())
            },
            || Ok(()),
            || Ok(()),
        )
        .unwrap_err();

        assert!(result.contains("MPD reiniciado"));
        assert!(result.contains("catálogo anterior atualizado"));
        assert!(result.contains("reprodução não foi restaurada"));
        assert!(result.contains("captura indisponível"));
        assert!(mirror_rolled_back.get());
        assert!(rejected_cleaned.get());
        assert!(!playback_restore_called.get());
    }

    #[test]
    fn manual_rescan_stop_failure_refuses_mirror_rollback() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = complete_published_mirror_refresh(
            logged_rescan_action(&events, "refresh", Err("timeout")),
            logged_rescan_action(&events, "capture", Ok(())),
            logged_rescan_action(&events, "stop", Err("processo ainda ativo")),
            logged_rescan_action(&events, "rollback", Ok(())),
            logged_restore_action(&events, "restore", Ok(RestorePreviousOutcome::Restored)),
            logged_rescan_action(&events, "cleanup-rejected", Ok(())),
            logged_config_commit(&events, "commit", Ok(())),
            logged_rescan_action(&events, "commit-cleanup", Ok(())),
        )
        .unwrap_err();
        assert!(result.contains("processo ainda ativo"));
        assert!(result.contains("mirror não foi revertido"));
        assert_eq!(*events.lock().unwrap(), ["refresh", "capture", "stop"]);
    }

    #[test]
    fn manual_rescan_reports_incomplete_recovery_without_cleanup() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = complete_published_mirror_refresh(
            logged_rescan_action(&events, "refresh", Err("ACK")),
            logged_rescan_action(&events, "capture", Ok(())),
            logged_rescan_action(&events, "stop", Ok(())),
            logged_rescan_action(&events, "rollback", Ok(())),
            logged_restore_action(&events, "restore", Err("audit restauradora falhou")),
            logged_rescan_action(&events, "cleanup-rejected", Ok(())),
            logged_config_commit(&events, "commit", Ok(())),
            logged_rescan_action(&events, "commit-cleanup", Ok(())),
        )
        .unwrap_err();
        assert!(result.contains("recuperação"));
        assert!(result.contains("audit restauradora falhou"));
        assert!(result.contains("mirror anterior foi revertido"));
        assert!(!result.contains("reprodução restaurada"));
        assert_eq!(
            *events.lock().unwrap(),
            ["refresh", "capture", "stop", "rollback", "restore"]
        );
    }

    #[test]
    fn shutdown_during_recovery_rolls_back_and_cleans_without_restart() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let result = complete_published_mirror_refresh(
            logged_rescan_action(&events, "refresh", Err("timeout")),
            logged_rescan_action(&events, "capture", Ok(())),
            logged_rescan_action(&events, "stop", Ok(())),
            logged_rescan_action(&events, "rollback", Ok(())),
            logged_restore_action(
                &events,
                "skip-restart",
                Ok(RestorePreviousOutcome::SkippedForShutdown),
            ),
            logged_rescan_action(&events, "cleanup-rejected", Ok(())),
            logged_config_commit(&events, "commit", Ok(())),
            logged_rescan_action(&events, "cleanup-previous", Ok(())),
        )
        .unwrap_err();

        assert!(result.contains("não foram recuperados porque a aplicação está sendo encerrada"));
        assert!(!result.contains("MPD reiniciado"));
        assert_eq!(
            *events.lock().unwrap(),
            [
                "refresh",
                "capture",
                "stop",
                "rollback",
                "skip-restart",
                "cleanup-rejected"
            ]
        );
    }

    #[test]
    fn shutdown_recovery_cleanup_failure_does_not_claim_mpd_restored() {
        let result = complete_published_mirror_refresh(
            || Err("timeout".to_string()),
            || Ok(()),
            || Ok(()),
            || Ok(()),
            |_| Ok(RestorePreviousOutcome::SkippedForShutdown),
            || Err("cleanup recusado".to_string()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap_err();

        assert!(result.contains("mirror revertido"));
        assert!(result.contains("MPD, catálogo e reprodução não foram recuperados"));
        assert!(result.contains("cleanup recusado"));
        assert!(!result.contains("MPD reiniciado"));
        assert!(!result.contains("Estado anterior restaurado"));
    }
}
