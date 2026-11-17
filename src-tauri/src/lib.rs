#[cfg(all(feature = "experimental-package", dev))]
compile_error!("experimental-package must be built with `tauri build` so it embeds the production frontend");

mod ab_comparison;
mod ab_commands;
mod direct_volume;
mod direct_playback;
mod alsa_preflight;
mod alsa_observation;
mod analyzer;
mod artwork_progress;
mod audio;
mod playback_options;
mod signal_processing;
mod catalog_genres;
mod config;
mod music_sources;
mod equalizer;
mod equalizer_portable;
mod equalizer_commands;
mod shared_dsp;
mod config_save_operation;
mod diagnostics;
mod local_diagnostics;
mod bounded_artwork_io;
mod favorites;
mod library_doctor;
mod library_recovery;
mod recovery_fs;
mod source_retirement;
mod recovery_catalog;
mod recovery_sandbox;
mod recovery_set;
mod profile_lease;
mod local_media;
mod library_refresh;
mod manual_check_playback;
mod source_identity;
mod source_availability;
mod local_import_preview;
mod local_import_artwork;
mod local_watcher;
mod index_coverage;
mod initial_plex_playback;
mod local_changes;
mod local_own_writes;
#[allow(dead_code)]
mod managed_preparation;
mod managed_save_intent;
mod first_run_managed;
#[cfg(test)]
mod initial_background_tests;
mod mpris;
mod upnp;
mod upnp_protocol;
mod upnp_bridge;
mod remote;
mod remote_http;
mod remote_bridge;
mod mpris_shutdown;
mod online_artwork;
mod system_tray;
mod online_metadata;
mod persistence;
mod playlists;
mod listening_history;
mod scrobbling;
mod smart_playlists;
mod radio;
mod navidrome;
mod sleep_timer;
mod plex;
mod shared_volume;
mod shared_mute;
mod supervisor;
mod mpd_engine;
mod installed_runtimes;
mod tags;
mod local_file_info;
mod album_file_sizes;
mod player_source_info;
mod sqlite_library;

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
use sqlite_library::SqliteLibraryState;
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

#[derive(Default)]
struct ProfileServiceRestoreState {
    attempted: AtomicBool,
    shutdown_started: AtomicBool,
    lifecycle: Mutex<()>,
}

impl ProfileServiceRestoreState {
    fn run_once_if(&self, eligible: bool, shutdown_requested: bool, restore: impl FnOnce()) -> bool {
        if !eligible || shutdown_requested || self.shutdown_started.load(Ordering::Acquire) {
            return false;
        }
        let Ok(_lifecycle) = self.lifecycle.lock() else {
            return false;
        };
        if shutdown_requested
            || self.shutdown_started.load(Ordering::Acquire)
            || self
                .attempted
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        restore();
        true
    }

    fn begin_shutdown(&self) {
        if let Ok(_lifecycle) = self.lifecycle.lock() {
            self.shutdown_started.store(true, Ordering::Release);
        }
    }
}

static AUDIO_ROUTE_SNAPSHOT_GENERATION: AtomicU64 = AtomicU64::new(0);
pub struct ConfigTransactionState {
    operations: Arc<config_save_operation::Coordinator>,
    initial_plex: initial_plex_playback::Gate,
    addition_playback: initial_plex_playback::Gate,
    manual_check_playback: Arc<manual_check_playback::Gate>,
    startup_config_read_gate: Mutex<bool>,
    source_mirror_cache: source_availability::MirrorReadCache,
    restart_gate: Mutex<()>,
    preparation: managed_preparation::ManagedPreparationCoordinator,
    lifecycle: AtomicU8,
    shutdown_worker_started: AtomicBool,
    shutdown_complete: AtomicBool,
}

struct RestartPermit<'a> {
    _guard: std::sync::MutexGuard<'a, ()>,
}

// Only a validated, live startup reconciliation or explicit check may own library.next.
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

fn should_start_mpd_for_null_test(
    config_valid: bool,
    config: &AppConfig,
    null_output_test: bool,
) -> bool {
    null_output_test || should_start_mpd_on_startup(config_valid, config)
}

const NULL_OUTPUT_TEST_ENV: &str = "SONANTE_064_TEST_NULL_OUTPUT";

fn parse_null_output_test_mode(value: Option<&std::ffi::OsStr>, debug_build: bool) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(value) if value == "1" && debug_build => Ok(true),
        Some(value) if value == "1" => Err("NULL_OUTPUT_TEST_REQUIRES_DEVELOPMENT_BUILD".into()),
        Some(_) => Err("NULL_OUTPUT_TEST_VALUE_INVALID".into()),
    }
}

fn requested_null_output_test_mode() -> Result<bool, String> {
    parse_null_output_test_mode(std::env::var_os(NULL_OUTPUT_TEST_ENV).as_deref(), cfg!(debug_assertions))
}

pub(crate) fn null_output_test_mode() -> bool {
    requested_null_output_test_mode().unwrap_or(false)
}

#[cfg(test)]
mod null_output_test_mode_tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn null_output_mode_requires_an_exact_opt_in_and_a_debug_build() {
        assert!(!parse_null_output_test_mode(None, true).unwrap());
        assert!(parse_null_output_test_mode(Some(OsStr::new("1")), true).unwrap());
        assert_eq!(
            parse_null_output_test_mode(Some(OsStr::new("1")), false).unwrap_err(),
            "NULL_OUTPUT_TEST_REQUIRES_DEVELOPMENT_BUILD"
        );
        assert_eq!(
            parse_null_output_test_mode(Some(OsStr::new("true")), true).unwrap_err(),
            "NULL_OUTPUT_TEST_VALUE_INVALID"
        );
    }

    #[test]
    fn null_test_starts_private_supervisor_on_a_fresh_first_run_profile() {
        let config = AppConfig::default();
        assert!(!should_start_mpd_for_null_test(false, &config, false));
        assert!(should_start_mpd_for_null_test(false, &config, true));
    }
}

fn is_shared_output(config: &AppConfig) -> bool {
    null_output_test_mode()
        || crate::audio_route::interpret_legacy_route(&config.audio_output_type, &config.alsa_device)
            .route.access
            == crate::audio_route::AudioAccess::Shared
}

fn is_shared_to_direct_transition(current: &AppConfig, proposed: &AppConfig) -> bool {
    !current.first_run
        && is_shared_output(current)
        && proposed.audio_output_type == "alsa"
        && !is_shared_output(proposed)
}

fn configured_audio_route(
    config: &AppConfig,
    processing: crate::audio_route::AudioProcessing,
) -> Result<crate::audio_route::AudioRoute, String> {
    let mut route = crate::audio_route::interpret_legacy_route(
        &config.audio_output_type,
        &config.alsa_device,
    ).route;
    route.processing = processing;
    route.validate().map_err(|_| "A rota de áudio solicitada é inválida.".to_string())?;
    Ok(route)
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
    let direct_endpoint_changed = proposed.audio_output_type != current.audio_output_type
        || proposed.alsa_device != current.alsa_device;
    let proposed_direct = proposed.audio_output_type == "alsa" && !is_shared_output(proposed);
    if proposed_direct && direct_endpoint_changed
        && !alsa_preflight::is_physical_address(&proposed.alsa_device)
    {
        return Err("ALSA_DIRECT_ENDPOINT_INVALID".into());
    }
    // A preflight PCM open while Shared is active reports the current
    // PipeWire owner as a conflict. Defer availability to the owned
    // supervisor after it has released its route; identity syntax remains
    // checked above and the post-release path observes the real PCM state.
    if proposed_direct && direct_endpoint_changed
        && !is_shared_to_direct_transition(current, proposed)
        && alsa_preflight::should_probe(&proposed.alsa_device, current_direct)
    {
        probe(&proposed.alsa_device).map_err(|error| error.code().to_string())?;
    }
    Ok(())
}

fn alsa_transition_error(stage: &str, reason: alsa_observation::Reason) -> String {
    let (code, message) = match reason {
        alsa_observation::Reason::Occupied => ("ALSA_DIRECT_DEVICE_BUSY", "o dispositivo continua ocupado por outro cliente"),
        alsa_observation::Reason::Changed => ("ALSA_DIRECT_DEVICE_CHANGED", "a identidade do dispositivo mudou durante a transição"),
        alsa_observation::Reason::Unavailable => ("ALSA_DIRECT_DEVICE_UNAVAILABLE", "o dispositivo selecionado não está disponível"),
        alsa_observation::Reason::NotOwned => ("ALSA_DIRECT_ACQUISITION_UNCONFIRMED", "o MPD não adquiriu o PCM dentro do prazo"),
        alsa_observation::Reason::Busy => ("ALSA_DIRECT_ACQUISITION_UNCONFIRMED", "a observação do PCM permaneceu ocupada até o prazo"),
        _ => ("ALSA_DIRECT_STATE_UNCONFIRMED", "não foi possível comprovar com segurança o estado do dispositivo"),
    };
    format!("{code}: {message} (etapa {stage}).")
}

fn start_direct_after_shared_release<T>(
    stop_owned_route: impl FnOnce() -> Result<(), String>,
    wait_for_physical_release: impl FnOnce() -> Result<(), String>,
    start_direct: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    stop_owned_route()?;
    wait_for_physical_release()?;
    start_direct()
}

fn confirm_playing_direct_route_before_commit(
    required: bool,
    resume_and_confirm_mpd: impl FnOnce() -> Result<(), String>,
    verify_owned_pcm: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    if !required { return Ok(false); }
    // MPD is the sole PCM acquisition owner. A successful protocol state alone
    // is insufficient; the read-only procfs observation must attribute the
    // selected PCM stream to that exact child before config is persisted.
    resume_and_confirm_mpd()?;
    verify_owned_pcm()?;
    Ok(true)
}

fn verify_direct_pcm_owned(
    supervisor: &Mutex<MpdSupervisor>,
    config: &AppConfig,
    identity: &alsa_observation::DeviceIdentity,
) -> Result<(), String> {
    let expected_route = configured_audio_route(config, crate::audio_route::AudioProcessing::Off)?;
    let (pid, generation) = {
        let supervisor = supervisor.lock().map_err(|_| "ALSA_DIRECT_SUPERVISOR_UNAVAILABLE")?;
        let route = supervisor.audio_route_observation();
        if route.null_output_test || route.eq_pipe_configured
            || route.operation != crate::audio_route::RouteOperationState::Stable
            || route.applied.as_ref() != Some(&expected_route)
        {
            return Err("ALSA_DIRECT_ROUTE_NOT_APPLIED".into());
        }
        (supervisor.observed_process_pid().ok_or("ALSA_DIRECT_SUPERVISOR_UNAVAILABLE")?, route.generation)
    };
    if identity.endpoint() != config.alsa_device {
        return Err("ALSA_DIRECT_DEVICE_CHANGED: identidade do endpoint divergiu da configuração solicitada.".into());
    }
    alsa_observation::wait_for_owned_pcm(identity, pid, Duration::from_secs(1))
        .map_err(|reason| alsa_transition_error("mpd_pcm_acquisition", reason))?;
    let supervisor = supervisor.lock().map_err(|_| "ALSA_DIRECT_SUPERVISOR_UNAVAILABLE")?;
    let route = supervisor.audio_route_observation();
    if supervisor.observed_process_pid() != Some(pid)
        || route.generation != generation
        || route.operation != crate::audio_route::RouteOperationState::Stable
        || route.applied.as_ref() != Some(&expected_route)
        || route.null_output_test || route.eq_pipe_configured
    {
        return Err("ALSA_DIRECT_DEVICE_CHANGED: a instância ou a rota mudou durante a confirmação.".into());
    }
    Ok(())
}

struct ProvisionalDirectChoice {
    gate: Arc<direct_playback::Gate>,
    previous: AppConfig,
    finalized: bool,
}

impl ProvisionalDirectChoice {
    fn stage(gate: Arc<direct_playback::Gate>, previous: &AppConfig, next: &AppConfig) -> Self {
        gate.commit_choice(next);
        Self { gate, previous: previous.clone(), finalized: false }
    }
    fn finalize(&mut self) { self.finalized = true; }
}

impl Drop for ProvisionalDirectChoice {
    fn drop(&mut self) {
        if !self.finalized { self.gate.commit_choice(&self.previous); }
    }
}

#[cfg(test)]
mod direct_route_release_tests {
    use super::*;

    fn shared_config() -> AppConfig {
        AppConfig { first_run: false, audio_output_type: "pipewire".into(), alsa_device: "hw:2,0".into(), ..AppConfig::default() }
    }

    fn direct_config() -> AppConfig {
        AppConfig { audio_output_type: "alsa".into(), alsa_device: "hw:2,0".into(), ..shared_config() }
    }

    #[test]
    fn legacy_shared_to_direct_skips_the_pre_release_pcm_probe_but_keeps_endpoint_validation() {
        let current = shared_config();
        let proposed = direct_config();
        let mut called = false;
        validate_config_change_preflight(&current, &current, &proposed, |_| {
            called = true;
            Err(alsa_preflight::PreflightError::Busy)
        }).unwrap();
        assert!(!called, "opening a second PCM before releasing Shared would only report the current route");

        let invalid = AppConfig { alsa_device: "plughw:2,0".into(), ..proposed.clone() };
        assert_eq!(validate_config_change_preflight(&current, &current, &invalid, |_| panic!("invalid endpoint must fail before probing")), Err("ALSA_DIRECT_ENDPOINT_INVALID".into()));

        let mut stale = current.clone();
        stale.alsa_device = "hw:3,0".into();
        assert!(validate_config_change_preflight(&stale, &current, &proposed, |_| panic!("stale config must fail before probing")).is_err());

        let first_run = AppConfig { first_run: true, ..current.clone() };
        let mut called = false;
        validate_config_change_preflight(&first_run, &first_run, &direct_config(), |_| {
            called = true;
            Err(alsa_preflight::PreflightError::Busy)
        }).unwrap_err();
        assert!(called, "first-run has no existing Sonante Shared route to release");
    }

    #[test]
    fn direct_start_is_ordered_after_owned_route_stop_and_observed_release() {
        let steps = std::cell::RefCell::new(Vec::new());
        let result = start_direct_after_shared_release(
            || { steps.borrow_mut().push("stop-owned"); Ok(()) },
            || { steps.borrow_mut().push("wait-released"); Ok(()) },
            || { steps.borrow_mut().push("start-direct"); Ok(17) },
        ).unwrap();
        assert_eq!(result, 17);
        assert_eq!(*steps.borrow(), ["stop-owned", "wait-released", "start-direct"]);

        let steps = std::cell::RefCell::new(Vec::new());
        let result = start_direct_after_shared_release::<()>(
            || { steps.borrow_mut().push("stop-owned"); Ok(()) },
            || { steps.borrow_mut().push("wait-released"); Err("another client still owns the PCM".into()) },
            || { steps.borrow_mut().push("start-direct"); Ok(()) },
        );
        assert_eq!(result, Err("another client still owns the PCM".into()));
        assert_eq!(*steps.borrow(), ["stop-owned", "wait-released"], "a busy device must not start Direct or fall back");
    }

    #[test]
    fn failed_owned_stop_never_waits_or_starts_a_second_supervisor() {
        let steps = std::cell::RefCell::new(Vec::new());
        let result = start_direct_after_shared_release::<()>(
            || { steps.borrow_mut().push("stop-owned"); Err("owned MPD stop failed".into()) },
            || { steps.borrow_mut().push("wait-released"); Ok(()) },
            || { steps.borrow_mut().push("start-direct"); Ok(()) },
        );
        assert_eq!(result, Err("owned MPD stop failed".into()));
        assert_eq!(*steps.borrow(), ["stop-owned"]);
    }

    #[test]
    fn direct_playback_is_confirmed_only_after_mpd_and_pcm_ownership_both_succeed() {
        let steps = std::cell::RefCell::new(Vec::new());
        assert!(confirm_playing_direct_route_before_commit(true,
            || { steps.borrow_mut().push("mpd-playing"); Ok(()) },
            || { steps.borrow_mut().push("owned-pcm"); Ok(()) },
        ).unwrap());
        assert_eq!(*steps.borrow(), ["mpd-playing", "owned-pcm"]);

        let steps = std::cell::RefCell::new(Vec::new());
        assert_eq!(confirm_playing_direct_route_before_commit(false,
            || { steps.borrow_mut().push("unexpected-resume"); Ok(()) },
            || { steps.borrow_mut().push("unexpected-pcm-check"); Ok(()) },
        ), Ok(false));
        assert!(steps.borrow().is_empty(), "paused and stopped route saves must not auto-resume to prove acquisition");

        let steps = std::cell::RefCell::new(Vec::new());
        assert_eq!(confirm_playing_direct_route_before_commit(true,
            || { steps.borrow_mut().push("mpd-playing"); Ok(()) },
            || { steps.borrow_mut().push("owned-pcm"); Err("not owned".into()) },
        ), Err("not owned".into()));
        assert_eq!(*steps.borrow(), ["mpd-playing", "owned-pcm"]);
    }
}

fn should_rescan_library_on_startup(
    config_valid: bool,
    config: &AppConfig,
    database_exists: bool,
) -> bool {
    should_start_mpd_on_startup(config_valid, config)
        && config.sources.unwrap_or_default().local
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
    expected.direct_volume_choice = next.direct_volume_choice.clone(); // Explicit route policy metadata does not change folders/output.
    expected == *next
}

fn validate_local_folder_admission(
    current: &AppConfig,
    proposed: &AppConfig,
) -> Result<(), String> {
    if !current.first_run && !local_folders_changed(current, proposed) {
        return Ok(()); // Existing overlaps cannot block unrelated preferences.
    }
    let previous = if current.first_run {
        Vec::new()
    } else {
        current.local_folders.iter().map(PathBuf::from).collect()
    };
    let next = proposed
        .local_folders
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    local_media::mirror::validate_new_source_overlaps(&previous, &next)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_reconciled: Option<bool>,
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
        catalog_reconciled: None,
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

#[cfg(test)]
fn request_rescan_and_wait(socket_path: &str) -> Result<(), String> {
    request_rescan_and_wait_traced(socket_path, None)
}

#[cfg(test)]
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

#[cfg(test)]
fn request_rescan_wait_and_audit(socket_path: &str) -> Result<(), String> {
    request_rescan_wait_and_audit_traced(socket_path, None)
}

#[cfg(test)]
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

#[cfg(test)]
fn request_startup_refresh_and_audit(
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
) -> Result<(), String> {
    request_startup_refresh_and_audit_traced(socket_path, refresh_plan, None)
}

#[cfg(test)]
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

#[cfg(test)]
fn request_refresh_and_audit_traced(
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
    diagnostics: Option<&SettingsSaveDiagnostics>,
    operation: Option<&config_save_operation::Operation>,
) -> Result<(), String> {
    request_refresh_and_inspect(socket_path, refresh_plan, diagnostics, operation, || {
        let audit = AudioEngine::audit_mpd_local_catalog_at(socket_path)?;
        if !audit.is_clean() { return Err(format!("A auditoria do catálogo encontrou {} URI(s) local(is) inválida(s).", audit.invalid_count)); }
        Ok(())
    })
}

fn request_refresh_and_inspect(
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
    diagnostics: Option<&SettingsSaveDiagnostics>,
    operation: Option<&config_save_operation::Operation>,
    inspect: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut refresh_duration = Duration::ZERO;
    let mut wait_duration = Duration::ZERO;
    let mut audit_duration = Duration::ZERO;
    let (strategy, directory_count) = match refresh_plan {
        local_media::mirror::ManagedRefreshPlan::MetadataOnly => ("metadata", 0),
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
            local_media::mirror::ManagedRefreshPlan::MetadataOnly => return Ok(()),
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
        let audit = inspect();
        audit_duration += started.elapsed();
        if let Some(diagnostics) = diagnostics {
            diagnostics.stage("audit", started, if audit.is_ok() { "ok" } else { "error" });
            if audit.is_ok() { diagnostics.mpd_last_confirmed.set("available_at_audit"); }
        }
        audit?;
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
        (_, local_media::mirror::ManagedRefreshPlan::MetadataOnly) => Ok(()),
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
    CancelledDuringRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedMirrorRefreshMode {
    Manual,
    Check,
    Startup,
}

fn finish_unchanged_managed_generation(
    mode: ManagedMirrorRefreshMode,
    socket_path: &str,
) -> Result<bool, String> {
    match mode {
        ManagedMirrorRefreshMode::Manual => {
            library_refresh::finish(
                library_refresh::refresh(socket_path, &local_media::mirror::ManagedRefreshPlan::Full, true, || false),
                |result| result,
            )?;
            Ok(true)
        }
        ManagedMirrorRefreshMode::Check
        | ManagedMirrorRefreshMode::Startup
        => Ok(false),
    }
}

#[cfg(test)]
fn refresh_published_managed_generation(
    mode: ManagedMirrorRefreshMode,
    socket_path: &str,
    refresh_plan: &local_media::mirror::ManagedRefreshPlan,
) -> Result<(), String> {
    match mode {
        ManagedMirrorRefreshMode::Manual => request_rescan_wait_and_audit(socket_path),
        ManagedMirrorRefreshMode::Check
        | ManagedMirrorRefreshMode::Startup
        => {
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
    if matches!(restore_outcome, RestorePreviousOutcome::CancelledDuringRecovery) {
        return Err(format!("{primary_error}; {}: recuperação cancelada no encerramento; gerações preservadas e catálogo/reprodução não confirmados.", library_refresh::CANCELLED));
    }
    if let Err(cleanup_error) = cleanup_rejected() {
        return match &restore_outcome {
            RestorePreviousOutcome::PlaybackIncomplete(error) => Err(format!(
                "{primary_error}; mirror revertido, MPD reiniciado e catálogo anterior atualizado (auditado se managed); reprodução não restaurada ({error}); a geração rejeitada não pôde ser limpa ({cleanup_error})."
            )),
            RestorePreviousOutcome::Restored => Err(format!(
                "{primary_error}; mirror revertido, MPD reiniciado e catálogo anterior atualizado (auditado se managed), reprodução restaurada; a geração rejeitada não pôde ser limpa ({cleanup_error})."
            )),
            RestorePreviousOutcome::CancelledDuringRecovery => unreachable!("handled before cleanup"),
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
        RestorePreviousOutcome::CancelledDuringRecovery => unreachable!("handled before cleanup"),
        RestorePreviousOutcome::SkippedForShutdown => Err(format!(
            "{primary_error}; mirror revertido e geração rejeitada limpa, mas MPD, catálogo e reprodução não foram recuperados porque a aplicação está sendo encerrada."
        )),
    }
}

fn route_save_resume_requested(current:&AppConfig,next:&AppConfig)->bool {
    !current.first_run && !next.first_run
        && (current.alsa_device!=next.alsa_device || is_shared_output(current)!=is_shared_output(next))
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
    let refresh = library_refresh::refresh(
        socket_path, &local_media::mirror::ManagedRefreshPlan::Full,
        previous_kind == local_media::mirror::ManagedMirrorPreviousKind::Managed,
        || transaction_state.shutdown_requested(),
    );
    if matches!(refresh, Ok(library_refresh::Outcome::Cancelled)) {
        return Ok(RestorePreviousOutcome::CancelledDuringRecovery);
    }
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
    if transaction_state.shutdown_requested() {
        return Ok(RestorePreviousOutcome::CancelledDuringRecovery);
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

// This policy never turns a manual check into permission to reread every
// unchanged track. The existing filesystem planner still proves path scope.
fn explicit_incremental_check_plan(
    mut plan: local_media::mirror::ManagedRefreshPlan,
    sources: &std::collections::BTreeMap<String, PathBuf>,
) -> Result<local_media::mirror::ManagedRefreshPlan, String> {
    match &mut plan {
        local_media::mirror::ManagedRefreshPlan::Targeted { directories, .. } if directories.is_empty() => {
            // Same virtual paths may have changed tags/mtime while closed.
            directories.extend(sources.keys().cloned());
        }
        local_media::mirror::ManagedRefreshPlan::Full => return Err("LOCAL_INCREMENTAL_CHECK_UNCONFIRMED".into()),
        _ => {}
    }
    Ok(plan)
}

fn scoped_incremental_check_plan(
    plan: local_media::mirror::ManagedRefreshPlan,
    sources: &std::collections::BTreeMap<String, PathBuf>,
    source_id: Option<&str>,
) -> Result<local_media::mirror::ManagedRefreshPlan, String> {
    let Some(id) = source_id else { return explicit_incremental_check_plan(plan, sources); };
    let root = sources.get(id).ok_or("LOCAL_INCREMENTAL_CHECK_SCOPE_CHANGED")?;
    let selected = [(id.to_owned(), root.clone())].into();
    let plan = explicit_incremental_check_plan(plan, &selected)?;
    if let local_media::mirror::ManagedRefreshPlan::Targeted { directories, .. } = &plan {
        if directories.iter().any(|path| Path::new(path).components().next()
            .and_then(|part| part.as_os_str().to_str()) != Some(id)) {
            return Err("LOCAL_INCREMENTAL_CHECK_SCOPE_CHANGED".into());
        }
    }
    Ok(plan)
}

fn run_managed_manual_rescan(app: &AppHandle) -> Result<(), String> {
    run_managed_mirror_refresh(app, ManagedMirrorRefreshMode::Manual, None, None).map(|_| ())
}

fn run_managed_mirror_refresh(
    app: &AppHandle,
    mode: ManagedMirrorRefreshMode,
    source_id: Option<&str>,
    expected_folders: Option<&[String]>,
) -> Result<bool, String> {
    let transaction_state = app.state::<ConfigTransactionState>();
    let config_read_guard = transaction_state.lock_startup_config_reads(mode)?;
    let transaction_guard = transaction_state.begin()?;
    // Revoke startup/check read ownership BEFORE releasing transaction admission.
    let mut config_read_guard = config_read_guard;
    let prepared_watch = if mode == ManagedMirrorRefreshMode::Manual {
        let roots = app.state::<ConfigState>().0.lock().map_err(|_| "LOCAL_WATCH_CONFIG_UNAVAILABLE")?.local_folders.iter().map(PathBuf::from).collect();
        Some(app.state::<LocalWatcherState>().prepare(roots)?)
    } else { None };
    let result = run_managed_mirror_refresh_admitted(app, mode, None, source_id, expected_folders, &mut config_read_guard, Some(&transaction_guard));
    if mode != ManagedMirrorRefreshMode::Check && matches!(result, Ok(true)) {
        install_committed_local_watcher(app, prepared_watch);
    }
    result
}

fn run_managed_mirror_refresh_admitted(
    app: &AppHandle,
    mode: ManagedMirrorRefreshMode,
    dirty_plan: Option<&local_media::mirror::ManagedRefreshPlan>,
    source_id: Option<&str>,
    expected_folders: Option<&[String]>,
    config_read_guard: &mut Option<StartupConfigReadGuard<'_>>,
    transaction_guard: Option<&config_save_operation::Transaction<'_>>,
) -> Result<bool, String> {
    let transaction_state = app.state::<ConfigTransactionState>();
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    let config = app
        .state::<ConfigState>()
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar pastas locais: {error}"))?
        .clone();
    if let Some(expected) = expected_folders {
        if config.local_folders != expected {
            return Err("LOCAL_INCREMENTAL_CHECK_SCOPE_CHANGED".into());
        }
    }
    if let Some(guard) = config_read_guard.as_mut() {
        guard.allow_reads(&MpdSupervisor::sonante_config_dir())?;
    }
    let socket_path = app
        .state::<AudioState>()
        .0
        .lock()
        .map_err(|error| format!("Falha ao acessar a biblioteca local: {error}"))?
        .local_album_socket_path();
    // Admit Direct commands before the catalog snapshot I/O too. During this
    // initial phase album reads keep their ordinary confirmed-database path.
    let check_session = if mode == ManagedMirrorRefreshMode::Check {
        Some(transaction_state.manual_check_playback.begin_preparing(
            transaction_guard.ok_or(manual_check_playback::BLOCKED)?, transaction_state.operations.as_ref())?)
    } else { None };
    // Capture the same pre-update catalog used by coverage, before preparation.
    // Only this real Check owner can expose it while MPD updates in the background.
    let previous_catalog = if mode == ManagedMirrorRefreshMode::Check {
        match AudioEngine::wait_for_managed_mpd_update(&socket_path, None, MANUAL_RESCAN_POLL_INTERVAL,
            || transaction_state.shutdown_requested()) {
            Ok(MpdRefreshCompletion::Completed) => {},
            _ => return Err("LOCAL_INDEX_PREVIOUS_UNAVAILABLE".into()),
        }
        Some(AudioEngine::read_validated_local_catalog_at(&socket_path)?)
    } else { None };
    if let (Some(session), Some(records)) = (&check_session, &previous_catalog) { session.bind_catalog(records)?; }
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
        },
        std::process::id(),
        MIRROR_GENERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let preparation = if let Some(id) = source_id {
        if mode != ManagedMirrorRefreshMode::Check { return Err("LOCAL_INCREMENTAL_CHECK_SCOPE_CHANGED".into()); }
        local_media::mirror::prepare_managed_generation_for_source_check(&live_path, &staging_path, &roots, &generation_id, id)?
    } else {
        local_media::mirror::prepare_managed_generation_for_refresh(&live_path, &staging_path, &roots,
            &generation_id, mode != ManagedMirrorRefreshMode::Startup || dirty_plan.is_some())?
    };
    let local_media::mirror::ManagedGenerationPreparation::Prepared(prepared) = preparation else {
        return match dirty_plan {
            Some(_) => Err(index_coverage::INVALID.into()),
            None => finish_unchanged_managed_generation(mode, &socket_path),
        };
    };
    if transaction_state.shutdown_requested() {
        return Err(error_with_rollback(
            "A reconciliação foi cancelada antes da publicação porque a aplicação está sendo encerrada."
                .to_string(),
            local_media::mirror::cleanup_managed_generation(&prepared.staging_path),
        ));
    }
    let structural_plan = if dirty_plan.is_some() || mode != ManagedMirrorRefreshMode::Startup {
        match local_media::mirror::plan_filesystem_refresh(&live_path, &prepared) {
            Ok(plan) => plan,
            Err(error) => return Err(error_with_rollback(error, local_media::mirror::cleanup_prepared_managed_generation(&prepared))),
        }
    } else { prepared.refresh_plan.clone() };
    let combined_plan = dirty_plan.map_or_else(|| structural_plan.clone(), |dirty| local_changes::merge(&structural_plan, dirty));
    let combined_plan = if mode != ManagedMirrorRefreshMode::Startup {
        scoped_incremental_check_plan(combined_plan, &prepared.manifest.sources, source_id)
            .map_err(|error|error_with_rollback(error,
                local_media::mirror::cleanup_prepared_managed_generation(&prepared)))?
    } else { combined_plan };
    if mode != ManagedMirrorRefreshMode::Manual
        && combined_plan != local_media::mirror::ManagedRefreshPlan::MetadataOnly
    {
        match AudioEngine::wait_for_managed_mpd_update(
            &socket_path, None, MANUAL_RESCAN_POLL_INTERVAL,
            || transaction_state.shutdown_requested(),
        ) {
            Ok(MpdRefreshCompletion::Completed) => {}
            Err(()) => {
                return Err(error_with_rollback(library_refresh::CANCELLED.into(),
                    local_media::mirror::cleanup_prepared_managed_generation(&prepared)));
            }
            Ok(completion) => {
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
    let coverage_token = app.state::<index_coverage::State>().begin()?;
    let coverage_check = if combined_plan == local_media::mirror::ManagedRefreshPlan::MetadataOnly { None } else {
        let previous_ids = match local_media::mirror::read_manifest(&live_path) {
            Ok(manifest) => manifest.sources.keys().cloned().collect(),
            Err(_) => local_media::resolve_local_sources(&live_path, &config.local_folders).map_err(|e|e.to_string())?.sources().keys().cloned().collect(),
        };
        Some(prepare_index_check(&socket_path, prepared.coverage.clone(), previous_ids, ||transaction_state.shutdown_requested(), previous_catalog)
            .map_err(|error|error_with_rollback(error,local_media::mirror::cleanup_prepared_managed_generation(&prepared)))?)
    };
    let coverage_report = std::cell::RefCell::new(None);
    let refresh_plan = prepared.refresh_plan.clone();
    let publication_result = local_media::mirror::publish_prepared_managed_generation_fenced(
        &live_path, &prepared, || {
            let guard = check_session.as_ref().map(|session| session.publication()).transpose()?;
            if transaction_state.shutdown_requested() { return Err(library_refresh::CANCELLED.into()); }
            Ok(guard)
        });
    let publication = match publication_result {
        Ok(publication) => publication,
        Err(error) if error.starts_with(local_media::mirror::PUBLICATION_UNCONFIRMED) => return Err(error),
        Err(error) => {
            if let Some(session) = &check_session { session.confirm(); }
            return Err(error_with_rollback(
                format!("Falha ao publicar o mirror preparado: {error}"),
                local_media::mirror::cleanup_managed_generation(&prepared.staging_path),
            ))
        }
    };
    if transaction_state.shutdown_requested() {
        if let Some(session) = &check_session { session.revoke(); }
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
    // Both explicit actions retain the incremental plan admitted above.
    let effective_plan = &combined_plan;
    let result = library_refresh::refresh_inspected(&socket_path, effective_plan, true, || transaction_state.shutdown_requested(), dirty_plan.is_some(), || {
        let (report, _) = coverage_check.as_ref().ok_or(index_coverage::INVALID)?.finish_at(&socket_path, &live_path)?;
        *coverage_report.borrow_mut() = Some(report); Ok(())
    });
    if result != Ok(library_refresh::Outcome::Completed) {
        if let Some(session) = &check_session {
            session.revoke();
            app.state::<AudioState>().0.lock().map_err(|_| manual_check_playback::BLOCKED)?.invalidate_playback_intents();
        }
    }
    let completion = library_refresh::finish(result, |refresh_result| complete_published_mirror_refresh(
        || refresh_result.clone(),
        || AudioEngine::capture_mpd_playback_snapshot(&socket_path),
        || stop_supervisor(&supervisor_state.0),
        || publication.rollback(),
        |snapshot| {
            let outcome = match previous_kind {
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
            }?;
            covered_index_recovery(outcome, coverage_check.as_ref(), &socket_path)
        },
        || {
            publication.cleanup_rejected_after_rollback()?;
            if let Some(session) = &check_session { session.confirm(); }
            Ok(())
        },
        || Ok(()),
        || if refresh_plan == local_media::mirror::ManagedRefreshPlan::MetadataOnly {
            publication.cleanup_previous_durable()
        } else { publication.cleanup_previous() },
    ));
    if completion.is_ok() {
        if let Some(session) = &check_session { session.confirm(); }
        if let Some(report) = coverage_report.into_inner() { publish_index_coverage(app, coverage_token, &config.local_folders, report); } else { inherit_index_coverage(app,coverage_token,&config.local_folders); }
        // Keep Check read/control ownership through final watcher bookkeeping,
        // while the same transaction still excludes competing source writers.
        if mode == ManagedMirrorRefreshMode::Check { install_committed_local_watcher(app, None); }
    }
    completion.map(|_| true)
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
    app.state::<Arc<direct_playback::Gate>>().commit_choice(new_config);
    plex_guard.update_config(new_config);
    *config_guard = new_config.clone();
    drop(plex_guard);
    drop(config_guard);
    remote_bridge::beta_preference_changed(app, expected_current.experimental_features, new_config.experimental_features);
    if expected_current.full_mode() != new_config.full_mode() && !null_output_test_mode() {
        remote_bridge::usage_mode_changed(app);
        upnp_bridge::usage_mode_changed(app);
    }
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

    #[cfg(test)]
    fn validate(&self, socket_path: &str) -> Result<(), String> {
        if AudioEngine::read_validated_local_catalog_at(socket_path)? != self.0 {
            return Err("LOCAL_SOURCE_REMOVAL_CATALOG_MISMATCH".to_string());
        }
        Ok(())
    }
}

struct RemovalRefreshExpectation {
    before_catalog: std::collections::HashSet<String>,
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
    queue_references_removed_ids(&plan.removed_ids, logical, queue)
}

fn queue_references_removed_ids(removed_ids: &[String], logical: &[TrackMetadata], queue: &audio::MpdQueueStructure) -> bool {
    let affected = |uri: &str| {
        uri.split_once('/')
            .is_some_and(|(id, _)| removed_ids.iter().any(|removed| removed == id))
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
    queue_clear_binding_ids(previous, next, &plan.removed_ids, &plan.remaining_ids, backup)
}

fn queue_clear_binding_ids(previous: &AppConfig, next: &AppConfig, removed: &[String], remaining: &[String], backup: &managed_save_intent::QueueBackup) -> Result<String, String> {
    // Playback may advance naturally during confirmation. Bind structural state,
    // occurrences/IDs and exact configs/sources, not wall-clock elapsed/cursor.
    let bytes = serde_json::to_vec(&(
        previous,
        next,
        removed,
        remaining,
        &backup.tracks,
        &backup.mpd,
        backup.playlist_version,
    ))
    .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

// Ask before recursive watcher registration, physical enumeration and staging.
// This reserves/captures only the current queue. Consent never authorizes a
// clear/publication: the original late admission rechecks catalog, queue and
// exact binding after preparation, including any natural track advancement.
fn audio_socket_for_removal_consent(audio: &Mutex<AudioEngine>) -> Result<String, String> {
    Ok(audio.lock().map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE")?.local_album_socket_path())
}

fn early_removal_queue_consent(audio: &Mutex<AudioEngine>, socket: &str, previous: &AppConfig, next: &AppConfig,
    sources: &local_media::mirror::RemovalConsentSources) -> Result<(), String> {
    let (_owner, tracks) = {
        let mut audio = audio.lock().map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE")?;
        // Preserve the established logical-resume policy; no new confirmation
        // or cursor/MPD effect is introduced for an already stopped restore.
        if audio.queue_has_logical_resume() { return Ok(()); }
        let owner = audio.reserve_queue_structure().ok_or("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_UNAVAILABLE")?;
        (owner, audio.get_queue())
    };
    early_removal_queue_consent_with(previous, next, sources, &tracks,
        || AudioEngine::capture_mpd_queue_structure(socket).map_err(|e| e.to_string()),
        || AudioEngine::capture_mpd_playback_snapshot(socket))
}

fn early_removal_queue_consent_with(previous: &AppConfig, next: &AppConfig,
    sources: &local_media::mirror::RemovalConsentSources, tracks: &[TrackMetadata],
    capture_queue: impl FnOnce() -> Result<audio::MpdQueueStructure, String>,
    capture_playback: impl FnOnce() -> Result<audio::MpdPlaybackSnapshot, String>) -> Result<(), String> {
    let queue = capture_queue()?;
    if !queue_references_removed_ids(&sources.removed_ids, tracks, &queue) { return Ok(()); }
    let playback = capture_playback()?;
    if playback.state == audio::PlaybackState::Stopped
        && (playback.current_index.is_some() || playback.elapsed != 0.0) {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_BACKUP_UNSUPPORTED".into());
    }
    let backup = managed_save_intent::QueueBackup::new(tracks, &playback.uris, &queue, &playback)?;
    let binding = queue_clear_binding_ids(previous, next, &sources.removed_ids, &sources.remaining_ids, &backup)?;
    Err(format!("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_REQUIRED:{binding}"))
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
    let Some(catalog) = RemovalCatalogExpectation::from_before(plan, before.clone()) else { return Ok(None) };
    if !removal_queue_matches_remaining_sources(plan, &logical, &queue, &catalog)
        || !removal_queue_before_publish(socket_path, &queue)?
    {
        return Ok(None);
    }
    Ok(Some((reservation, RemovalRefreshExpectation { before_catalog: before, catalog, queue })))
}

#[cfg(test)]
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

fn plan_saved_folders_refresh(
    live_path: &Path, prepared: &local_media::mirror::MirrorBuildResult,
    diagnostics: &SettingsSaveDiagnostics,
) -> Result<(local_media::mirror::ManagedRefreshPlan, Option<local_media::mirror::PureSourceRemoval>), String> {
    if prepared.refresh_plan == local_media::mirror::ManagedRefreshPlan::MetadataOnly {
        diagnostics.rejection.set("metadata_only");
        return Ok((local_media::mirror::ManagedRefreshPlan::MetadataOnly, None));
    }
    let mut removal_plan = None;
    let refresh_plan = match local_media::mirror::plan_added_sources_refresh_diagnostic(live_path, prepared) {
        Ok((Some(plan), reason)) => {
            diagnostics.rejection.set(reason);
            plan
        }
        Ok((None, reason)) => {
            diagnostics.rejection.set(reason);
            match local_media::mirror::plan_removed_sources_refresh(live_path, prepared) {
                Ok(Some(plan)) => {
                    let refresh = plan.refresh_plan.clone();
                    removal_plan = Some(plan);
                    diagnostics.rejection.set("none");
                    refresh
                }
                Ok(None) => local_media::mirror::ManagedRefreshPlan::Full,
                Err(error) => return Err(error_with_rollback(error,
                    local_media::mirror::cleanup_prepared_managed_generation(prepared))),
            }
        }
        Err(error) => {
            diagnostics.rejection.set("planning_error");
            return Err(error_with_rollback(
                format!("Falha ao planejar refresh das novas fontes: {error}"),
                local_media::mirror::cleanup_prepared_managed_generation(prepared),
            ))
        }
    };
    Ok((refresh_plan, removal_plan))
}

fn after_confirmed_local_folders_save<T>(result: Result<T, String>, uncertain: bool, install: impl FnOnce()) -> Result<T, String> {
    if result.is_ok() && !uncertain { install(); }
    result
}

fn prepare_initial_preferences(
    app: &AppHandle, observed: &AppConfig, next: &AppConfig,
    operation: &config_save_operation::Operation,
) -> Result<local_watcher::PreparedWatch, String> {
    let transactions = app.state::<ConfigTransactionState>();
    let owner = app.state::<profile_lease::State>().lease()?;
    owner.validate()?;
    let _startup = transactions.startup_config_read_gate.try_lock().map_err(|_| "SOURCE_AVAILABILITY_BUSY")?;
    let cancelled = || operation.shutdown_requested() || transactions.shutdown_requested();
    recovery_fs::check(&cancelled)?;
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    let current = app.state::<ConfigState>().0.lock().map_err(|_| "Config unavailable")?.clone();
    validate_config_change_preflight(observed, &current, next, alsa_preflight::probe)?;
    if !is_finishing_first_run(&current, next) { return Err("FIRST_RUN_CONFIG_CHANGED".into()); }
    let watch = local_diagnostics::wizard_step(local_diagnostics::Stage::WizardWatch, ||
        app.state::<LocalWatcherState>().prepare(next.local_folders.iter().map(PathBuf::from).collect()))?;
    let generation = format!("first-run-{}-{}", std::process::id(), MIRROR_GENERATION_SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let mut initial = local_diagnostics::wizard_step(local_diagnostics::Stage::WizardLibraryPrepare, ||
        first_run_managed::InitialPublication::prepare(&owner, &current, next, &generation, &cancelled))?;
    local_diagnostics::wizard_step(local_diagnostics::Stage::WizardLibraryPublish, || initial.publish(&cancelled))?;
    // Preserve the historical output admission: the owned MPD must accept its
    // configuration before preferences are confirmed. Its auto-index can run;
    // neither first DB completion nor a rescan idle is required for navigation.
    let supervisor = app.state::<SupervisorState>();
    let started = local_diagnostics::wizard_step(local_diagnostics::Stage::WizardEngine, ||
        supervisor.0.lock().map_err(|_| "MPD unavailable".to_string())?.start(next)
            .and_then(|_| initial.validate_published()));
    if let Err(error) = started {
        return Err(error_with_rollback(error, initial.rollback(|| stop_supervisor(&supervisor.0))));
    }
    let preferences_committed = Cell::new(false);
    let persistence_uncertain = Cell::new(false);
    let result = (|| {
        recovery_fs::check(&cancelled)?;
        let config_state = app.state::<ConfigState>();
        let mut config = config_state.0.lock().map_err(|_| "Config unavailable")?;
        let plex_state = app.state::<PlexState>();
        let mut plex = plex_state.0.lock().map_err(|_| "Plex config unavailable")?;
        if *config != current { return Err("FIRST_RUN_CONFIG_CHANGED".into()); }
        initial.validate_preferences_before_save()?;
        let save_started = Instant::now();
        let saved = next.save();
        let save_error = saved.as_ref().err().map(ToString::to_string);
        local_diagnostics::record(local_diagnostics::Stage::WizardPreferences, save_started.elapsed(),
            save_error.as_deref());
        match saved {
            Ok(()) => preferences_committed.set(true),
            Err(ConfigSaveError::BeforeRename(error)) => return Err(error),
            Err(error @ ConfigSaveError::AfterRenameUnconfirmed(_)) => {
                persistence_uncertain.set(true);
                return Err(uncertain_config_save_message(&error));
            }
        }
        local_diagnostics::wizard_step(local_diagnostics::Stage::WizardBackground, || initial.prepare_background())?;
        owner.validate()?;
        plex.update_config(next);
    music_sources::install(next);
        *config = next.clone();
        app.state::<Arc<direct_playback::Gate>>().commit_choice(next);
        Ok(())
    })();
    match result {
        Ok(()) => Ok(watch),
        Err(error) => {
            let cleanup = if !preferences_committed.get() && !persistence_uncertain.get() {
                initial.rollback(|| stop_supervisor(&supervisor.0))
            } else { stop_supervisor(&supervisor.0) };
            Err(error_with_rollback(error, cleanup))
        }
    }
}

fn confirm_sqlite_initial_preferences<T>(
    current: &AppConfig,
    next: &AppConfig,
    library: &SqliteLibraryState,
    publish: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if !is_finishing_first_run(current, next) {
        return Err("FIRST_RUN_CONFIG_CHANGED".into());
    }
    library.with_initial_sources_guard(
        &next.local_folders,
        next.sources.unwrap_or_default().local,
        publish,
    )
}

fn prepare_initial_sqlite_preferences(
    app: &AppHandle,
    observed: &AppConfig,
    next: &AppConfig,
    operation: &config_save_operation::Operation,
) -> (Result<(), String>, bool) {
    let committed = Cell::new(false);
    let uncertain_save = Cell::new(false);
    let started_here = Cell::new(false);
    let result = (|| {
        let transactions = app.state::<ConfigTransactionState>();
        let owner = app.state::<profile_lease::State>().lease()?;
        owner.validate()?;
        let _startup = transactions
            .startup_config_read_gate
            .try_lock()
            .map_err(|_| "SOURCE_AVAILABILITY_BUSY")?;
        let cancelled = || operation.shutdown_requested() || transactions.shutdown_requested();
        recovery_fs::check(&cancelled)?;
        managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
        let current = app
            .state::<ConfigState>()
            .0
            .lock()
            .map_err(|_| "Config unavailable")?
            .clone();
        validate_config_change_preflight(observed, &current, next, alsa_preflight::probe)?;
        let library = app.state::<SqliteLibraryState>();
        library.with_initial_sources_guard(
            &next.local_folders,
            next.sources.unwrap_or_default().local,
            || Ok(()),
        )?;
        // The null-output test already owns its private MPD on first run.
        // Normal first run admits the requested output before committing the
        // completed preferences, as the legacy Wizard does.
        let supervisor = app.state::<SupervisorState>();
        {
            let mut supervisor = supervisor.0.lock().map_err(|_| "MPD unavailable")?;
            if supervisor.observed_process_pid().is_none() {
                local_diagnostics::wizard_step(local_diagnostics::Stage::WizardEngine, || {
                    supervisor.start(next).map(|_| ())
                })?;
                started_here.set(true);
            }
        }
        confirm_sqlite_initial_preferences(&current, next, &library, || {
            owner.validate()?;
            recovery_fs::check(&cancelled)?;
            match persist_experimental_local_folders_config(app, &current, next) {
                Ok(()) => {
                    committed.set(true);
                    music_sources::install(next);
                    Ok(())
                }
                Err(error) => {
                    uncertain_save.set(matches!(error, ConfigSaveError::AfterRenameUnconfirmed(_)));
                    Err(config_save_error_message(&error))
                }
            }
        })
    })();
    // A confirmed write is terminal success unless a later generation-guard
    // validation fails. Only an unconfirmed rename, or a post-write failure,
    // leaves the caller unable to know whether preferences and catalog agree.
    let uncertain = uncertain_save.get() || (committed.get() && result.is_err());
    let result = if result.is_err() && started_here.get() && !uncertain {
        let supervisor = app.state::<SupervisorState>();
        result.map_err(|error| error_with_rollback(error, stop_supervisor(&supervisor.0)))
    } else {
        result
    };
    (result, uncertain)
}

fn run_initial_catalog(app: &AppHandle, operation: &config_save_operation::Operation, start_mpd: bool, prepared_watch: Option<local_watcher::PreparedWatch>) -> Result<(), String> {
    let transactions = app.state::<ConfigTransactionState>();
    let owner = app.state::<profile_lease::State>().lease()?;
    let pending = first_run_managed::PendingPreparation::open(&owner)?;
    let config = first_run_managed::PendingPreparation::config(&MpdSupervisor::sonante_config_dir())?.ok_or("FIRST_RUN_PREPARATION_INCOMPLETE")?;
    let cancelled = || operation.shutdown_requested() || transactions.shutdown_requested();
    recovery_fs::check(&cancelled)?;
    let snapshot = pending.snapshot()?;
    let mut inventory = snapshot.coverage_inventory()?;
    pending.restore_policy_counts(&mut inventory)?;
    let check = index_coverage::Check::from_catalog(inventory,
        index_coverage::Catalog::from_records(std::collections::HashSet::new())?, std::collections::BTreeSet::new())?;
    let watch = match prepared_watch {
        Some(watch) => watch,
        None => app.state::<LocalWatcherState>().prepare(config.local_folders.iter().map(PathBuf::from).collect())?,
    };
    let coverage_token = app.state::<index_coverage::State>().begin()?;
    let supervisor = app.state::<SupervisorState>();
    let result: Result<(), String> = (|| {
        if start_mpd { supervisor.0.lock().map_err(|_| "MPD unavailable")?.start(&config)?; }
        owner.validate()?;
        pending.snapshot()?;
        let socket = app.state::<AudioState>().0.lock().map_err(|_| "Playback unavailable")?.local_album_socket_path();
        if is_shared_output(&config) {
            let pid = supervisor.0.lock().map_err(|_| "MPD unavailable")?.observed_process_pid().ok_or("MPD unavailable")?;
            AudioEngine::send_owned_command_to_socket(&socket, pid, "status")?;
            let id = transactions.operations.initial_catalog_id()?.ok_or(initial_plex_playback::WAIT)?;
            if transactions.initial_plex.publish(id, socket.clone(), pid).is_err() {
                // Optional playback concurrency cannot invalidate saved preferences
                // or the catalog. Unsupported pidfd keeps the actionable wait guard.
                eprintln!("[Audio] Initial Plex playback permission unavailable; waiting for catalog completion.");
            }
        }
        if !snapshot.manifest.sources.is_empty() {
            let sources = snapshot.manifest.sources.keys().cloned().collect::<Vec<_>>();
            operation.enable_initial_preview(&socket, &sources).map_err(|_| "FIRST_RUN_PREVIEW_UNAVAILABLE")?;
            operation.enable_preview_artwork(local_import_artwork::ArtworkScope::new(
                &MpdSupervisor::library_dir(), &snapshot.manifest, &sources));
        }
        request_refresh_and_inspect(&socket, &local_media::mirror::ManagedRefreshPlan::Full, None, Some(operation), || {
            let (report, _) = check.finish_at(&socket, &MpdSupervisor::library_dir())?;
            snapshot.revalidate().map_err(|_| "FIRST_RUN_GENERATION_CHANGED")?;
            recovery_fs::check(&cancelled)?;
            pending.finish(&owner)?;
            publish_index_coverage(app, coverage_token, &config.local_folders, report);
            Ok(())
        })?;
        install_committed_local_watcher(app, Some(watch));
        Ok(())
    })();
    result
}

fn spawn_initial_catalog(
    app: AppHandle,
    operation: config_save_operation::Operation,
    start_mpd: bool,
    watch: Option<local_watcher::PreparedWatch>,
    resume_initial: bool,
) {
    operation.initial_catalog_background();
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_initial_catalog(&app, &operation, start_mpd, watch).map_err(|error| {
            // Includes failures before index admission. Preferences stay saved;
            // stop only the owned child and preserve the generation for reopen.
            let transactions = app.state::<ConfigTransactionState>();
            let cleanup = transactions.initial_plex.revoke_and_stop(|| {
                let audio_state = app.state::<AudioState>();
                let mut audio = audio_state.0.lock().map_err(|_| "Playback unavailable".to_string())?;
                audio.invalidate_playback_intents();
                audio.mark_queue_uncertain();
                Ok(())
            }, || stop_supervisor(&app.state::<SupervisorState>().0));
            format!("FIRST_RUN_CATALOG_PENDING: {}", error_with_rollback(error, cleanup))
        });
        let failed = result.is_err();
        operation.finish(result, failed);
        if !failed {
            let _ = app.emit(LOCAL_LIBRARY_RECONCILED_EVENT, ());
            if resume_initial && start_mpd {
                restore_profile_services_after_resume(&app);
            }
        }
    });
}

fn retirement_consent_for_profile(
    library: &Path,
    current: &AppConfig,
    next: &AppConfig,
) -> Result<Option<local_media::mirror::RemovalConsentSources>, String> {
    let roots = local_media::mirror::canonical_configured_roots(
        &next
            .local_folders
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
    )?
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    match std::fs::symlink_metadata(library.join(local_media::mirror::MANIFEST_NAME)) {
        Ok(_) => {
            let manifest = local_media::mirror::read_manifest(library)?;
            let previous = manifest
                .sources
                .values()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            if roots.is_subset(&previous) && roots.len() < previous.len() {
                if manifest.registry_id.is_none() {
                    return Err("LOCAL_SOURCE_REMOVAL_PLAN_UNCONFIRMED".into());
                }
                return local_media::mirror::source_removal_consent(
                    &manifest.sources,
                    &roots.into_iter().collect::<Vec<_>>(),
                )?
                .map(Some)
                .ok_or_else(|| "LOCAL_SOURCE_REMOVAL_PLAN_UNCONFIRMED".into());
            }
            Ok(None)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Preserve legacy addition admission, but never silently rebuild a
            // legacy collection as a side effect of a source-only removal.
            let previous = local_media::mirror::canonical_configured_roots(
                &current
                    .local_folders
                    .iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>(),
            )?
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
            if roots.is_subset(&previous) && roots.len() < previous.len() {
                return Err("LOCAL_SOURCE_REMOVAL_PLAN_UNCONFIRMED".into());
            }
            Ok(None)
        }
        Err(_) => Err("LOCAL_SOURCE_REMOVAL_PLAN_UNCONFIRMED".into()),
    }
}

// A retirement updates only source IDs which no longer exist in the virtual
// library. There is no complete-generation construction or Full-plan fallback.
fn run_source_retirement_save(
    app: &AppHandle,
    current: &AppConfig,
    next: &AppConfig,
    sources: &local_media::mirror::RemovalConsentSources,
    diagnostics: &SettingsSaveDiagnostics,
    operation: &config_save_operation::Operation,
    uncertain: &Cell<bool>,
    authorization: Option<&QueueClearAuthorization>,
) -> Result<(), String> {
    let owner = app.state::<profile_lease::State>().lease()?;
    let audio_state = app.state::<AudioState>();
    let socket = audio_socket_for_removal_consent(&audio_state.0)?;
    if authorization.is_none() {
        early_removal_queue_consent(&audio_state.0, &socket, current, next, sources)?;
    }
    let plan = local_media::mirror::PureSourceRemoval {
        removed_ids: sources.removed_ids.clone(),
        remaining_ids: sources.remaining_ids.clone(),
        refresh_plan: local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: sources.removed_ids.clone(),
            changed_files: 0,
        },
    };
    // Existing affected-queue admission keeps exact source/config/queue consent.
    // An unaffected Plex or paused logical queue needs no mapping to local URIs:
    // both logical and native structures are kept unchanged and rechecked.
    let mut confirmed = admit_confirmed_queue_removal(
        &audio_state.0,
        &socket,
        current,
        next,
        &plan,
        authorization,
    )?;
    let untouched = if confirmed.is_none() {
        if authorization.is_some() {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
        }
        Some(admit_untouched_retirement_queue(
            &audio_state.0,
            &socket,
            &plan,
        )?)
    } else {
        None
    };
    let before = if let Some(confirmed) = &confirmed {
        confirmed.catalog_before.clone()
    } else {
        untouched
            .as_ref()
            .ok_or("LOCAL_SOURCE_REMOVAL_QUEUE_UNCONFIRMED")?
            .1
            .before_catalog
            .clone()
    };
    let expected = RemovalCatalogExpectation::from_before(&plan, before.clone())
        .ok_or("LOCAL_SOURCE_REMOVAL_CATALOG_UNCONFIRMED")?;
    let cancelled = || operation.shutdown_requested();
    recovery_fs::check(&cancelled)?;
    let captured = audio_state
        .0
        .lock()
        .map_err(|_| "Playback unavailable")?
        .get_queue();
    let cache = owner
        .root()
        .read("queue_cache.json", 16 * 1024 * 1024, &cancelled)?;
    recovery_set::validate_queue_capture(cache.as_deref(), &captured)?;
    diagnostics.strategy.set("retire_sources");
    operation.phase(config_save_operation::Phase::Removing);
    let coverage_token = app.state::<index_coverage::State>().begin()?;
    let started = Instant::now();
    let retirement = match source_retirement::prepare(
        &owner,
        current,
        next,
        before,
        &cancelled,
        &recovery_catalog::inspect,
    ) {
        Ok(tx) => tx,
        Err(error) => {
            if owner
                .root()
                .exists(source_retirement::PENDING)
                .unwrap_or(true)
            {
                uncertain.set(true);
                return Err(error_with_rollback(
                    error,
                    stop_supervisor(&app.state::<SupervisorState>().0),
                ));
            }
            return Err(error);
        }
    };
    diagnostics.stage("retirement_backup", started, "ok");
    let result = (|| -> Result<(), String> {
        if retirement.removed_ids() != plan.removed_ids || retirement.after() != expected.0 {
            return Err("LOCAL_SOURCE_REMOVAL_PLAN_CHANGED".into());
        }
        recovery_fs::check(&cancelled)?;
        if let Some(confirmed) = confirmed.as_mut() {
            clear_retirement_queue(confirmed, &audio_state.0, &retirement, &owner, &cancelled)?;
        }
        let expected_queue = if let Some(confirmed) = &confirmed {
            &confirmed.queue
        } else {
            &untouched
                .as_ref()
                .ok_or("LOCAL_SOURCE_REMOVAL_QUEUE_UNCONFIRMED")?
                .1
                .queue
        };
        if !removal_queue_before_publish(&socket, expected_queue)? {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".into());
        }
        if AudioEngine::read_validated_local_catalog_at(&socket)? != retirement.before() {
            return Err("LOCAL_SOURCE_REMOVAL_CATALOG_CHANGED".into());
        }
        let started = Instant::now();
        retirement.publish(&owner, &cancelled, &|_| Ok(()))?;
        diagnostics.stage("retirement_publish", started, "ok");
        operation.phase(config_save_operation::Phase::Removing);
        request_refresh_and_inspect(
            &socket,
            &plan.refresh_plan,
            Some(diagnostics),
            Some(operation),
            || {
                if AudioEngine::read_validated_local_catalog_at(&socket)? != expected.0 {
                    return Err("LOCAL_SOURCE_REMOVAL_CATALOG_MISMATCH".into());
                }
                if !removal_queue_before_publish(&socket, expected_queue)? {
                    return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".into());
                }
                Ok(())
            },
        )?;
        recovery_fs::check(&cancelled)?;
        // The transaction slot excludes another writer; refuse an unexpected
        // memory change before exchanging the durable configuration.
        if *app
            .state::<ConfigState>()
            .0
            .lock()
            .map_err(|_| "Config unavailable")?
            != *current
        {
            return Err("LOCAL_SOURCE_REMOVAL_CONFIG_CHANGED".into());
        }
        retirement.commit(&owner, &cancelled, &recovery_catalog::inspect, &|_| Ok(()))?;
        adopt_retired_source_config(app, current, next)?;
        Ok(())
    })();
    if let Err(error) = result {
        uncertain.set(true);
        // No rollback of a live daemon's DB. Stop only the owned runtime and
        // retain the journal/queue fence for the explicit recovery protocol.
        if let Some(confirmed) = confirmed.as_mut() {
            if let Ok(mut audio) = audio_state.0.lock() {
                audio.retain_uncertain_queue_reservation(&mut confirmed.owner);
            }
        }
        let stopped = stop_supervisor(&app.state::<SupervisorState>().0);
        return Err(error_with_rollback(error, stopped));
    }
    if let Some(confirmed) = confirmed.as_mut() {
        confirmed.owner.release_after_confirmed_completion();
    }
    inherit_index_coverage(app, coverage_token, &next.local_folders);
    install_saved_folders_watcher(app, None, true);
    Ok(())
}

fn admit_untouched_retirement_queue(
    audio: &Mutex<AudioEngine>,
    socket: &str,
    plan: &local_media::mirror::PureSourceRemoval,
) -> Result<(audio::QueueStructureReservation, RemovalRefreshExpectation), String> {
    let (reservation, logical) = {
        let mut audio = audio.lock().map_err(|_| "Playback unavailable")?;
        let owner = audio
            .reserve_queue_structure()
            .ok_or("LOCAL_SOURCE_REMOVAL_QUEUE_UNCONFIRMED")?;
        (owner, audio.get_queue())
    };
    let queue = AudioEngine::capture_mpd_queue_structure(socket).map_err(|e| e.to_string())?;
    if queue_references_removed_sources(plan, &logical, &queue) {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".into());
    }
    let before = AudioEngine::read_validated_local_catalog_at(socket)?;
    let catalog = RemovalCatalogExpectation::from_before(plan, before.clone())
        .ok_or("LOCAL_SOURCE_REMOVAL_CATALOG_UNCONFIRMED")?;
    if !removal_queue_before_publish(socket, &queue)? {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".into());
    }
    Ok((
        reservation,
        RemovalRefreshExpectation {
            before_catalog: before,
            catalog,
            queue,
        },
    ))
}

fn clear_retirement_queue(
    confirmed: &mut ConfirmedQueueRemoval,
    audio: &Mutex<AudioEngine>,
    retirement: &source_retirement::Transaction,
    owner: &profile_lease::Lease,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    recovery_fs::check(cancel)?;
    let (effect, socket) = {
        let mut audio = audio.lock().map_err(|_| "Playback unavailable")?;
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
    if confirmed.logical_resume.is_some()
        && AudioEngine::capture_mpd_playback_snapshot(&socket)? != confirmed.playback
    {
        return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
    }
    retirement.validate_before_effect(owner, cancel)?;
    let (empty, bytes) = effect.clear_reserved_queue(&confirmed.queue)?;
    audio
        .lock()
        .map_err(|_| "Playback unavailable")?
        .apply_verified_reserved_queue(&confirmed.owner, Vec::new())?;
    confirmed.queue = empty;
    retirement.persist_queue_cache(owner, &bytes)
}

fn adopt_retired_source_config(
    app: &AppHandle,
    previous: &AppConfig,
    next: &AppConfig,
) -> Result<(), String> {
    let state = app.state::<ConfigState>();
    let plex_state = app.state::<PlexState>();
    let mut config = state.0.lock().map_err(|_| "Config unavailable")?;
    let mut plex = plex_state.0.lock().map_err(|_| "Plex unavailable")?;
    if *config != *previous || AppConfig::load()? != *next {
        return Err("LOCAL_SOURCE_REMOVAL_CONFIG_CHANGED".into());
    }
    // The journal already durably exchanged config.json; do not save it again.
    plex.update_config(next);
    *config = next.clone();
    app.state::<Arc<direct_playback::Gate>>()
        .commit_choice(next);
    Ok(())
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
    let profile_owner = app.state::<profile_lease::State>().lease()?;
    profile_owner.validate()?;
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

    if let Some(sources)=retirement_consent_for_profile(&MpdSupervisor::library_dir(),&current_config,new_config)?{
        return run_source_retirement_save(app,&current_config,new_config,&sources,diagnostics,operation,uncertain,queue_clear_authorization);
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
    let removal_sources = if previous_sources.topology() == local_media::LocalLibraryTopology::Managed
        && local_media::mirror::read_manifest(&live_path)?.registry_id.is_some() {
        local_media::mirror::source_removal_consent(previous_sources.sources(), &roots)?
    } else { None };
    if queue_clear_authorization.is_none() {
        if let Some(sources) = &removal_sources {
            let socket = audio_socket_for_removal_consent(&app.state::<AudioState>().0)?;
            early_removal_queue_consent(&app.state::<AudioState>().0, &socket, &current_config, new_config, sources)?;
        }
    }
    // A source removal retires existing watches after commit; it neither
    // enumerates surviving physical trees nor starts new recursive monitoring.
    let prepared_watch = if removal_sources.is_some() { None } else {
        Some(app.state::<LocalWatcherState>().prepare(roots.clone())?)
    };
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
        let result = persist_experimental_local_folders_config(app, &current_config, new_config).map_err(|error| {
            uncertain.set(matches!(&error, ConfigSaveError::AfterRenameUnconfirmed(_)));
            config_save_error_message(&error)
        });
        if result.is_ok() && !uncertain.get() {
            if let Ok(token)=app.state::<index_coverage::State>().begin(){inherit_index_coverage(app,token,&new_config.local_folders);}
        }
        return after_confirmed_local_folders_save(result, uncertain.get(), || install_saved_folders_watcher(app, prepared_watch, removal_sources.is_some()));
    };

    if operation.shutdown_requested() {
        return Err(error_with_rollback(
            config_save_operation::INTERRUPTED.to_string(),
            local_media::mirror::cleanup_prepared_managed_generation(&prepared),
        ));
    }

    let (mut refresh_plan, mut removal_plan) =
        plan_saved_folders_refresh(&live_path, &prepared, diagnostics)?;

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
    diagnostics.strategy.set(match refresh_plan {
        local_media::mirror::ManagedRefreshPlan::MetadataOnly => "metadata",
        local_media::mirror::ManagedRefreshPlan::Targeted { .. } => "update_targeted",
        local_media::mirror::ManagedRefreshPlan::Full => "rescan",
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
    let coverage_token = app.state::<index_coverage::State>().begin()?;
    let coverage_check = if refresh_plan == local_media::mirror::ManagedRefreshPlan::MetadataOnly { None } else {
        let before = confirmed_removal.as_ref().map(|c| c.catalog_before.clone())
            .or_else(|| removal_guard.as_ref().map(|(_,proof)|proof.before_catalog.clone()));
        Some(prepare_index_check(&socket_path, prepared.coverage.clone(), previous_sources.sources().keys().cloned().collect(),
            || operation.shutdown_requested(), before).map_err(|error|error_with_rollback(error, local_media::mirror::cleanup_prepared_managed_generation(&prepared)))?)
    };
    let coverage_report = std::cell::RefCell::new(None);
    let coverage_catalog = std::cell::RefCell::new(None);
    let queue_capture = (|| -> Result<(), String> {
        let queue_tracks = audio_state.0.lock().map_err(|_|"LIBRARY_RECOVERY_QUEUE_UNPROVEN")?.durable_queue_tracks();
        let queue_cache = profile_owner.root().read("queue_cache.json",16*1024*1024,&||operation.shutdown_requested())?;
        recovery_set::validate_queue_capture(queue_cache.as_deref(),&queue_tracks)
    })();
    queue_capture.map_err(|error|error_with_rollback(error,local_media::mirror::cleanup_prepared_managed_generation(&prepared)))?;
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
    let prior_catalog = coverage_check.as_ref().map(|check|check.previous_records());
    if let Err(error)=recovery_set::capture_before(&profile_owner,prior_catalog.as_ref(),&||operation.shutdown_requested(),&recovery_catalog::inspect){
        return Err(error_with_rollback(error,intent.abort_unpublished(&prepared)));
    }
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
    let mut playback_window = AdditionPlaybackWindow::new(&transaction_state.addition_playback, &audio_state.0, operation);
    if added_sources && removal_plan.is_none()
        && previous_kind != local_media::mirror::ManagedMirrorPreviousKind::Legacy
        && matches!(managed_plan, local_media::mirror::ManagedRefreshPlan::Targeted { .. }) {
        if playback_window.open(&previous_sources, coverage_check.as_ref(), &live_path, &socket_path, &supervisor_state.0).is_err() {
            // Optional concurrency must not change Save admission or recovery.
            playback_window.close()?;
            eprintln!("[Audio] Confirmed playback unavailable during addition; waiting for completion.");
        }
    }
    let refresh_result = request_refresh_and_inspect(&socket_path, managed_plan, Some(diagnostics), Some(operation), || {
        playback_window.close()?;
        let check = coverage_check.as_ref().ok_or(index_coverage::INVALID)?;
        let (report, catalog) = check.finish_at(&socket_path, &live_path)?;
        let expected = removal_guard.as_ref().map(|(_,proof)|proof);
        if let Some(expected) = expected {
            if catalog.records != expected.catalog.0 { return Err("LOCAL_SOURCE_REMOVAL_CATALOG_MISMATCH".into()); }
            if AudioEngine::capture_mpd_queue_structure(&socket_path).map_err(|e|e.to_string())? != expected.queue { return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".into()); }
        } else if let Some(confirmed) = confirmed_removal.as_ref() {
            if catalog.records != confirmed.catalog_after.0 { return Err("LOCAL_SOURCE_REMOVAL_CATALOG_MISMATCH".into()); }
            if AudioEngine::capture_mpd_queue_structure(&socket_path).map_err(|e|e.to_string())? != confirmed.queue { return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED".into()); }
        }
        *coverage_catalog.borrow_mut() = Some(catalog.records.clone());
        *coverage_report.borrow_mut() = Some(report);
        Ok(())
    });
    // Failure to drain is a hard fence: preserve intent/generations, never stop,
    // roll back or commit while an admitted effect could still run.
    if let Err(primary) = &refresh_result {
        playback_window.abort().map_err(|cleanup| format!("{primary}; {cleanup}"))?;
    } else { playback_window.close()?; }
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
    let prepared_database = std::cell::RefCell::new(None::<recovery_set::PreparedDatabase>);
    let result = complete_published_mirror_refresh(
        || {
            refresh_result.take().unwrap_or_else(|| Err("Refresh já consumido.".to_string()))?;
            // A validator refusal is a pre-commit refresh failure. Recover the
            // previous library before changing config, rather than declaring
            // config persistence uncertain solely because a validator failed.
            let proof = recovery_set::prepare_database(&profile_owner, coverage_catalog.borrow().as_ref(),
                &||operation.shutdown_requested(), &recovery_catalog::inspect)?;
            *prepared_database.borrow_mut() = Some(proof);
            Ok(())
        },
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
                let outcome = covered_index_recovery(outcome, coverage_check.as_ref(), &socket_path)?;
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
            let result = (|| {
                let proof = prepared_database.borrow();
                let proof = proof.as_ref().ok_or_else(|| ConfigSaveError::BeforeRename("LIBRARY_RECOVERY_DATABASE_UNCONFIRMED".into()))?;
                proof.validate_current(&profile_owner, &||operation.shutdown_requested()).map_err(ConfigSaveError::BeforeRename)?;
                persist_experimental_local_folders_config(app, &current_config, new_config)?;
                // Keep confirm_next's complete profile/config/generation/scope
                // checks; reuse only the exact already validated DB contents.
                recovery_set::confirm_next(&profile_owner, coverage_catalog.borrow().as_ref(), &||operation.shutdown_requested(),
                    &|bytes, cancel|proof.validate_bytes(&profile_owner.root().id(), bytes, cancel))
                    .map_err(ConfigSaveError::AfterRenameUnconfirmed)
            })();
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
    let (mut result, mut unconfirmed) = intent.conclude_transaction(result, completed.get());
    if !unconfirmed {
        let validate = |bytes: &[u8], cancel: &dyn Fn() -> bool| {
            if matches!(completed.get(), Some(managed_save_intent::Completed::Committed)) {
                let proof = prepared_database.borrow();
                proof.as_ref().ok_or("LIBRARY_RECOVERY_DATABASE_UNCONFIRMED".to_string())?
                    .validate_bytes(&profile_owner.root().id(), bytes, cancel)
            } else {
                // Recovery validates the previous database, never the rejected one.
                recovery_catalog::inspect(bytes, cancel)
            }
        };
        if let Err(error)=recovery_set::finish_save(&profile_owner,&||operation.shutdown_requested(),&validate){
            result=Err(format!("{error}; {}: conclusão do conjunto de recuperação pendente.",managed_save_intent::BLOCKED));unconfirmed=true;
        }
    }
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
    if result.is_ok() && !unconfirmed {
        if let Some(report) = coverage_report.into_inner() {
            operation.coverage(report.clone());
            publish_index_coverage(app, coverage_token, &new_config.local_folders, report);
        } else { inherit_index_coverage(app,coverage_token,&new_config.local_folders); }
    }
    after_confirmed_local_folders_save(result, unconfirmed, || install_saved_folders_watcher(app, prepared_watch, removal_sources.is_some()))
}

/// Owns the addition-only permission; every return/panic revokes it. No
/// Coordinator/Audio/Supervisor mutex is held while draining admitted effects.
struct AdditionPlaybackWindow<'a> {
    gate: &'a initial_plex_playback::Gate,
    audio: &'a Mutex<AudioEngine>,
    operation: &'a config_save_operation::Operation,
    opened: bool,
}
impl<'a> AdditionPlaybackWindow<'a> {
    fn new(gate: &'a initial_plex_playback::Gate, audio: &'a Mutex<AudioEngine>, operation: &'a config_save_operation::Operation) -> Self {
        Self { gate, audio, operation, opened: false }
    }
    fn open(&mut self, previous: &local_media::ResolvedLocalSources, check: Option<&index_coverage::Check>,
        library: &Path, socket: &str, supervisor: &Mutex<MpdSupervisor>) -> Result<(), String> {
        let scope = initial_plex_playback::Addition::capture(library, previous.sources(), check.ok_or(index_coverage::INVALID)?.previous_records())?;
        let pid = supervisor.lock().map_err(|_| "MPD unavailable")?.observed_process_pid().ok_or("MPD unavailable")?;
        let mut audio = self.audio.lock().map_err(|_| "Playback unavailable")?;
        audio.ensure_queue_coherent()?;
        if audio.startup_queue_pending() { return Err("PLAYBACK_QUEUE_UNCERTAIN".into()); }
        let queue = AudioEngine::capture_mpd_queue_structure(socket).map_err(|_| "PLAYBACK_QUEUE_UNCERTAIN")?;
        let uris = queue.entries.into_iter().map(|entry| entry.uri).collect::<Vec<_>>();
        scope.verify_queue(&audio.get_queue(), &uris)?;
        AudioEngine::send_owned_command_to_socket(socket, pid, "status")?;
        self.gate.publish_addition(self.operation.id().into(), socket.into(), pid, uris, scope)?;
        self.opened = true;
        audio.invalidate_playback_intents();
        drop(audio);
        self.operation.addition_playback(true)
    }
    fn close(&mut self) -> Result<(), String> { self.finish(false) }
    fn abort(&mut self) -> Result<(), String> { self.finish(true) }
    fn finish(&mut self, failed: bool) -> Result<(), String> {
        if !self.opened { return Ok(()); }
        let admission = if self.operation.shutdown_requested() { Ok(()) } else { self.operation.addition_playback(false) };
        if failed { self.gate.invalidate(); self.gate.drain()?; } else { self.gate.retire()?; }
        let mut audio = self.audio.lock().map_err(|_| "Playback unavailable")?;
        audio.invalidate_playback_intents();
        // A partial command or failed cache write must never reach commit or
        // silently restore the pre-intervention cache. Preserve recovery state.
        audio.ensure_queue_persisted()?;
        self.opened = false;
        admission
    }
}
impl Drop for AdditionPlaybackWindow<'_> {
    fn drop(&mut self) {
        if self.abort().is_err() { eprintln!("[Audio] Addition playback revocation unconfirmed; Save remains blocked."); }
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
fn claim_audio_analyzer_host(
    host: String,
    session_id: String,
    window: Window,
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<analyzer::AnalyzerOwner, String> {
    if window.label() != "main"
        || !analyzer::authorized_analyzer_host(&host)
        || app.get_webview_window(&host).is_none()
    {
        return Err("Transferência do analyzer não autorizada.".into());
    }
    let mut analyzer = analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?;
    if app.get_webview_window(&host).is_none() {
        return Err("Host do analyzer fechado.".into());
    }
    analyzer.claim_for_host(&host, session_id, &app)
}

#[tauri::command]
fn release_audio_analyzer_host(
    session_id: String,
    generation: u64,
    window: Window,
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Revogação do analyzer não autorizada.".into());
    }
    analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?
        .release_owner(&session_id, generation, &app)
}

#[tauri::command]
fn start_audio_analyzer(
    session_id: String,
    generation: u64,
    lease_id: String,
    window: Window,
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    if !analyzer::authorized_analyzer_host(window.label())
        || app.get_webview_window(window.label()).is_none()
    {
        return Err("Host do analyzer não autorizado ou fechado.".into());
    }
    let config_state = app.state::<ConfigState>();
    let config = config_state.0.lock().map_err(|_| "ANALYZER_MODE_UNAVAILABLE")?;
    if !config.full_mode() { return Err("ANALYZER_MODE_DISABLED".into()); }
    drop(config);
    let mut analyzer = analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?;
    if app.get_webview_window(window.label()).is_none() {
        return Err("Host do analyzer fechado.".into());
    }
    analyzer.start_for_host(window.label(), &session_id, generation, lease_id, app)
}

#[tauri::command]
fn stop_audio_analyzer(
    session_id: String,
    generation: u64,
    lease_id: String,
    window: Window,
    app: AppHandle,
    analyzer_state: State<AnalyzerState>,
) -> Result<(), String> {
    if !analyzer::authorized_analyzer_host(window.label()) {
        return Err("Host do analyzer não autorizado.".into());
    }
    analyzer_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar o analyzer: {}", e))?
        .stop_for_host(window.label(), &session_id, generation, &lease_id, &app)
}

#[derive(Debug, Clone, Serialize)]
pub struct MpdStatusSnapshot {
    pub manual_check_playback: bool,
    pub manual_check_artwork: bool,
    pub health: MpdHealth,
    pub playback: Option<PlaybackStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output_observation: Option<alsa_observation::Observation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) engine_evidence: Option<mpd_engine::EngineEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shared_dsp: Option<shared_dsp::Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) audio_route: Option<crate::audio_route::AudioRouteProjection>,
}

impl ConfigTransactionState {
    fn new() -> Self {
        let operations = Arc::new(config_save_operation::Coordinator::new());
        let manual_check_playback = operations.manual_check_playback.clone();
        Self {
            operations,
            initial_plex: initial_plex_playback::Gate::default(),
            addition_playback: initial_plex_playback::Gate::default(),
            manual_check_playback,
            startup_config_read_gate: Mutex::new(false),
            source_mirror_cache: source_availability::MirrorReadCache::default(),
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

    fn begin_audio_transition(
        &self,
        requested: crate::audio_route::AudioRoute,
        kind: config_save_operation::AudioTransitionKind,
        before: impl FnOnce() -> Result<(), String>,
    ) -> Result<(
        config_save_operation::Transaction<'_>,
        config_save_operation::AudioTransitionHandle,
    ), String> {
        let mut transaction = self.operations.begin()?;
        before()?;
        let generation = transaction.begin_audio_transition(requested, kind)?;
        Ok((transaction, self.operations.audio_transition_handle(generation)))
    }

    fn audio_transition_snapshot(
        &self,
    ) -> Option<config_save_operation::AudioTransitionSnapshot> {
        self.operations.audio_transition_snapshot().ok().flatten()
    }

    fn lock_startup_config_reads(
        &self,
        mode: ManagedMirrorRefreshMode,
    ) -> Result<Option<StartupConfigReadGuard<'_>>, String> {
        if !matches!(mode, ManagedMirrorRefreshMode::Startup | ManagedMirrorRefreshMode::Check) {
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
                self.initial_plex.invalidate();
                self.addition_playback.invalidate();
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
async fn get_playback_status(app: AppHandle, diagnostic_id: Option<u64>) -> Result<PlaybackStatus, String> {
    let diagnostic = diagnostics::DevSpan::ipc("health.legacy", diagnostic_id);
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        let mut diagnostic = diagnostic;
        let result = (|| {
            let shared_before = app.state::<SupervisorState>().0.lock().map_err(|_| "Controle de volume indisponível.")?.shared_mute_context();
            let before = app.state::<SupervisorState>().0.lock().map_err(|_| "Controle de volume indisponível.")?.volume_context();
            diagnostic.phase("wait.audio");
            let audio = app.state::<AudioState>();
            let audio = audio.0.lock().map_err(|_| "Estado de reprodução indisponível.".to_string())?;
            diagnostic.phase("mpd.status");
            let mut playback = audio.get_telemetry_status()?;
            drop(audio);
            diagnostic.phase("wait.supervisor");
            let supervisor = app.state::<SupervisorState>();
            let supervisor = supervisor.0.lock().map_err(|_| "Controle de volume indisponível.".to_string())?;
            diagnostic.phase("volume.backend");
            let after = supervisor.volume_context();
            drop(supervisor);
            diagnostic.phase("volume.read");
            apply_volume_context(&mut playback, &before, &after);
            attach_shared_mute(&mut playback,&app.state::<SupervisorState>().0,shared_before)?;
            listening_history::observe(&app, &playback);
            upnp_bridge::publish(&app, &playback, true);
            Ok(playback)
        })();
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta de reprodução indisponível.".to_string())?
}

#[tauri::command]
async fn get_mpd_status_snapshot(app: AppHandle, diagnostic_id: Option<u64>) -> Result<MpdStatusSnapshot, String> {
    let diagnostic = diagnostics::DevSpan::ipc("health.snapshot", diagnostic_id);
    diagnostic.phase("executor.queued");
    let route_snapshot_generation = next_audio_route_snapshot_generation();
    tauri::async_runtime::spawn_blocking(move || {
        let mut diagnostic = diagnostic;
        let before = output_observation_context(&app);
        let result = collect_mpd_status_snapshot_with_diagnostic(
            &app.state::<SupervisorState>().0, &app.state::<AudioState>().0, &diagnostic)
            .map(|mut snapshot| {
                snapshot.manual_check_playback = app.state::<ConfigTransactionState>().manual_check_playback.active();
                snapshot.manual_check_artwork = app.state::<ConfigTransactionState>().manual_check_playback.artwork_ready();
                let playing = snapshot.health == MpdHealth::Available
                    && snapshot.playback.as_ref().is_some_and(|p| p.state == "play" && p.snapshot_consistent);
                snapshot.output_observation = Some(alsa_observation::sample(playing, before.clone(),
                    || output_observation_context(&app), alsa_observation::observe));
                if playing && matches!(snapshot.output_observation, Some(alsa_observation::Observation::Observed { .. })) {
                    if let Some(context) = before.as_ref().filter(|c| !c.shared) {
                        let socket = {
                            let audio = app.state::<AudioState>();
                            audio.0.try_lock().ok().map(|audio| audio.local_album_socket_path())
                        };
                        let context_valid = output_observation_context(&app).as_ref() == Some(context);
                        // Only DSD needs an additional negotiated-transport query. It is
                        // read-only, bounded and uses this snapshot, without another timer.
                        let transport = if context_valid && snapshot.playback.as_ref().is_some_and(|p| p.audio_format.starts_with("dsd") || p.audio_format.contains(":dsd:")) {
                            socket.as_ref().and_then(|socket| signal_processing::read_output_transport(Path::new(socket), context.pid))
                        } else { None };
                        if !context_valid || output_observation_context(&app).as_ref() != Some(context) {
                            snapshot.output_observation = Some(alsa_observation::Observation::unverified(alsa_observation::Reason::Changed));
                            if let Some(playback) = snapshot.playback.as_mut() { playback.processing = None; }
                        } else if let Some(processing) = snapshot.playback.as_mut().and_then(|p| p.processing.as_mut()) {
                            processing.transport = transport;
                        }
                    }
                }
                if snapshot.playback.as_ref().is_some_and(|p| !p.snapshot_consistent) {
                    snapshot.output_observation = Some(alsa_observation::Observation::unverified(alsa_observation::Reason::Changed));
                }
                if let Some(dsp) = app.try_state::<shared_dsp::State>() {
                    dsp.observe_playing(playing);
                    let mut state=dsp.snapshot();
                    state.in_path=app.state::<SupervisorState>().0.try_lock().ok().map(|s|s.shared_eq_pipe().is_some());
                    if state.in_path==Some(true)&&state.phase==shared_dsp::Phase::Off {state.phase=if state.error.is_some(){shared_dsp::Phase::Error}else{shared_dsp::Phase::Stopping};}
                    snapshot.shared_dsp = Some(state);
                }
                // Build provenance only: reuse the owned process/route context,
                // without hashing, filesystem reads or extra MPD commands.
                snapshot.engine_evidence = mpd_engine::sample_evidence(
                    playing && matches!(snapshot.output_observation, Some(alsa_observation::Observation::Observed { .. })),
                    before.as_ref(),
                    || output_observation_context(&app),
                    |context| {
                        let supervisor = app.state::<SupervisorState>();
                        let evidence = supervisor.0.try_lock().ok().and_then(|s| s.engine_evidence_for_pid(context.pid));
                        evidence
                    },
                );
                if let Some(playback) = &snapshot.playback { listening_history::observe(&app, playback); upnp_bridge::publish(&app, playback, true); }
                snapshot.audio_route = Some(project_linux_audio_route(
                    &app,
                    &snapshot,
                    route_snapshot_generation,
                ));
                snapshot
            });
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta de saúde indisponível.".to_string())?
}

// Snapshot only, no I/O under application locks, no new IPC or transport work.
fn output_observation_context(app: &AppHandle) -> Option<alsa_observation::Context> {
    let transactions = app.state::<ConfigTransactionState>();
    if transactions.shutdown_requested() || transactions.shutdown_started() { return None; }
    transactions.operations.reject_active().ok()?;
    let (endpoint, shared) = {
        let config = app.state::<ConfigState>();
        let config = config.0.try_lock().ok()?;
        if config.first_run { return None; }
        (
            config.alsa_device.clone(),
            crate::audio_route::interpret_legacy_route(
                &config.audio_output_type,
                &config.alsa_device,
            )
            .route
            .access
                == crate::audio_route::AudioAccess::Shared,
        )
    };
    let pid = {
        let supervisor = app.state::<SupervisorState>();
        let supervisor = supervisor.0.try_lock().ok()?;
        supervisor.observed_process_pid()?
    };
    Some(alsa_observation::Context { pid, endpoint, shared })
}

fn observe_mpd_process(supervisor: &Mutex<MpdSupervisor>, diagnostic: &diagnostics::DevSpan) -> Result<MpdProcessObservation, String> {
    diagnostic.phase("wait.supervisor");
    let mut supervisor = supervisor.lock().map_err(|e| format!("Falha ao acessar a saúde do MPD: {}", e))?;
    diagnostic.phase("health.process");
    supervisor.observe_health()
}

fn unavailable_snapshot(health: MpdHealth) -> MpdStatusSnapshot {
    MpdStatusSnapshot {
        manual_check_playback: false,
        manual_check_artwork: false,
        health,
        playback: None,
        output_observation: None,
        engine_evidence: None,
        shared_dsp: None,
        audio_route: None,
    }
}

fn next_audio_route_snapshot_generation() -> u64 {
    let mut current = AUDIO_ROUTE_SNAPSHOT_GENERATION.load(Ordering::Acquire);
    loop {
        if current == u64::MAX {
            return current;
        }
        match AUDIO_ROUTE_SNAPSHOT_GENERATION.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return current + 1,
            Err(observed) => current = observed,
        }
    }
}

fn project_linux_audio_route(
    app: &AppHandle,
    snapshot: &MpdStatusSnapshot,
    generation: u64,
) -> crate::audio_route::AudioRouteProjection {
    use crate::audio_route::{
        project_audio_route, RouteDspState, RouteOperationState,
        RoutePlatform, RouteProjectionInput, RouteSupervisorState,
    };
    let persisted = app
        .state::<ConfigState>()
        .0
        .try_lock()
        .ok()
        .filter(|config| !config.first_run)
        .map(|config| crate::audio_route::interpret_legacy_route(
            &config.audio_output_type,
            &config.alsa_device,
        ));
    let runtime = app
        .state::<SupervisorState>()
        .0
        .try_lock()
        .ok()
        .map(|supervisor| supervisor.audio_route_observation());
    let supervisor_state = match (&snapshot.health, runtime.is_some()) {
        (_, false) => RouteSupervisorState::Unknown,
        (MpdHealth::Available, true) => RouteSupervisorState::Available,
        (MpdHealth::Starting, true) => RouteSupervisorState::Starting,
        (MpdHealth::Stopping, true) => RouteSupervisorState::Stopping,
        (MpdHealth::Unavailable(MpdUnavailableReason::StartupFailed), true) => RouteSupervisorState::Failed,
        (MpdHealth::Unavailable(_), true) => RouteSupervisorState::Unavailable,
    };
    let dsp_state = snapshot
        .shared_dsp
        .as_ref()
        .map(|dsp| match dsp.phase {
            shared_dsp::Phase::Off => RouteDspState::Off,
            shared_dsp::Phase::Preparing => RouteDspState::Preparing,
            shared_dsp::Phase::Ready => RouteDspState::Ready,
            shared_dsp::Phase::Active if dsp.verified && dsp.in_path == Some(true) => RouteDspState::ActiveVerified,
            shared_dsp::Phase::Active => RouteDspState::ActiveUnverified,
            shared_dsp::Phase::Updating => RouteDspState::Updating,
            shared_dsp::Phase::Stopping => RouteDspState::Stopping,
            shared_dsp::Phase::Error => RouteDspState::Failed,
        })
        .unwrap_or_else(|| {
            if runtime.as_ref().is_some_and(|value| value.eq_pipe_configured) {
                RouteDspState::ActiveUnverified
            } else {
                RouteDspState::Unavailable
            }
        });
    let applied = runtime.as_ref().and_then(|value| value.applied.clone());
    let configured = persisted.as_ref().map(|value| &value.route);
    let route_differs = match (configured, applied.as_ref()) {
        (Some(requested), Some(current)) => {
            requested.destination != current.destination || requested.access != current.access
        }
        (Some(_), None) => true,
        _ => false,
    };
    let config_operation = if route_differs {
        app.state::<ConfigTransactionState>()
            .operations
            .snapshot()
            .ok()
            .flatten()
    } else {
        None
    };
    let config_operation_state = config_operation.as_ref().map(|operation| match operation.phase {
        config_save_operation::Phase::Preparing
        | config_save_operation::Phase::Removing
        | config_save_operation::Phase::Indexing
        | config_save_operation::Phase::Validating => RouteOperationState::Transitioning,
        config_save_operation::Phase::Failed => RouteOperationState::Failed,
        config_save_operation::Phase::Uncertain => RouteOperationState::Uncertain,
        config_save_operation::Phase::Completed => RouteOperationState::Stable,
    });
    let operation_diagnostic = config_operation.as_ref().and_then(|operation| match operation.phase {
        config_save_operation::Phase::Failed => Some(crate::audio_route::RouteDiagnostic::ConfigurationFailed),
        config_save_operation::Phase::Uncertain => Some(crate::audio_route::RouteDiagnostic::ConfigurationUncertain),
        _ => None,
    });
    let mut operation = runtime
        .as_ref()
        .map_or(RouteOperationState::Uncertain, |value| value.operation);
    if let Some(value) = config_operation_state {
        if value != RouteOperationState::Stable {
            operation = value;
        }
    }
    if matches!(dsp_state, RouteDspState::Preparing | RouteDspState::Ready | RouteDspState::Updating | RouteDspState::Stopping) {
        operation = RouteOperationState::Transitioning;
    } else if dsp_state == RouteDspState::Failed {
        operation = RouteOperationState::Failed;
    } else if dsp_state == RouteDspState::ActiveUnverified {
        operation = RouteOperationState::Uncertain;
    }
    let operation_id = config_operation
        .filter(|_| route_differs)
        .map(|value| format!("config:{}", value.id))
        .or_else(|| {
            snapshot.shared_dsp.as_ref().and_then(|dsp| {
                (dsp.revision > 0 && dsp_state != RouteDspState::Off)
                    .then(|| format!("dsp:{}", dsp.revision))
            })
        })
        .or_else(|| {
            runtime.as_ref().and_then(|value| {
                (value.generation > 0).then(|| format!("mpd:{}", value.generation))
            })
        });
    let direct_pcm_observed = matches!(
        snapshot.output_observation,
        Some(alsa_observation::Observation::Observed { .. })
    );
    let mut projection = project_audio_route(RouteProjectionInput {
        persisted,
        applied,
        supervisor: supervisor_state,
        playback_consistent: snapshot
            .playback
            .as_ref()
            .is_some_and(|playback| playback.snapshot_consistent),
        playing: snapshot
            .playback
            .as_ref()
            .is_some_and(|playback| playback.state == "play" && playback.snapshot_consistent),
        direct_pcm_observed,
        dsp: dsp_state,
        operation,
        diagnostic: operation_diagnostic,
        operation_id,
        generation,
        platform: RoutePlatform::Linux,
        null_output_test: runtime.as_ref().is_some_and(|value| value.null_output_test),
    });

    // The route coordinator is backed by the same admission guard used by
    // Config Save and EQ. Overlay only its current generation; stale worker
    // completions are discarded by the coordinator before reaching here.
    if let Some(transition) = app.state::<ConfigTransactionState>().audio_transition_snapshot() {
        match transition.state {
            RouteOperationState::Transitioning => {
                projection.operation = RouteOperationState::Transitioning;
                projection.operation_id = Some(format!("route:{}", transition.generation));
                let previously_confirmed = projection.route.as_ref().and_then(|route| route.confirmed.clone());
                projection.route = crate::audio_route::AudioRouteSnapshot::transitioning(
                    transition.requested,
                    previously_confirmed,
                ).ok();
            }
            RouteOperationState::Failed | RouteOperationState::Uncertain => {
                projection.operation = transition.state;
                projection.operation_id = Some(format!("route:{}", transition.generation));
                projection.diagnostic = Some(match (transition.kind, transition.state) {
                    (config_save_operation::AudioTransitionKind::Configuration, RouteOperationState::Failed) => crate::audio_route::RouteDiagnostic::ConfigurationFailed,
                    (config_save_operation::AudioTransitionKind::Configuration, RouteOperationState::Uncertain) => crate::audio_route::RouteDiagnostic::ConfigurationUncertain,
                    (config_save_operation::AudioTransitionKind::Equalizer, RouteOperationState::Failed) => crate::audio_route::RouteDiagnostic::DspFailed,
                    (config_save_operation::AudioTransitionKind::Equalizer, RouteOperationState::Uncertain) => crate::audio_route::RouteDiagnostic::DspNotVerified,
                    _ => unreachable!(),
                });
                projection.route = crate::audio_route::AudioRouteSnapshot::uncertain(transition.requested).ok();
            }
            RouteOperationState::Stable => {}
        }
    }
    projection
}

fn apply_volume_context(playback: &mut PlaybackStatus, before: &direct_volume::Context, after: &direct_volume::Context) {
    if before.backend != after.backend {
        playback.snapshot_consistent = false;
        playback.volume = VolumeStatus::pipewire_unavailable();
    } else if !before.apply(after, playback) {
        apply_volume_backend(playback, after.backend);
    }
}

fn apply_volume_backend(playback: &mut PlaybackStatus, backend: VolumeBackend) {
    if backend==VolumeBackend::PipeWire {
        playback.volume=match shared_volume::read_target().and_then(|target|shared_volume::read_target_volume(&target).map(|v|(target,v))) {
            Ok((target,v))=>{let mut volume=VolumeStatus::pipewire(v.value,v.muted);volume.shared_token=Some(target.token());volume},
            Err(_)=>VolumeStatus::pipewire_unavailable(),
        };
    } else { apply_volume_backend_with(playback,backend,shared_volume::read_pipewire_volume); }
}
fn attach_shared_mute(playback:&mut PlaybackStatus,supervisor:&Mutex<MpdSupervisor>,before:Option<String>)->Result<(),String>{
    if !matches!(playback.volume.backend,VolumeBackend::PipeWire|VolumeBackend::MpdSoftware) {return Ok(());}
    let sup=supervisor.lock().map_err(|_|"SHARED_MUTE_CONTEXT_CHANGED")?;
    let after=sup.shared_mute_context();
    if before.is_none()||before!=after {playback.snapshot_consistent=false;playback.volume=VolumeStatus::pipewire_unavailable();return Ok(());}
    let base=after.ok_or("SHARED_MUTE_CONTEXT_CHANGED")?;
    let token=if playback.volume.backend==VolumeBackend::PipeWire {format!("{}:{}",base,playback.volume.shared_token.as_deref().ok_or("SHARED_MUTE_CONTEXT_CHANGED")?)} else {base};
    // Poll replies may predate a volume gesture. They supply context only;
    // restoration history is captured from a fresh read inside the command.
    playback.volume.shared_token=Some(token);Ok(())
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

#[cfg(test)]
fn collect_mpd_status_snapshot(supervisor: &Mutex<MpdSupervisor>, audio: &Mutex<AudioEngine>) -> Result<MpdStatusSnapshot, String> {
    collect_mpd_status_snapshot_with_diagnostic(supervisor, audio, &diagnostics::DevSpan::new("health.test"))
}

fn collect_mpd_status_snapshot_with_diagnostic(
    supervisor: &Mutex<MpdSupervisor>,
    audio: &Mutex<AudioEngine>,
    diagnostic: &diagnostics::DevSpan,
) -> Result<MpdStatusSnapshot, String> {
    if let MpdProcessObservation::NotRunning(health) = observe_mpd_process(supervisor, diagnostic)? {
        return Ok(unavailable_snapshot(health));
    }

    let shared_before = supervisor.lock().map_err(|_| "Controle de volume indisponível.")?.shared_mute_context();
    let before = supervisor.lock().map_err(|_| "Controle de volume indisponível.")?.volume_context();
    diagnostic.phase("wait.audio");
    let playback_result = {
        let audio = audio.lock().map_err(|e| format!("Falha ao acessar o cliente MPD: {}", e))?;
        diagnostic.phase("mpd.status");
        audio.get_telemetry_status()
    };

    match playback_result {
        Ok(mut playback) => {
            let (health, after) = {
                diagnostic.phase("wait.supervisor");
                let mut supervisor = supervisor
                    .lock()
                    .map_err(|e| format!("Falha ao confirmar a saúde do MPD: {}", e))?;
                diagnostic.phase("health.process");
                match supervisor.observe_health()? {
                    MpdProcessObservation::Running => {
                        let health = supervisor.mark_available();
                        (health, supervisor.volume_context())
                    }
                    MpdProcessObservation::NotRunning(health) => {
                        return Ok(unavailable_snapshot(health));
                    }
                }
            };
            diagnostic.phase("volume.read");
            apply_volume_context(&mut playback, &before, &after);
            attach_shared_mute(&mut playback,supervisor,shared_before)?;
            Ok(MpdStatusSnapshot {
                manual_check_playback: false,
        manual_check_artwork: false,
                health,
                playback: Some(playback),
                output_observation: None,
        engine_evidence: None,
        shared_dsp: None,
        audio_route: None,
            })
        }
        Err(_) => {
            if let MpdProcessObservation::NotRunning(health) = observe_mpd_process(supervisor, diagnostic)? {
                return Ok(unavailable_snapshot(health));
            }

            diagnostic.phase("wait.audio");
            let probe = {
                let audio = audio.lock().map_err(|e| format!("Falha ao verificar o protocolo MPD: {}", e))?;
                diagnostic.phase("mpd.probe");
                audio.probe_mpd()
            };
            let reason = match probe {
                Err(MpdProbeFailure::SocketUnavailable) => {
                    MpdUnavailableReason::SocketUnavailable
                }
                Ok(()) | Err(MpdProbeFailure::ProtocolUnavailable) => {
                    MpdUnavailableReason::ProtocolUnavailable
                }
            };

            let health = {
                diagnostic.phase("wait.supervisor");
                let mut supervisor = supervisor
                    .lock()
                    .map_err(|e| format!("Falha ao concluir a saúde do MPD: {}", e))?;
                diagnostic.phase("health.process");
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
fn change_playback_option(
    audio_state: State<AudioState>,
    transactions: State<ConfigTransactionState>,
    action: playback_options::PlaybackOptionAction,
    expected: playback_options::PlaybackOptions,
) -> Result<playback_options::PlaybackOptions, String> {
    with_playback_control(&audio_state.0, &transactions, |audio| audio.change_playback_option(action, expected))
}

#[tauri::command]
fn toggle_playback(
    config_state: State<ConfigState>, audio_state: State<AudioState>,
    transactions: State<ConfigTransactionState>, intent: Option<String>, want_play:Option<bool>,
) -> Result<(),String> {
    audio_state.note_transport_intervention();
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante Play/Resume: {e}"))?;
        is_shared_output(&config)
    };
    let token=admit_or_validate_playback(&audio_state.0,&transactions,intent)?;
    finish_playback_intent(&audio_state.0,&transactions,&token,|audio|match want_play{Some(true)=>audio.play(is_shared),Some(false)=>audio.pause(),None=>audio.toggle_play_pause(is_shared)})
}

#[tauri::command]
fn next_track(
    config_state: State<ConfigState>, audio_state: State<AudioState>,
    transactions: State<ConfigTransactionState>, intent: Option<String>,
) -> Result<(),String> {
    audio_state.note_transport_intervention();
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante Next: {}", e))?;
        is_shared_output(&config)
    };
    let token=admit_or_validate_playback(&audio_state.0,&transactions,intent)?;
    finish_playback_intent(&audio_state.0,&transactions,&token,|audio|audio.next(is_shared))
}

#[tauri::command]
fn previous_track(
    config_state: State<ConfigState>, audio_state: State<AudioState>,
    transactions: State<ConfigTransactionState>, intent: Option<String>,
) -> Result<(),String> {
    audio_state.note_transport_intervention();
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante Previous: {}", e))?;
        is_shared_output(&config)
    };
    let token=admit_or_validate_playback(&audio_state.0,&transactions,intent)?;
    finish_playback_intent(&audio_state.0,&transactions,&token,|audio|audio.previous(is_shared))
}

#[tauri::command]
fn seek_playback(
    seconds: f64,
    transactions: State<ConfigTransactionState>,
    window: Window,
    config_state: State<ConfigState>,
    audio_state: State<AudioState>,
) -> Result<(), String> {
    audio_state.note_transport_intervention();
    let is_shared = {
        let config = config_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar configuração durante seek: {}", e))?;
        is_shared_output(&config)
    };
    let observation = diagnostics::now_playing_seek_begin(window.label(), seconds, is_shared);
    let result = with_playback_control(&audio_state.0, &transactions, |audio| audio.seek(seconds, is_shared));
    diagnostics::now_playing_seek_finish(observation, &result);
    result
}

// Audio -> short Coordinator snapshot -> session lease. Never a Plex await.
fn initial_playback_context(transactions: &ConfigTransactionState, token: Option<&str>) -> Result<Option<Arc<initial_plex_playback::Session>>, String> {
    if transactions.shutdown_requested() || transactions.shutdown_started() { return Err("PLAYBACK_SUPERSEDED".into()); }
    transactions.manual_check_playback.admit()?;
    let bound = match token { Some(token) => transactions.addition_playback.bound(token)?.or(transactions.initial_plex.bound(token)?), None => None };
    match transactions.operations.reject_playback() {
        Ok(()) => Ok(bound),
        Err(error) if error == config_save_operation::ADDITION_INDEXING => {
            let session = match bound { Some(session) => Some(session), None if token.is_none() => transactions.addition_playback.current()?, None => None };
            let id = transactions.operations.snapshot()?.filter(|op| op.confirmed_playback && op.phase == config_save_operation::Phase::Indexing).map(|op| op.id);
            session.filter(|session| session.is_addition() && id.as_deref() == Some(session.operation.as_str())).map(Some).ok_or(error)
        }
        Err(error) if error == initial_plex_playback::WAIT => {
            let session = match bound { Some(session) => Some(session), None if token.is_none() => transactions.initial_plex.current()?, None => None };
            let id = transactions.operations.initial_catalog_id()?;
            session.filter(|session| id.as_deref() == Some(session.operation.as_str())).map(Some).ok_or(error)
        }
        Err(error) => Err(error),
    }
}

fn admit_or_validate_playback(
    audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState, token: Option<String>,
) -> Result<String, String> {
    admit_or_validate_playback_checked(audio, transactions, token, |_| Ok(()))
}
fn admit_or_validate_playback_checked(
    audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState, token: Option<String>,
    check: impl FnOnce(&AudioEngine) -> Result<(), String>,
) -> Result<String, String> {
    let mut audio = audio.lock().map_err(|_| "Playback unavailable")?;
    check(&audio)?;
    if token.is_none() { audio.note_eq_intervention(); }
    let context = initial_playback_context(transactions, token.as_deref())?;
    if let Some(token) = token { audio.validate_playback_intent(&token)?; Ok(token) }
    else {
        let token = audio.admit_playback_intent()?;
        if let Some(context) = context {
            if context.is_addition() { transactions.addition_playback.bind(&token, context)?; }
            else { transactions.initial_plex.bind(&token, context)?; }
        } else { transactions.initial_plex.clear_intent(); transactions.addition_playback.clear_intent(); }
        Ok(token)
    }
}

enum InitialPlaybackEffect { Queue, Clear, VerifiedMedia(Vec<String>) }
fn initial_queue_uris(audio: &AudioEngine, context: &initial_plex_playback::Session, allow_empty: bool) -> Result<Vec<String>, String> {
    audio.ensure_queue_coherent()?;
    let queue = audio.get_queue();
    if (!allow_empty && queue.is_empty()) || audio.startup_queue_pending()
        || queue.iter().any(|track| match &track.media_locator {
            Some(MediaLocator::Upnp { .. } | MediaLocator::Radio { .. } | MediaLocator::Navidrome { .. }) => true,
            Some(MediaLocator::Plex { server_id, part_key, .. }) => server_id.is_empty() || part_key.is_empty(),
            Some(MediaLocator::Local { uri }) => !context.accepts_local(uri),
            None => !context.accepts_local(&track.uri),
        }) {
        return Err(initial_plex_playback::WAIT.into());
    }
    let actual = AudioEngine::capture_mpd_queue_structure(&audio.local_album_socket_path()).map_err(|_| "PLAYBACK_QUEUE_UNCERTAIN")?;
    if actual.entries.len() != queue.len() || queue.iter().zip(&actual.entries)
        .any(|(track, entry)| !crate::audio::queue_entry_matches_track(entry, track))
    { return Err("PLAYBACK_QUEUE_UNCERTAIN".into()); }
    Ok(actual.entries.into_iter().map(|entry| entry.uri).collect())
}
fn finish_playback_effect<T>(
    audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState, token: &str,
    effect: InitialPlaybackEffect, apply: impl FnOnce(&mut AudioEngine) -> Result<T, String>,
) -> Result<T, String> {
    let mut audio = audio.lock().map_err(|_| "Playback unavailable")?;
    // The attached command gate owns Check fencing at the actual MPD effect.
    // Taking the same read lock here could deadlock when publication is waiting.
    let context = initial_playback_context(transactions, Some(token))?;
    audio.validate_playback_intent(token)?;
    if let Some(context) = context {
        let _lease = context.enter()?;
        let before = initial_queue_uris(&audio, &context, !matches!(effect, InitialPlaybackEffect::Queue));
        let verified = before.and_then(|before| context.accepts_queue(&before));
        if let Err(error) = verified {
            if error == "PLAYBACK_QUEUE_UNCERTAIN" { audio.mark_queue_uncertain(); }
            return Err(error);
        }
        let result = apply(&mut audio);
        let accepted = result.is_ok() || matches!(&result, Err(error) if error == "PLAYBACK_PERSISTENCE_FAILED");
        // A rejected effect may have left the old queue intact or changed it
        // partially. Verify the appropriate proof without discarding its error.
        // Revocation skips even these read-only commands before owned stop.
        let after = initial_plex_playback::check_effect().and_then(|_| initial_queue_uris(&audio, &context, true)).and_then(|after| {
            if !accepted { return context.accepts_queue(&after); }
            match &effect {
                InitialPlaybackEffect::VerifiedMedia(expected) if expected == &after => context.confirm_queue(after),
                InitialPlaybackEffect::Clear if after.is_empty() => context.confirm_queue(after),
                InitialPlaybackEffect::Queue => context.accepts_queue(&after),
                _ => Err("PLAYBACK_QUEUE_UNCERTAIN".into()),
            }
        });
        if after.is_err() {
            audio.mark_queue_uncertain();
            return match result { Err(error) => Err(error), Ok(_) => Err("PLAYBACK_QUEUE_UNCERTAIN".into()) };
        }
        result
    } else { apply(&mut audio) }
}
fn finish_playback_intent<T>(
    audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState, token: &str,
    apply: impl FnOnce(&mut AudioEngine) -> Result<T, String>,
) -> Result<T, String> {
    finish_playback_effect(audio, transactions, token, InitialPlaybackEffect::Queue, apply)
}
pub(crate) fn with_playback_control<T>(audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState,
    apply: impl FnOnce(&mut AudioEngine) -> Result<T, String>) -> Result<T, String> {
    let token = admit_or_validate_playback(audio, transactions, None)?;
    finish_playback_intent(audio, transactions, &token, apply)
}

#[cfg(test)]
async fn resolve_and_apply_playback_intent<P, T>(
    audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState, token: &str,
    prepare: impl std::future::Future<Output = Result<P, String>>,
    apply: impl FnOnce(&mut AudioEngine, P) -> Result<T, String>,
) -> Result<T, String> {
    let prepared = prepare.await;
    // Supersession wins over an old resolver error, too. No stale UI failure.
    finish_playback_intent(audio, transactions, token, |audio| apply(audio, prepared?))
}

#[tauri::command]
fn admit_playback_intent(audio: State<AudioState>, transactions: State<ConfigTransactionState>) -> Result<String, String> {
    audio.note_transport_intervention();
    admit_or_validate_playback(&audio.0, &transactions, None)
}

#[tauri::command]
fn playback_intent_current(token: String, audio: State<AudioState>, transactions: State<ConfigTransactionState>) -> bool {
    audio.0.lock().is_ok_and(|audio| initial_playback_context(&transactions, Some(&token)).is_ok()
        && audio.validate_playback_intent(&token).is_ok())
}

#[tauri::command]
fn cancel_playback_intent(token: String, audio: State<AudioState>) -> Result<(), String> {
    audio.0.lock().map_err(|_| "Playback unavailable".to_string())?.cancel_playback_intent(&token);
    Ok(())
}

#[tauri::command]
fn play_uris(uris: Vec<String>, start_index: usize, intent: Option<String>, state: State<AudioState>, transactions: State<ConfigTransactionState>) -> Result<(), String> {
    let token = admit_or_validate_playback(&state.0, &transactions, intent)?;
    if initial_playback_context(&transactions, Some(&token))?.is_some() { return Err(initial_plex_playback::WAIT.into()); }
    finish_playback_intent(&state.0, &transactions, &token, |audio| audio.play_uris(uris, start_index))
}

async fn resolve_playback_uris(
    tracks: &[TrackMetadata],
    plex_client: &PlexClient,
    sqlite_library: Option<&SqliteLibraryState>,
    configured_roots: &[String],
) -> Result<Vec<String>, String> {
    let mut uris = Vec::with_capacity(tracks.len());
    for track in tracks {
        let sqlite_locator = match &track.media_locator {
            Some(MediaLocator::Local { uri }) if uri.starts_with("sqlite-track://") || uri.starts_with("sqlite-cue://") => Some(uri),
            _ => None,
        };
        let uri = if let Some(MediaLocator::Radio { station_id }) = &track.media_locator {
            if !radio::valid_id(station_id) { return Err("RADIO_INVALID".into()); }
            radio::stream_url(&track.uri)?
        } else if let Some(locator) = sqlite_locator {
            if !SqliteLibraryState::is_sqlite_track_locator(locator) {
                return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
            }
            let library = sqlite_library.ok_or("SQLITE_LIBRARY_UNAVAILABLE")?;
            library.resolve_playback_locator_with_roots(locator, configured_roots)?
        } else if let Some(locator) = &track.media_locator {
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

fn revalidate_sqlite_playback_bindings(
    tracks: &[TrackMetadata],
    uris: &[String],
    sqlite_library: &SqliteLibraryState,
    configured_roots: &[String],
) -> Result<(), String> {
    if tracks.len() != uris.len() { return Err("SQLITE_TRACK_RESOLUTION_UNCONFIRMED".into()); }
    for (track, uri) in tracks.iter().zip(uris) {
        let Some(MediaLocator::Local { uri: locator }) = &track.media_locator else { continue };
        if !locator.starts_with("sqlite-track://") && !locator.starts_with("sqlite-cue://") { continue; }
        if !SqliteLibraryState::is_sqlite_track_locator(locator) {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        if sqlite_library.resolve_playback_locator_with_roots(locator, configured_roots)? != *uri {
            return Err("SQLITE_TRACK_CHANGED_DURING_RESOLUTION".into());
        }
    }
    Ok(())
}

async fn resolve_device_switch_queue(
    audio: &Mutex<AudioEngine>,
    plex_client: &PlexClient,
    sqlite_library: Option<&SqliteLibraryState>,
    configured_roots: &[String],
) -> Result<ResolvedDeviceSwitchQueue, String> {
    let captured = audio
        .lock()
        .map_err(|e| format!("Falha ao acessar a fila antes da troca de saída: {}", e))?
        .capture_device_switch_queue()?;
    let uris = resolve_playback_uris(captured.tracks(), plex_client, sqlite_library, configured_roots).await?;
    if let Some(library) = sqlite_library { revalidate_sqlite_playback_bindings(captured.tracks(), &uris, library, configured_roots)?; }
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
    let resolve_clock = Instant::now();
    let sqlite_library = app.state::<SqliteLibraryState>().clone();
    let configured_roots = match app.state::<ConfigState>().0.lock() {
        Ok(config) => config.local_folders.clone(),
        Err(_) => {
            eprintln!("[Audio] Failed to access local source configuration for queue restoration.");
            if let Ok(mut audio) = audio_state.0.lock() { audio.fail_startup_queue_restore(); }
            return;
        }
    };
    let resolved = async {
        let mut rebound = queue.clone();
        if rebound.iter().any(|track| matches!(track.media_locator, Some(MediaLocator::Navidrome { .. }))) {
            music_sources::require_tracks(&rebound)?;
            // Local credential load/proxy binding only; do not scan the catalog
            // or fetch every song just to reconstruct a stopped queue.
            navidrome::navidrome_status(app.clone()).await?;
            navidrome::bind_tracks(&mut rebound)?;
        }
        let uris = resolve_playback_uris(&rebound, &plex_client, Some(&sqlite_library), &configured_roots).await?;
        revalidate_sqlite_playback_bindings(&rebound, &uris, &sqlite_library, &configured_roots)?;
        for (track, uri) in rebound.iter_mut().zip(&uris) {
            if matches!(&track.media_locator, Some(MediaLocator::Local { uri }) if SqliteLibraryState::is_sqlite_track_locator(uri)) {
                track.uri = uri.clone();
            }
        }
        let source_generations = sqlite_library.playback_source_generations(&rebound)?;
        Ok::<_, String>((rebound, uris, source_generations))
    }.await;
    local_diagnostics::record(local_diagnostics::Stage::QueueResolve, resolve_clock.elapsed(), resolved.as_ref().err().map(String::as_str));
    match resolved {
        Ok((rebound, uris, source_generations)) => match audio_state.0.lock() {
            Ok(mut audio) => {
                if let Err(error) = local_diagnostics::timed(local_diagnostics::Stage::QueueApply, || {
                    // Audio -> Navidrome lease matches explicit playback. A
                    // reconnect cannot retire the proxy during the MPD effect.
                    sqlite_library.with_playback_generation_guard(&source_generations, || {
                        revalidate_sqlite_playback_bindings(&rebound, &uris, &sqlite_library, &configured_roots)?;
                        navidrome::with_bound_tracks(rebound, |tracks| audio.restore_startup_queue_references(&queue, tracks, &uris))
                    })
                }) {
                    audio.fail_startup_queue_restore();
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

const INDEX_COVERAGE_EVENT: &str = "local-library-index-coverage";
#[tauri::command]
fn get_local_index_coverage(app: AppHandle) -> index_coverage::Snapshot {
    let folders = app.state::<ConfigState>().0.lock().map(|c| c.local_folders.clone()).unwrap_or_default();
    app.state::<index_coverage::State>().snapshot(&folders)
}
fn publish_index_coverage(app: &AppHandle, token:u64, folders:&[String], report:index_coverage::Report) {
    if let Some(snapshot) = app.state::<index_coverage::State>().publish(token, folders, report) {
        if app.emit(INDEX_COVERAGE_EVENT, snapshot).is_err() { eprintln!("[Mirror] coverage_notification=unavailable"); }
    }
}
fn inherit_index_coverage(app:&AppHandle,token:u64,folders:&[String]) {
    if let Some(snapshot)=app.state::<index_coverage::State>().inherit(token,folders){if app.emit(INDEX_COVERAGE_EVENT,snapshot).is_err(){eprintln!("[Mirror] coverage_notification=unavailable");}}
}
fn covered_index_recovery(outcome:RestorePreviousOutcome, check:Option<&index_coverage::Check>,socket:&str)->Result<RestorePreviousOutcome,String>{
    match outcome {
        RestorePreviousOutcome::SkippedForShutdown|RestorePreviousOutcome::CancelledDuringRecovery=>Ok(RestorePreviousOutcome::CancelledDuringRecovery),
        outcome=>{if let Some(check)=check{check.verify_recovered(socket)?;}Ok(outcome)}
    }
}
fn prepare_index_check(socket:&str, inventory:index_coverage::Inventory, ids:std::collections::BTreeSet<String>, mut shutdown:impl FnMut()->bool, before:Option<std::collections::HashSet<String>>) -> Result<index_coverage::Check,String> {
    if shutdown() { return Err(library_refresh::CANCELLED.into()); }
    if let Some(before) = before { return index_coverage::Check::from_catalog(inventory, index_coverage::Catalog::from_records(before)?, ids); }
    match AudioEngine::wait_for_managed_mpd_update(socket, None, MANUAL_RESCAN_POLL_INTERVAL, &mut shutdown) {
        Ok(MpdRefreshCompletion::Completed) => {},
        Err(()) => return Err(library_refresh::CANCELLED.into()),
        Ok(_) => return Err("LOCAL_INDEX_PREVIOUS_UNAVAILABLE".into()),
    }
    index_coverage::Check::prepare(inventory, socket, ids)
}

#[tauri::command]
fn get_local_watch_needs_check(watcher: State<LocalWatcherState>) -> bool { watcher.needs_check() }

fn local_watch_needs_check(app: &AppHandle, reason: &'static str) {
    match local_watcher::notice_needs_check(reason) {
        None => {
            eprintln!("[Watcher] changes_pending; external changes await an explicit Check for changes; no automatic catalog update");
            return;
        }
        Some(false) => { let _ = app.emit("local-library-watch-ready", ()); return; }
        Some(true) => {}
    }
    app.state::<LocalWatcherState>().mark_needs_check();
    eprintln!("[Watcher] needs_check={reason}; automatic monitoring stopped or unavailable; manual rescan after writes/sources are stable");
    let _ = app.emit("local-library-watch-needs-check", ());
}

fn current_local_watch_context(app: &AppHandle) -> Result<local_changes::Context, String> {
    let folders = app.state::<ConfigState>().0.lock().map_err(|_| "LOCAL_WATCH_CONFIG_UNAVAILABLE")?.local_folders.clone();
    local_changes::Context::capture(&MpdSupervisor::library_dir(), &folders)
}

fn install_saved_folders_watcher(app: &AppHandle, prepared: Option<local_watcher::PreparedWatch>, removal: bool) {
    if !removal { install_committed_local_watcher(app, prepared); return; }
    // Match manual-only startup: do not start a recursive reader merely to
    // remove a source. A reader already present keeps its survivor watches.
    let watcher=app.state::<LocalWatcherState>();
    if !watcher.running() { return; }
    let result=current_local_watch_context(app).and_then(|context|watcher.install(context,None));
    if result.is_err() && !app.state::<ConfigTransactionState>().shutdown_requested() {
        local_watch_needs_check(app,"confirmed_removal_subscription_unconfirmed");
    }
}

fn install_committed_local_watcher(app: &AppHandle, prepared: Option<local_watcher::PreparedWatch>) {
    if !music_sources::local() { return; }
    let result = current_local_watch_context(app).and_then(|context| app.state::<LocalWatcherState>().install(context, prepared));
    if result.is_err() && !app.state::<ConfigTransactionState>().shutdown_requested() {
        local_watch_needs_check(app, "confirmed_subscription_install_failed");
    } else if result.is_ok() && !app.state::<LocalWatcherState>().running() {
        start_local_watcher(app);
    }
}

#[cfg(test)]
fn run_local_content_coverage(socket:&str,plan:&local_media::mirror::ManagedRefreshPlan,inventory:index_coverage::Inventory,library:&Path,shutdown:impl Fn()->bool)->Result<index_coverage::Report,String>{
    let ids=inventory.source_ids();
    let check=prepare_index_check(socket,inventory,ids,&shutdown,None)?;
    let report=std::cell::RefCell::new(None);
    match library_refresh::refresh_inspected(socket,plan,true,&shutdown,true,||{
        let(result,_)=check.finish_at(socket,library)?;*report.borrow_mut()=Some(result);Ok(())
    })? {
        library_refresh::Outcome::Cancelled=>Err(library_refresh::CANCELLED.into()),
        library_refresh::Outcome::Completed=>report.into_inner().ok_or(index_coverage::INVALID.into()),
    }
}
fn start_local_watcher(app: &AppHandle) {
    if !music_sources::local() { return; }
    let notice_app = app.clone();
    let watcher_app = app.clone();
    let result = current_local_watch_context(app).and_then(|context| app.state::<LocalWatcherState>().start_context(context,
        // Normal external changes are informational. Source/context failures
        // remain actionable alarms. Neither path publishes a generation or
        // updates the MPD database without a user action.
        move |batch| {
            use local_watcher::Reconcile;
            let folders = match watcher_app.state::<ConfigState>().0.lock() {
                Ok(config) => config.local_folders.clone(), Err(_) => return Reconcile::Blocked,
            };
            if !batch.context.is_current(&MpdSupervisor::library_dir(), &folders) { return Reconcile::Blocked; }
            local_watcher::reconcile_manual_changes(
                &watcher_app.state::<LocalWatcherState>().own_writes, batch,
            )
        },
        move |reason| local_watch_needs_check(&notice_app, reason)));
    if result.is_err() { local_watch_needs_check(app, "start_failed"); }
}

async fn complete_startup_after_ready(app: AppHandle) {
    // An indexed library opens from its saved database. Restoring the logical
    // queue must not register a recursive watcher, enumerate source contents,
    // rebuild the mirror or schedule an MPD update/rescan.
    let started = std::time::Instant::now();
    restore_cached_queue_after_ready(app.clone()).await;
    // Startup is one-shot and only reaches this function after the existing
    // supervisor reports MPD started. EQ restoration reuses that owned route.
    equalizer_commands::restore_desired_on_startup(app.clone()).await;
    // Duration describes completion of the existing restoration attempt, not proof of playback.
    local_diagnostics::record(local_diagnostics::Stage::QueueRestoreAttempt, started.elapsed(), None);
}

// Queue edits prepare media outside locks, then enter the existing nonblocking
// lifecycle reservation. They never authorize initial/addition playback or gain.
#[tauri::command]
async fn enqueue_tracks(app: AppHandle, mut tracks: Vec<TrackMetadata>, placement: audio::QueueInsertPlacement, intent: Option<String>) -> Result<usize, String> {
    AudioEngine::validate_queue_insert_tracks(&tracks)?;
    let token = admit_or_validate_playback(&app.state::<AudioState>().0, &app.state::<ConfigTransactionState>(), intent)?;
    if initial_playback_context(&app.state::<ConfigTransactionState>(), Some(&token))?.is_some() { return Err(initial_plex_playback::WAIT.into()); }
    if music_sources::navidrome() && tracks.iter().any(|t|matches!(t.media_locator,Some(MediaLocator::Navidrome{..}))) { navidrome::navidrome_status(app.clone()).await?; navidrome::bind_tracks(&mut tracks)?; }
    let client = app.state::<PlexState>().0.lock().map_err(|_| "QUEUE_INSERT_UNAVAILABLE")?.clone();
    let sqlite_library = app.state::<SqliteLibraryState>().inner().clone();
    let configured_roots = app.state::<ConfigState>().0.lock().map_err(|_| "QUEUE_INSERT_UNAVAILABLE")?.local_folders.clone();
    let resolved = resolve_playback_uris(&tracks, &client, Some(&sqlite_library), &configured_roots).await;
    let source_generations = sqlite_library.playback_source_generations(&tracks)?;
    tauri::async_runtime::spawn_blocking(move || {
        let transactions = app.state::<ConfigTransactionState>();
        let _permit = begin_volume_transaction(&transactions)?;
        if transactions.shutdown_requested() || transactions.shutdown_started() { return Err("PLAYBACK_SUPERSEDED".into()); }
        transactions.manual_check_playback.admit()?;
        // Check current server selection before taking Audio; no lock crosses HTTP.
        { let plex = app.state::<PlexState>(); let plex = plex.0.lock().map_err(|_| "QUEUE_INSERT_UNAVAILABLE")?;
          if tracks.iter().any(|track| matches!(track.media_locator, Some(MediaLocator::Plex { .. }))
              && !plex.playback_references_match(std::slice::from_ref(track))) { return Err("QUEUE_INSERT_CONTEXT_CHANGED".into()); } }
        let uris = resolved.map_err(|error| if error.starts_with("SQLITE_") { error } else { "QUEUE_INSERT_UNAVAILABLE".into() })?;
        for (track, uri) in tracks.iter_mut().zip(&uris) {
            if matches!(&track.media_locator, Some(MediaLocator::Local { uri }) if SqliteLibraryState::is_sqlite_track_locator(uri)) {
                track.uri = uri.clone();
            }
        }
        let state = app.state::<AudioState>();
        let mut audio = state.0.lock().map_err(|_| "QUEUE_INSERT_UNAVAILABLE")?;
        audio.validate_playback_intent(&token)?;
        sqlite_library.with_playback_generation_guard(&source_generations, || {
            revalidate_sqlite_playback_bindings(&tracks, &uris, &sqlite_library, &configured_roots)?;
            navidrome::with_bound_tracks(tracks, |tracks| audio.enqueue_tracks(tracks, uris, placement))
        })
    }).await.map_err(|_| "QUEUE_INSERT_UNAVAILABLE".to_string())?
}

#[tauri::command]
async fn search_radio(app: AppHandle, search: radio::Search) -> Result<radio::Page, String> {
    let state=app.state::<radio::State>();
    tokio::time::timeout(std::time::Duration::from_secs(14), radio::search(&state, search)).await.map_err(|_| "RADIO_DIRECTORY_UNAVAILABLE".to_string())?
}
#[tauri::command]
async fn radio_filters(app: AppHandle) -> Result<radio::Filters, String> {
    let state=app.state::<radio::State>();
    tokio::time::timeout(std::time::Duration::from_secs(14), radio::filters(&state)).await.map_err(|_| "RADIO_DIRECTORY_UNAVAILABLE".to_string())?
}
#[tauri::command]
async fn radio_favorites(app: AppHandle, id: Option<String>, enabled: Option<bool>, name: Option<String>, url: Option<String>) -> Result<Vec<radio::Station>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<radio::State>(); let mut store = state.store.lock().map_err(|_| "RADIO_FAVORITES_UNAVAILABLE")?;
        match (id,enabled,name,url) {
            (Some(id),Some(enabled),None,None) => store.favorite(&id,enabled),
            (None,None,Some(name),Some(url)) => store.manual(&name,&url),
            (Some(id),None,Some(name),Some(url)) => store.edit_manual(&id,&name,&url),
            (None,None,None,None) => store.list(), _ => Err("RADIO_INVALID".into()),
        }
    }).await.map_err(|_| "RADIO_FAVORITES_UNAVAILABLE".to_string())?
}
#[tauri::command]
async fn stop_radio(app: AppHandle, id: String, intent: Option<String>) -> Result<(), String> {
    let token = admit_or_validate_playback(&app.state::<AudioState>().0, &app.state::<ConfigTransactionState>(), intent)?;
    tauri::async_runtime::spawn_blocking(move || {
        finish_playback_intent(&app.state::<AudioState>().0, &app.state::<ConfigTransactionState>(), &token, |audio| audio.stop_radio(&id))
    }).await.map_err(|_| "RADIO_PLAYBACK_UNAVAILABLE".to_string())?
}
#[tauri::command]
async fn play_radio(app: AppHandle, id: String, intent: Option<String>) -> Result<(), String> {
    let token = admit_or_validate_playback(&app.state::<AudioState>().0, &app.state::<ConfigTransactionState>(), intent)?;
    if initial_playback_context(&app.state::<ConfigTransactionState>(), Some(&token))?.is_some() { return Err(initial_plex_playback::WAIT.into()); }
    let notify = app.clone(); let clicked = id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let track = { let state = app.state::<radio::State>(); let mut store = state.store.lock().map_err(|_| "RADIO_FAVORITES_UNAVAILABLE")?; store.station(&id)?.track()? };
        let uri = track.uri.clone();
        finish_playback_intent(&app.state::<AudioState>().0, &app.state::<ConfigTransactionState>(), &token, |audio| audio.play_tracks(vec![track], vec![uri], 0))
    }).await.map_err(|_| "RADIO_PLAYBACK_UNAVAILABLE".to_string())??;
    tauri::async_runtime::spawn(async move { let state=notify.state::<radio::State>(); let _=tokio::time::timeout(std::time::Duration::from_secs(5), radio::count_click(&state,&clicked)).await; });
    Ok(())
}

#[tauri::command]
async fn listening_history(app: AppHandle, enabled: Option<bool>, clear: Option<bool>) -> Result<listening_history::HistoryFile, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<listening_history::HistoryState>();
        let mut store = state.0.lock().map_err(|_| "HISTORY_UNAVAILABLE")?;
        if enabled.is_some() || clear.unwrap_or(false) { store.configure(enabled, clear.unwrap_or(false)) } else { store.list() }
    }).await.map_err(|_| "HISTORY_UNAVAILABLE".to_string())?
}
#[tauri::command]
async fn history_plex_navigation(app: AppHandle, id: String) -> Result<plex::PlexTrackNavigation, String> {
    let locator = {
        let state = app.state::<listening_history::HistoryState>(); let mut store = state.0.lock().map_err(|_| "HISTORY_UNAVAILABLE")?;
        store.list()?.entries.into_iter().find(|entry| entry.id == id).and_then(|entry| entry.track.media_locator).ok_or("HISTORY_UNAVAILABLE")?
    };
    let client = app.state::<PlexState>().0.lock().map_err(|_| "HISTORY_UNAVAILABLE")?.clone();
    let navigation = client.resolve_track_navigation(&locator).await?;
    { let state = app.state::<listening_history::HistoryState>(); let mut store = state.0.lock().map_err(|_| "HISTORY_UNAVAILABLE")?;
      if !store.list()?.entries.iter().any(|entry| entry.id == id && entry.track.media_locator.as_ref() == Some(&locator)) { return Err("HISTORY_UNAVAILABLE".into()); } }
    let MediaLocator::Plex { server_id, .. } = &locator else { return Err("HISTORY_UNAVAILABLE".into()); };
    if !app.state::<PlexState>().0.lock().map_err(|_| "HISTORY_UNAVAILABLE")?.navigation_server_is_selected(server_id) { return Err("HISTORY_UNAVAILABLE".into()); }
    Ok(navigation)
}

#[tauri::command]
async fn get_queue_edit_snapshot(app: AppHandle) -> Result<audio::QueueEditSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let transactions = app.state::<ConfigTransactionState>(); let _permit = begin_volume_transaction(&transactions)?;
        if transactions.shutdown_requested() || transactions.shutdown_started() { return Err("PLAYBACK_SUPERSEDED".into()); }
        transactions.manual_check_playback.admit()?;
        let state = app.state::<AudioState>(); let mut audio = state.0.lock().map_err(|_| "QUEUE_EDIT_UNAVAILABLE")?;
        audio.queue_edit_snapshot()
    }).await.map_err(|_| "QUEUE_EDIT_UNAVAILABLE".to_string())?
}
#[tauri::command]
async fn edit_queue(app: AppHandle, revision: String, action: audio::QueueEditAction, intent: Option<String>) -> Result<(), String> {
    let token = admit_or_validate_playback(&app.state::<AudioState>().0, &app.state::<ConfigTransactionState>(), intent)?;
    if initial_playback_context(&app.state::<ConfigTransactionState>(), Some(&token))?.is_some() { return Err(initial_plex_playback::WAIT.into()); }
    tauri::async_runtime::spawn_blocking(move || {
        let transactions = app.state::<ConfigTransactionState>(); let _permit = begin_volume_transaction(&transactions)?;
        if transactions.shutdown_requested() || transactions.shutdown_started() { return Err("PLAYBACK_SUPERSEDED".into()); }
        transactions.manual_check_playback.admit()?;
        let state = app.state::<AudioState>(); let mut audio = state.0.lock().map_err(|_| "QUEUE_EDIT_UNAVAILABLE")?;
        audio.validate_playback_intent(&token)?; audio.edit_queue(&revision, action)
    }).await.map_err(|_| "QUEUE_EDIT_UNAVAILABLE".to_string())?
}

#[tauri::command]
async fn play_tracks(
    app: AppHandle,
    mut tracks: Vec<TrackMetadata>,
    start_index: usize,
    intent: Option<String>,
    transactions: State<'_, ConfigTransactionState>,
    audio_state: State<'_, AudioState>,
    plex_state: State<'_, PlexState>,
) -> Result<(), String> {
    let token = admit_or_validate_playback(&audio_state.0, &transactions, intent)?;
    let plex_client = plex_state
        .0
        .lock()
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
        .clone();
    let context = initial_playback_context(&transactions, Some(&token))?;
    let initial = context.is_some();
    if context.as_ref().is_some_and(|scope| !scope.accepts_tracks(&tracks, &plex_client)) { return Err(initial_plex_playback::WAIT.into()); }
    if music_sources::navidrome() && tracks.iter().any(|t|matches!(t.media_locator,Some(MediaLocator::Navidrome{..}))) { navidrome::navidrome_status(app.clone()).await?; navidrome::bind_tracks(&mut tracks)?; }
    let sqlite_library = app.state::<SqliteLibraryState>().clone();
    let configured_roots = app.state::<ConfigState>().0.lock().map_err(|_| "LOCAL_PLAYBACK_UNAVAILABLE")?.local_folders.clone();
    let resolved = resolve_playback_uris(&tracks, &plex_client, Some(&sqlite_library), &configured_roots).await;
    // Supersession/revocation wins even over a resolver error.
    { let audio = audio_state.0.lock().map_err(|_| "Playback unavailable")?;
      initial_playback_context(&transactions, Some(&token))?; audio.validate_playback_intent(&token)?; }
    let uris = resolved?;
    revalidate_sqlite_playback_bindings(&tracks, &uris, &sqlite_library, &configured_roots)?;
    let source_generations = sqlite_library.playback_source_generations(&tracks)?;
    for (track, uri) in tracks.iter_mut().zip(&uris) {
        if matches!(&track.media_locator, Some(MediaLocator::Local { uri }) if SqliteLibraryState::is_sqlite_track_locator(uri)) {
            track.uri = uri.clone();
        }
    }
    if let Some(scope) = &context {
        let plex = plex_state.0.lock().map_err(|_| "Plex unavailable")?;
        if !scope.accepts_resolved(&tracks, &uris, &plex) { return Err(initial_plex_playback::WAIT.into()); }
    }
    let effect = if initial { InitialPlaybackEffect::VerifiedMedia(uris.clone()) } else { InitialPlaybackEffect::Queue };
    finish_playback_effect(&audio_state.0, &transactions, &token, effect,
        |audio| {
            sqlite_library.with_playback_generation_guard(&source_generations, || {
                revalidate_sqlite_playback_bindings(&tracks, &uris, &sqlite_library, &configured_roots)?;
                navidrome::with_bound_tracks(tracks, |tracks| audio.play_tracks(tracks, uris, start_index))
            })
        })
}

#[tauri::command]
fn set_volume(
    volume: u32,
    sup_state: State<SupervisorState>,
    audio_state: State<AudioState>,
    transactions: State<ConfigTransactionState>,
) -> Result<(), String> {
    audio_state.note_transport_intervention();
    let admission = begin_volume_transaction(&transactions);
    if admission.is_err() {
        let supervisor = sup_state.0.lock().map_err(|_| "Controle de volume indisponível.")?;
        require_gradual_volume(supervisor.volume_backend())?;
        let backend = supervisor.shared_volume_backend();
        drop(supervisor);
        return with_initial_volume(&audio_state.0, &transactions, |audio| {
            set_volume_with_backend(backend, volume, shared_volume::set_pipewire_volume, |value| audio.set_volume(value))
        });
    }
    let _transaction = admission?;
    let supervisor = sup_state.0.lock().map_err(|_| "Controle de volume indisponível.")?;
    require_gradual_volume(supervisor.volume_backend())?;
    let backend = supervisor.shared_volume_backend();
    drop(supervisor);
    set_volume_with_backend(backend, volume, shared_volume::set_pipewire_volume, |value| {
        audio_state.0.lock().map_err(|_| "Volume do MPD indisponível.")?.set_volume(value)
    })
}

// One idempotent request shared by shortcuts, PlayerBar and both Now Playing hosts.
#[tauri::command]
fn set_shared_mute(muted:bool,token:String,sup_state:State<SupervisorState>,audio_state:State<AudioState>,transactions:State<ConfigTransactionState>)->Result<(),String>{
    audio_state.note_transport_intervention();
    let admission = begin_volume_transaction(&transactions);
    let mut supervisor = sup_state.0.lock().map_err(|_| "SHARED_MUTE_CONTEXT_CHANGED")?;
    match admission {
        Ok(_permit) => {
            let audio = audio_state.0.lock().map_err(|_| "SHARED_MUTE_CONTEXT_CHANGED")?;
            apply_shared_mute(&mut supervisor, &audio, muted, &token)
        }
        Err(_) => with_initial_volume(&audio_state.0, &transactions,
            |audio| apply_shared_mute(&mut supervisor, audio, muted, &token)),
    }
}
fn apply_shared_mute(supervisor: &mut MpdSupervisor, audio: &AudioEngine, muted: bool, token: &str) -> Result<(), String> {
    let base=supervisor.shared_mute_context().ok_or("SHARED_MUTE_CONTEXT_CHANGED")?;
    if supervisor.volume_backend()==VolumeBackend::PipeWire {
        let target=shared_volume::read_target()?;
        let context=format!("{}:{}",base,target.token());
        let actual=shared_volume::read_target_volume(&target)?;
        let action=supervisor.shared_mute.plan(token,&context,actual.value,actual.muted,true,muted)?;
        shared_volume::apply_mute_action(&target,action)
    } else {
        let value=audio.observed_software_volume()?;
        let action=supervisor.shared_mute.plan(token,&base,value,value==0,false,muted)?;
        match action {shared_mute::Action::None=>Ok(()),shared_mute::Action::Volume(value)=>audio.set_volume(value),_=>Err("SHARED_MUTE_CONTEXT_CHANGED".into())}
    }
}
fn with_initial_volume<T>(audio: &Mutex<AudioEngine>, transactions: &ConfigTransactionState,
    apply: impl FnOnce(&mut AudioEngine) -> Result<T, String>) -> Result<T, String> {
    let mut audio = audio.lock().map_err(|_| "Playback unavailable")?;
    let context = initial_playback_context(transactions, None)?.ok_or(config_save_operation::BUSY)?;
    let _lease = context.enter()?;
    let verified = initial_queue_uris(&audio, &context, false).and_then(|queue| context.accepts_queue(&queue));
    if let Err(error) = verified {
        if error == "PLAYBACK_QUEUE_UNCERTAIN" { audio.mark_queue_uncertain(); }
        return Err(error);
    }
    initial_plex_playback::check_effect()?;
    apply(&mut audio)
}

fn require_gradual_volume(backend: VolumeBackend) -> Result<(), String> {
    match backend {
        VolumeBackend::DirectEquipment => Err(direct_volume::GRADUAL.into()),
        VolumeBackend::Unavailable => Err(direct_volume::STALE.into()),
        _ => Ok(()),
    }
}

// First ordinary playback may migrate a legacy Direct choice. This writes
// policy only, under the normal lifecycle and durable config writer; never gain.
#[tauri::command]
async fn confirm_direct_volume_choice(app:AppHandle,expected_config:AppConfig,intent:String)->Result<(),String>{
    tauri::async_runtime::spawn_blocking(move||{
        let transactions=app.state::<ConfigTransactionState>();
        let _permit=begin_volume_transaction(&transactions)?;
        let mut config=app.state::<ConfigState>().0.lock().map_err(|_|direct_volume::STALE)?.clone();
        managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
        if config.first_run || config!=expected_config || AppConfig::load()?!=config{return Err(direct_volume::STALE.into());}
        app.state::<AudioState>().0.lock().map_err(|_|direct_volume::STALE)?.validate_playback_intent(&intent)?;
        config.direct_volume_choice=config.direct_context();
        if config.direct_volume_choice.is_none(){return Err(direct_volume::STALE.into());}
        if transactions.shutdown_requested(){return Err(config_save_operation::INTERRUPTED.into());}
        if let Err(error)=config.save(){app.state::<Arc<direct_playback::Gate>>().forget_choice();return Err(error.to_string());}
        *app.state::<ConfigState>().0.lock().map_err(|_|direct_volume::STALE)?=config.clone();
        app.state::<Arc<direct_playback::Gate>>().commit_choice(&config);
        if transactions.shutdown_requested(){return Err(config_save_operation::INTERRUPTED.into());}
        app.state::<AudioState>().0.lock().map_err(|_|direct_volume::STALE)?.validate_playback_intent(&intent)?;
        Ok(())
    }).await.map_err(|_|direct_volume::STALE.to_string())?
}

#[tauri::command]
async fn set_direct_mute(app: AppHandle, muted: bool, token: String) -> Result<(), String> {
    app.state::<AudioState>().note_transport_intervention();
    tauri::async_runtime::spawn_blocking(move || {
        let transactions = app.state::<ConfigTransactionState>();
        let _transaction = begin_volume_transaction(&transactions)?;
        let state = app.state::<SupervisorState>();
        let context = state.0.lock().map_err(|_| "Controle de volume indisponível.")?.volume_context();
        admit_direct_volume_playback(&context, &token, &app.state::<AudioState>().0)?;
        let supervisor = state.0.lock().map_err(|_| "Controle de volume indisponível.")?;
        if !muted {
            let current=app.state::<ConfigState>().0.lock().map_err(|_|direct_volume::STALE)?.clone();
            managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
            if AppConfig::load()?!=current{return Err(direct_volume::STALE.into());}
        }
        supervisor.set_direct_mute(muted, &token, || transactions.shutdown_requested())?;
        drop(supervisor);
        let mut config=app.state::<ConfigState>().0.lock().map_err(|_|direct_volume::STALE)?.clone();
        config.direct_muted=muted;
        if let Err(error)=config.save(){app.state::<Arc<direct_playback::Gate>>().forget_choice();return Err(error.to_string());}
        *app.state::<ConfigState>().0.lock().map_err(|_|direct_volume::STALE)?=config;
        Ok(())
    }).await.map_err(|_| "Controle de volume indisponível.".to_string())?
}

// A volume gesture must never wait behind a Save and execute minutes later.
// Reuse the existing nonblocking lifecycle reservation, including shutdown.
fn begin_volume_transaction(transactions: &ConfigTransactionState) -> Result<config_save_operation::Transaction<'_>, String> {
    transactions.operations.try_begin_background().map_err(|_| {
        transactions.operations.reject_active().err().unwrap_or_else(|| config_save_operation::BUSY.into())
    })
}

fn admit_direct_volume_playback(context: &direct_volume::Context, token: &str, audio: &Mutex<AudioEngine>) -> Result<(), String> {
    context.validate_token(token)?;
    invalidate_direct_volume_playback(audio)
}

fn invalidate_direct_volume_playback(audio: &Mutex<AudioEngine>) -> Result<(), String> {
    let mut audio = audio.lock().map_err(|_| "Estado de reprodução indisponível.")?;
    audio.invalidate_playback_intents();
    audio.note_eq_intervention();
    Ok(())
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
async fn get_queue(app: AppHandle, diagnostic_id: Option<u64>) -> Result<Vec<audio::TrackMetadata>, String> {
    let diagnostic = diagnostics::DevSpan::ipc("queue.read", diagnostic_id);
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        let mut diagnostic = diagnostic;
        let result = read_queue_from_audio(&app.state::<AudioState>().0, &diagnostic);
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta da fila indisponível.".to_string())?
}

fn read_queue_from_audio(audio: &Mutex<AudioEngine>, diagnostic: &diagnostics::DevSpan) -> Result<Vec<TrackMetadata>, String> {
    diagnostic.phase("wait.audio");
    let audio = audio.lock().map_err(|_| "Estado de reprodução indisponível.".to_string())?;
    diagnostic.phase("queue.read");
    audio.ensure_queue_coherent()?;
    Ok(audio.get_queue())
}

#[tauri::command]
async fn play_queue_index(
    app: AppHandle,
    index: usize,
    intent: Option<String>,
) -> Result<(), String> {
    let audio_state = app.state::<AudioState>();
    let transactions = app.state::<ConfigTransactionState>();
    let token = admit_or_validate_playback(&audio_state.0, &transactions, intent)?;
    let queued_track = {
        let audio = audio_state.0.lock().map_err(|_| "Playback unavailable")?;
        audio.ensure_queue_coherent()?;
        audio.get_queue().get(index).cloned().ok_or("QUEUE_INDEX_INVALID")?
    };
    let sqlite_locator = match &queued_track.media_locator {
        Some(MediaLocator::Local { uri }) if SqliteLibraryState::is_sqlite_track_locator(uri) => {
            Some(uri.clone())
        }
        _ => None,
    };
    let sqlite_uri = if let Some(locator) = &sqlite_locator {
        let library = app.state::<SqliteLibraryState>().inner().clone();
        let locator = locator.clone();
        let configured_roots = app.state::<ConfigState>().0.lock().map_err(|_| "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED")?.local_folders.clone();
        Some(tauri::async_runtime::spawn_blocking(move || {
            library.resolve_playback_locator_with_roots(&locator, &configured_roots)
        }).await.map_err(|_| "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED")??)
    } else {
        None
    };
    let is_shared = {
        let config = app.state::<ConfigState>();
        let config = config.0.lock().map_err(|e| format!("Falha ao acessar configuração durante seleção da fila: {e}"))?;
        is_shared_output(&config)
    };
    tauri::async_runtime::spawn_blocking(move || {
        let sqlite_library = app.state::<SqliteLibraryState>().inner().clone();
        let configured_roots = app.state::<ConfigState>().0.lock().map_err(|_| "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED")?.local_folders.clone();
        let transactions = app.state::<ConfigTransactionState>();
        finish_playback_intent(
            &app.state::<AudioState>().0,
            &transactions,
            &token,
            |audio| {
                if let (Some(locator), Some(uri)) = (&sqlite_locator, &sqlite_uri) {
                    // Serialize the final locator check with source removal.
                    let current = sqlite_library.resolve_playback_locator_with_roots(locator, &configured_roots)?;
                    if current != *uri { return Err("SQLITE_TRACK_CHANGED_DURING_RESOLUTION".into()); }
                    audio.play_sqlite_index(index, locator, uri, is_shared)
                } else {
                    audio.play_index(index, is_shared)
                }
            },
        )
    }).await.map_err(|_| "Playback unavailable".to_string())?
}

#[tauri::command]
fn clear_queue(intent: Option<String>, state: State<AudioState>, transactions: State<ConfigTransactionState>) -> Result<(), String> {
    let token = admit_or_validate_playback(&state.0, &transactions, intent)?;
    finish_playback_effect(&state.0, &transactions, &token, InitialPlaybackEffect::Clear, |audio| audio.clear_queue())
}

#[tauri::command]
fn set_window_title(title: String, window: Window, diagnostic_id: Option<u64>) -> Result<(), String> {
    let mut diagnostic = diagnostics::DevSpan::ipc("window.title", diagnostic_id);
    diagnostic.phase("window.native");
    let result = window.set_title(&title).map_err(|e| e.to_string());
    diagnostic.result(&result);
    result
}

#[tauri::command]
async fn list_local_directory(path: String, app: AppHandle, diagnostic_id: Option<u64>) -> Result<Vec<audio::LocalItem>, String> {
    music_sources::require_local()?;
    let diagnostic = diagnostics::DevSpan::ipc("directory.local", diagnostic_id);
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        list_local_directory_blocking(path, &app.state::<AudioState>().0, diagnostic)
    }).await.map_err(|_| "Consulta de diretório indisponível.".to_string())?
}

#[cfg(test)]
async fn list_local_directory_from_audio(path: String, audio: &Mutex<AudioEngine>) -> Result<Vec<audio::LocalItem>, String> {
    list_local_directory_blocking(path, audio, diagnostics::DevSpan::new("directory.test"))
}
fn list_local_directory_blocking(path: String, audio: &Mutex<AudioEngine>, mut diagnostic: diagnostics::DevSpan) -> Result<Vec<audio::LocalItem>, String> {
    let result = (|| {
        diagnostic.phase("wait.audio");
        let socket_path = audio.lock().map_err(|_| "Conexão da biblioteca local indisponível.".to_string())?.local_album_socket_path();
        diagnostic.phase("mpd.directory");
        AudioEngine::list_directory_at(&socket_path, &path)
    })();
    diagnostic.result(&result);
    result
}

#[tauri::command]
async fn get_local_album_tracks(album_id: String, app: AppHandle, diagnostic_id: Option<u64>) -> Result<Vec<audio::LocalAlbumDiscTracks>, String> {
    music_sources::require_local()?;
    let diagnostic = diagnostics::DevSpan::ipc("tracks.local", diagnostic_id);
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        let mut diagnostic = diagnostic;
        let result = (|| {
            diagnostic.phase("wait.audio");
            let socket_path = app.state::<AudioState>().0.lock()
                .map_err(|_| "Conexão da biblioteca local indisponível.".to_string())?.local_album_socket_path();
            diagnostic.phase("mpd.tracks");
            let transactions = app.state::<ConfigTransactionState>();
            if let Some(context) = transactions.addition_playback.current()? {
                if initial_playback_context(&transactions, None)?.is_some_and(|scope| scope.is_addition()) {
                    let _lease = context.enter()?;
                    let groups = AudioEngine::get_confirmed_album_tracks_at(&socket_path, &album_id)?
                        .ok_or(config_save_operation::BUSY)?;
                    if groups.iter().flat_map(|group| &group.tracks).any(|track| !context.accepts_local(&track.path)) {
                        return Err(config_save_operation::BUSY.into());
                    }
                    return Ok(groups);
                }
            }
            transactions.manual_check_playback.album(|files| {
                if transactions.shutdown_requested() { return Err(library_refresh::CANCELLED.into()); }
                let groups = match files {
                    Some(files) => AudioEngine::get_checked_album_tracks_at(&socket_path, &album_id, files)?,
                    None => AudioEngine::get_local_album_tracks_at(&socket_path, &album_id)?,
                };
                if transactions.shutdown_requested() { return Err(library_refresh::CANCELLED.into()); }
                groups.ok_or_else(|| "Álbum local não encontrado no catálogo atual.".to_string())
            })
        })();
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta de faixas indisponível.".to_string())?
}

// Separates descriptive Editions caches for DEV/installed/custom profiles.
// No directory creation, music/source access or startup work.
#[tauri::command]
fn get_edition_cache_namespace() -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(MpdSupervisor::sonante_config_dir().as_os_str().as_encoded_bytes()))
}

fn require_experimental_features(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<ConfigState>();
    let config = state.0.lock().map_err(|_| "EXPERIMENTAL_DISABLED")?;
    if config.experimental_features_enabled() { Ok(()) } else { Err("EXPERIMENTAL_DISABLED".into()) }
}

#[tauri::command]
async fn get_edition_profile(source: album_file_sizes::Request, app: AppHandle) -> Result<Option<album_file_sizes::Profile>, String> {
    use album_file_sizes::Request;
    require_experimental_features(&app)?;
    match source {
        Request::Plex {server_id,library_id,album_id} => {
            if !music_sources::plex() {return Err("SOURCE_DISABLED".into());}
            let client=app.state::<PlexState>().0.lock().map_err(|_|"EDITIONS_SOURCE")?.clone();
            client.edition_profile(&server_id,&library_id,&album_id).await
        },
        Request::Navidrome {connection_id,account_id,album_id} => navidrome::edition_profile(&connection_id,&account_id,&album_id).await,
        Request::Local {album_id} if album_id.starts_with("sqlite:") => {
            let state = app.state::<SqliteLibraryState>().inner().clone();
            tauri::async_runtime::spawn_blocking(move || {
                state.local_edition_profile(&album_id)
            }).await.map_err(|_| "EDITIONS_SOURCE".to_string())?
        },
        Request::Local {album_id} => tauri::async_runtime::spawn_blocking(move || {
            let (result,_)=with_local_source_context(&app,|before,recheck,context| {
                if before.observed_conflict {return Err("LOCAL_CATALOG_IDENTITY_CONFLICT".into());}
                let socket=app.state::<AudioState>().0.try_lock().map_err(|_|"EDITIONS_SOURCE")?.local_album_socket_path();
                let expected=MpdSupervisor::socket_path();
                if Path::new(&socket)!=expected.as_path(){return Err("LOCAL_CATALOG_CONTEXT_CHANGED".into());}
                let groups=AudioEngine::get_local_album_tracks_at(&socket,&album_id)?.ok_or("EDITIONS_SOURCE")?;
                let tracks=groups.into_iter().flat_map(|g|g.tracks).filter(|t|t.item_type=="file").collect::<Vec<_>>();
                let uris=tracks.iter().map(|t|t.path.clone()).collect::<Vec<_>>();
                let sizes=album_file_sizes::inspect(context.library,context.configured,&uris);
                let after=recheck()?;
                let actual=app.state::<AudioState>().0.try_lock().map_err(|_|"EDITIONS_SOURCE")?.local_album_socket_path();
                if after.observed_conflict||before.sources!=after.sources||actual!=socket||MpdSupervisor::socket_path()!=expected{return Err("LOCAL_CATALOG_CONTEXT_CHANGED".into());}
                Ok(Some(album_file_sizes::Profile::new(sizes,tracks.into_iter().map(|t|t.title))))
            })?;
            Ok(result)
        }).await.map_err(|_|"EDITIONS_SOURCE".to_string())?,
    }
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
    music_sources::require_local()?;
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
async fn get_local_file_info(
    path: String,
    config_state: State<'_, ConfigState>,
) -> Result<local_file_info::LocalFileInfo, local_file_info::InfoError> {
    use local_file_info::InfoError;
    let permit = local_file_info::InspectionPermit::acquire()?;
    let folders = config_state
        .0
        .lock()
        .map_err(|_| InfoError::Unavailable)?
        .local_folders
        .clone();
    let expected_folders = folders.clone();
    let library = MpdSupervisor::library_dir();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        tags::inspect_local_file_info(&library, &path, &folders)
    })
    .await
    .map_err(|_| InfoError::Unavailable)??;
    if config_state
        .0
        .lock()
        .map_err(|_| InfoError::Unavailable)?
        .local_folders
        != expected_folders
    {
        return Err(InfoError::Stale);
    }
    Ok(result)
}

#[tauri::command]
async fn get_local_track_tags(
    path: String,
    config_state: State<'_, ConfigState>,
) -> Result<tags::LocalTrackTags, tags::LocalTrackTagsError> {
    music_sources::require_local().map_err(|message| tags::LocalTrackTagsError {code:tags::LocalTrackTagsErrorCode::InvalidLocalPath,message,format:None,editable:false,unsupported_reason:None,committed:false})?;
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
async fn sqlite_library_tag_album_inspection(
    album_id: String,
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
) -> Result<tags::LocalAlbumTagInspection, tags::LocalTrackTagsError> {
    require_experimental_features(&app)
        .map_err(|_| tags::LocalTrackTagsError::experimental_features_disabled())?;
    let library = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        library.sqlite_tag_album_inspection(&album_id)
            .map_err(|_| tags::LocalTrackTagsError::album_catalog_failed())
    })
    .await
    .map_err(|_| tags::LocalTrackTagsError::task_failed())?
}

#[tauri::command]
async fn sqlite_library_tag_album_preflight(
    request: tags::LocalAlbumTagBatchPreflightRequest,
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
) -> Result<tags::LocalAlbumTagBatchPlan, tags::LocalAlbumTagBatchPreflightError> {
    require_experimental_features(&app)
        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::experimental_features_disabled())?;
    let library = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        library.preflight_sqlite_tag_batch(request)
    })
    .await
    .map_err(|_| tags::LocalAlbumTagBatchPreflightError::task_failed())?
}

#[tauri::command]
async fn sqlite_library_tag_album_apply(
    request: tags::LocalAlbumTagBatchPreflightRequest,
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
) -> Result<LocalAlbumTagBatchResult, tags::LocalAlbumTagBatchPreflightError> {
    require_experimental_features(&app)
        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::experimental_features_disabled())?;
    let library = state.inner().clone();
    let (write_result, catalog_reconciled, refresh_paths) = tauri::async_runtime::spawn_blocking(move || {
        library.apply_sqlite_tag_batch(request)
    })
    .await
    .map_err(|_| tags::LocalAlbumTagBatchPreflightError::task_failed())??;
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    let fallback = write_result.clone();
    let refresh_paths_fallback = refresh_paths.clone();
    let mut result = match tauri::async_runtime::spawn_blocking(move || {
        complete_local_album_tag_batch(write_result, |uri| {
            let path = refresh_paths.get(uri).ok_or(TargetedLibraryUpdateErrorCode::InvalidUri)?;
            AudioEngine::update_library_path(&socket_path, path)
        })
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            eprintln!("[SQLiteTags] MPD metadata refresh task failed: {}", crate::diagnostics::error_summary(&error));
            complete_local_album_tag_batch(fallback, |uri| {
                if refresh_paths_fallback.contains_key(uri) {
                    Err(TargetedLibraryUpdateErrorCode::RefreshTaskFailed)
                } else {
                    Err(TargetedLibraryUpdateErrorCode::InvalidUri)
                }
            })
        }
    };
    result.catalog_reconciled = Some(catalog_reconciled);
    eprintln!(
        "[SQLiteTags] album_batch_result committed={} unchanged={} failed={} not_attempted={} partial_commit={} catalog_reconciled={}",
        result.committed_count,
        result.unchanged_count,
        result.failed_count,
        result.not_attempted_count,
        result.partial_commit,
        catalog_reconciled,
    );
    Ok(result)
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
    watcher_state: State<'_, LocalWatcherState>,
) -> Result<LocalAlbumTagBatchResult, tags::LocalAlbumTagBatchPreflightError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    let own_writes = watcher_state.own_writes.clone();
    let (write_result, lease) = tauri::async_runtime::spawn_blocking(move || {
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
        let mut leases = Vec::new();
        for uri in &track_uris {
            if !local_media::is_virtual_cue_track_uri(uri) {
                if let Ok(lease) = own_writes.begin(&library_dir, &[uri.clone()]) { leases.push((uri.clone(), lease)); }
            }
        }
        let result = tags::apply_local_album_tag_batch(
            &library_dir,
            request,
            track_uris,
            &local_folders,
        )?;
        Ok::<_, tags::LocalAlbumTagBatchPreflightError>((result, leases))
    })
    .await
    .map_err(|_| tags::LocalAlbumTagBatchPreflightError::task_failed())??;

    let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
    let fallback = write_result.clone();
    let result = match tauri::async_runtime::spawn_blocking(move || {
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
    };
    if let Ok(result) = &result {
        for (uri, lease) in lease {
            if let Some(track) = result.tracks.iter().find(|track| track.write.uri == uri) {
                let revision = track.write.tags.as_ref().map(|tags| tags.revision.as_str()).unwrap_or("");
                lease.complete(track.mpd_refresh.state == LocalAlbumTagMpdRefreshState::Accepted, revision);
            }
        }
    }
    result
}

#[tauri::command]
async fn update_local_track_tags(
    path: String,
    expected_revision: String,
    patch: tags::LocalTrackTagUpdate,
    config_state: State<'_, ConfigState>,
    watcher_state: State<'_, LocalWatcherState>,
) -> Result<LocalTrackTagUpdateResult, tags::LocalTrackTagsError> {
    let local_folders = config_state
        .0
        .lock()
        .map_err(|_| tags::LocalTrackTagsError::state_unavailable())?
        .local_folders
        .clone();
    let library_dir = MpdSupervisor::library_dir();
    let refresh_path = path.clone();
    let own_writes = watcher_state.own_writes.clone();
    let (write_result, lease) = tauri::async_runtime::spawn_blocking(move || {
        let lease = own_writes.begin(&library_dir, &[path.clone()]).ok();
        let result = tags::update_local_track_tags(
            &library_dir,
            &path,
            &local_folders,
            &expected_revision,
            &patch,
        );
        (result, lease)
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
    let result = complete_local_tag_update(&refresh_path, write_result, |_| refresh_result);
    if let (Some(lease), Ok(result)) = (lease, &result) { lease.complete(result.mpd_refresh.success, &result.tags.revision); }
    result
}

// A navigation effect reads these even while Local is open. Local favorite
// existence checks can enter NAS syscalls; never run them on the UI thread.
static FAVORITES_READS: bounded_artwork_io::Reads = bounded_artwork_io::Reads::new(1);

#[tauri::command]
async fn get_favorites(diagnostic_id: Option<u64>) -> Result<Vec<FavoriteAlbum>, String> {
    run_favorites_io(&FAVORITES_READS, "favorites.read", diagnostic_id, load_favorites).await
}

async fn run_favorites_io<T: Send + 'static>(
    reads: &'static bounded_artwork_io::Reads,
    operation: &'static str,
    diagnostic_id: Option<u64>,
    load: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let mut diagnostic = diagnostics::DevSpan::ipc(operation, diagnostic_id);
    diagnostic.phase("admission.try");
    let admission = reads.try_acquire().ok_or_else(|| "favorites_busy".to_string());
    diagnostic.result(&admission);
    let permit = admission?;
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        diagnostic.phase("filesystem.favorites");
        let result = load();
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta dos favoritos indisponível.".to_string())?
}

fn load_favorites() -> Result<Vec<FavoriteAlbum>, String> {
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
    let mut albums=FavoriteAlbum::load_all(config.as_ref())?;
    albums.retain(|album| match album.source.as_str() { "local" => music_sources::local(), "plex" => music_sources::plex(), _ => true });
    Ok(albums)
}

#[tauri::command]
async fn toggle_favorite(album: FavoriteAlbum, diagnostic_id: Option<u64>) -> Result<bool, String> {
    run_favorites_io(&FAVORITES_READS, "favorites.toggle", diagnostic_id, move || {
        let config = AppConfig::load()?;
        if (album.source == "local" && !music_sources::local()) || (album.source == "plex" && !music_sources::plex()) { return Err("SOURCE_DISABLED".into()); }
        FavoriteAlbum::toggle(album, &config)
    }).await
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
    app: AppHandle,
    playlist_id: String,
    playlist_state: State<'_, PlaylistState>,
    audio_state: State<'_, AudioState>,
    plex_state: State<'_, PlexState>,
    sqlite_library: State<'_, SqliteLibraryState>,
) -> Result<Vec<PlaylistItemAvailability>, String> {
    let configured_roots = app.state::<ConfigState>().0.lock()
        .map_err(|_| "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED")?.local_folders.clone();
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
    if music_sources::navidrome() && playlist.items.iter().any(|t|matches!(t.media_locator,MediaLocator::Navidrome{..})) { let _ = navidrome::navidrome_status(app).await; }
    let mut statuses = Vec::with_capacity(playlist.items.len());
    for item in playlist.items {
        if music_sources::require_locator(&item.media_locator).is_err() { continue; }
        let status = match &item.media_locator {
            MediaLocator::Upnp { .. } | MediaLocator::Radio { .. } => local_playlist_item_availability(item.id, Err("UPNP_SESSION_EXPIRED".into())),
            MediaLocator::Navidrome { .. } => local_playlist_item_availability(item.id, navidrome::available(&item.media_locator).await),
            MediaLocator::Local { uri } => {
                let result = if SqliteLibraryState::is_sqlite_track_locator(uri) {
                    sqlite_library.resolve_playback_locator_with_roots(uri, &configured_roots).map(|_| true)
                } else {
                    audio_state
                        .0
                        .lock()
                        .map_err(|_| "O estado da biblioteca local está indisponível.".to_string())
                        .and_then(|audio| audio.local_media_exists(uri))
                };
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
                    // Resolve Local occurrences afresh for this exact locator.
                    // For SQLite CUE this is the transient range token, not
                    // the persisted opaque locator.
                    MediaLocator::Local { .. } => uri.clone(),
                    MediaLocator::Plex { .. } | MediaLocator::Upnp { .. } | MediaLocator::Radio { .. } | MediaLocator::Navidrome { .. } => String::new(),
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
    app: AppHandle,
    playlist_id: String,
    intent: Option<String>,
    transactions: State<'_, ConfigTransactionState>,
    start_item_id: Option<String>,
    shuffle: Option<bool>,
    playlist_state: State<'_, PlaylistState>,
    audio_state: State<'_, AudioState>,
    plex_state: State<'_, PlexState>,
    sqlite_library: State<'_, SqliteLibraryState>,
) -> Result<PlaylistPlaybackResult, String> {
    let configured_roots = app.state::<ConfigState>().0.lock()
        .map_err(|_| "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED")?.local_folders.clone();
    let token = admit_or_validate_playback(&audio_state.0, &transactions, intent)?;
    if shuffle == Some(true) && start_item_id.is_some() {
        return Err("Não é possível combinar embaralhamento com início em uma faixa.".into());
    }
    let mut playlist = playlist_state
        .0
        .lock()
        .map_err(|_| "O estado das playlists está indisponível.".to_string())?
        .get(&playlist_id)?;
    let plex_client = plex_state
        .0
        .lock()
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
        .clone();
    let context = initial_playback_context(&transactions, Some(&token))?;
    let initial = context.is_some();
    let reference_tracks = playlist.items.iter().map(|item| TrackMetadata {
        title: item.metadata.title.clone(), artist: item.metadata.artist.clone(), album: item.metadata.album.clone(),
        thumb: None, plex_image: None, media_locator: Some(item.media_locator.clone()), uri: String::new(), duration: item.metadata.duration
    }).collect::<Vec<_>>();
    if context.as_ref().is_some_and(|scope| !scope.accepts_tracks(&reference_tracks, &plex_client)) { return Err(initial_plex_playback::WAIT.into()); }
    if music_sources::navidrome() && playlist.items.iter().any(|t|matches!(t.media_locator,MediaLocator::Navidrome{..})) { let _ = navidrome::navidrome_status(app).await; }
    let mut resolved_uris = Vec::with_capacity(playlist.items.len());
    for item in &mut playlist.items {
        if music_sources::require_locator(&item.media_locator).is_err() { resolved_uris.push(None); continue; }
        let uri = match &item.media_locator {
            MediaLocator::Upnp { .. } | MediaLocator::Radio { .. } => None,
            MediaLocator::Navidrome { .. } => match navidrome::resolve_saved(&item.media_locator).await { Ok((uri, locator)) => { item.media_locator=locator; Some((uri,None)) }, Err(_) => None },
            MediaLocator::Local { uri } => {
                if SqliteLibraryState::is_sqlite_track_locator(uri) {
                    match sqlite_library.resolve_playback_locator_with_roots(uri, &configured_roots) {
                        Ok(path) => Some((path, None)),
                        Err(_) => {
                            eprintln!("[Playlist] SQLite item {} is unavailable or no longer current.", item.id);
                            None
                        }
                    }
                } else {
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
    { let audio = audio_state.0.lock().map_err(|_| "Playback unavailable")?;
      initial_playback_context(&transactions, Some(&token))?; audio.validate_playback_intent(&token)?; }
    let mut prepared = prepare_playlist_playback(&playlist, resolved_uris, start_item_id.as_deref())?;
    if shuffle == Some(true) && prepared.tracks.len() > 1 {
        let mut source = std::fs::File::open("/dev/urandom")
            .map_err(|error| format!("Falha ao obter aleatoriedade para a playlist: {error}"))?;
        shuffle_prepared_playlist_playback(&mut prepared, |upper| random_playlist_index(upper, &mut source))?;
    }
    if let Some(scope) = &context {
        let plex = plex_state.0.lock().map_err(|_| "Plex unavailable")?;
        if !scope.accepts_resolved(&prepared.tracks, &prepared.playback_uris, &plex) { return Err(initial_plex_playback::WAIT.into()); }
    }
    let effect = if initial { InitialPlaybackEffect::VerifiedMedia(prepared.playback_uris.clone()) } else { InitialPlaybackEffect::Queue };
    let source_generations = sqlite_library.playback_source_generations(&prepared.tracks)?;
    finish_playback_effect(&audio_state.0, &transactions, &token, effect,
        |audio| {
            sqlite_library.with_playback_generation_guard(&source_generations, || {
                revalidate_sqlite_playback_bindings(&prepared.tracks, &prepared.playback_uris, &sqlite_library, &configured_roots)?;
                navidrome::with_bound_tracks(prepared.tracks, |tracks| audio.play_tracks(tracks, prepared.playback_uris, prepared.start_index))
            })
        })?;
    Ok(PlaylistPlaybackResult {
        skipped_count: prepared.skipped_count,
    })
}

#[tauri::command]
async fn get_local_cover(path: String, app: AppHandle, diagnostic_id: Option<u64>) -> Result<Option<String>, String> {
    music_sources::require_local()?;
    let mut diagnostic = diagnostics::DevSpan::ipc("cover.local", diagnostic_id);
    diagnostic.phase("admission.try");
    let admission = bounded_artwork_io::COVER_READS.acquire();
    diagnostic.result(&admission);
    let permit = admission?;
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        diagnostic.phase("wait.config");
        let result = with_local_cover_context(&app, |before, recheck, context| {
            if before.observed_conflict { return Err("LOCAL_COVER_IDENTITY_CONFLICT".into()); }
            let socket_path = MpdSupervisor::socket_path().to_string_lossy().into_owned();
            let verify = || -> Result<(), String> {
                // Retain the real config/Lease/transaction checks at each I/O
                // boundary. SourceCoverTarget separately revalidates source B
                // and held physical FDs. Full original inventories are checked
                // once more below, before any artwork result is published.
                (context.validate_config)()?;
                if MpdSupervisor::socket_path().to_string_lossy().as_ref() != socket_path.as_str() {
                    return Err("LOCAL_COVER_CONTEXT_CHANGED".into());
                }
                Ok(())
            };
            verify()?;
            diagnostic.phase("filesystem");
            let cover = local_media::with_source_cover_target(
                context.owner, context.cover_owner, context.coordinator, context.library, context.configured, &path,
                |target| AudioEngine::get_local_cover_from_source(&socket_path, &path, target, &verify),
            )?;
            verify()?;
            let after = recheck()?;
            if after.observed_conflict { return Err("LOCAL_COVER_IDENTITY_CONFLICT".into()); }
            if before.sources != after.sources { return Err("LOCAL_COVER_CONTEXT_CHANGED".into()); }
            verify()?;
            Ok(cover)
        }).map(|(cover, _)| cover);
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta de capa local indisponível.".to_string())?
}

#[tauri::command]
async fn get_local_covers(paths: Vec<String>, app: AppHandle) -> Result<Vec<Result<Option<String>,String>>,String> {
    music_sources::require_local()?;
    if paths.is_empty() || paths.len()>2 || paths.iter().any(|path|path.len()>4096) {
        return Err("LOCAL_COVER_BATCH_BOUND".into());
    }
    let permit=bounded_artwork_io::COVER_READS.acquire()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit=permit;
        with_local_cover_context(&app, |before,recheck,context| {
            if before.observed_conflict {return Err("LOCAL_COVER_IDENTITY_CONFLICT".into());}
            let socket=MpdSupervisor::socket_path().to_string_lossy().into_owned();
            let verify=||->Result<(),String> {
                (context.validate_config)()?;
                if MpdSupervisor::socket_path().to_string_lossy().as_ref()!=socket.as_str() {
                    return Err("LOCAL_COVER_CONTEXT_CHANGED".into());
                }
                Ok(())
            };
            verify()?;
            let covers=context.inventory.read_cover_pair_in_context(context.owner,context.cover_owner,context.coordinator,
                context.library,context.configured,&paths,|path,target|
                    AudioEngine::get_local_cover_from_source(&socket,path,target,&verify))?;
            verify()?;
            let after=recheck()?;
            if after.observed_conflict {return Err("LOCAL_COVER_IDENTITY_CONFLICT".into());}
            if before.sources!=after.sources {return Err("LOCAL_COVER_CONTEXT_CHANGED".into());}
            verify()?;
            Ok(covers)
        }).map(|(covers,_)|covers)
    }).await.map_err(|_|"Consulta de capas locais indisponível.".to_string())?
}

#[tauri::command]
async fn get_online_album_metadata_details(
    mbid: String,
) -> Result<online_metadata::ReleaseDetails, online_metadata::SearchError> {
    online_metadata::lookup(mbid).await
}

#[tauri::command]
async fn search_online_album_metadata(
    request: online_metadata::SearchRequest,
) -> Result<online_metadata::SearchResult, online_metadata::SearchError> {
    online_metadata::search(request).await
}

#[tauri::command]
async fn get_online_album_cover(
    request: online_artwork::OnlineAlbumCoverRequest,
    config_state: State<'_, ConfigState>,
    cache_only: Option<bool>,
) -> Result<Option<String>, String> {
    let enabled = config_state
        .0
        .lock()
        .map_err(|_| "Configuração indisponível.".to_string())?
        .online_artwork_enabled;
    if cache_only == Some(true) {
        return online_artwork::cached_cover(request, enabled).await;
    }
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
async fn check_local_library(app: AppHandle, source_id: Option<String>, expected_folders: Option<Vec<String>>) -> Result<bool, String> {
    music_sources::require_local()?;
    tauri::async_runtime::spawn_blocking(move || {
        run_managed_mirror_refresh(&app, ManagedMirrorRefreshMode::Check, source_id.as_deref(), expected_folders.as_deref())
    })
    .await
    .map_err(|error| format!("Falha na tarefa de verificação da biblioteca: {error}"))?
}

#[tauri::command]
async fn rescan_library(app: AppHandle) -> Result<(), String> {
    music_sources::require_local()?;
    tauri::async_runtime::spawn_blocking(move || run_managed_manual_rescan(&app))
        .await
        .map_err(|error| format!("Falha na tarefa de rescan da biblioteca: {error}"))?
}

#[tauri::command]
async fn get_config(app: AppHandle) -> Result<AppConfig, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<profile_lease::State>().lease()?.validate()?;
        read_config_after_startup_reconciliation(
            &app.state::<ConfigState>(),
            &app.state::<ConfigTransactionState>(),
            &MpdSupervisor::sonante_config_dir(),
        )
    })
    .await
    .map_err(|error| format!("Falha na tarefa de leitura da configuração: {error}"))?
}

// Guards stay on this worker's stack through the callback and final validation.
// This context binds a consistent profile/config/ledger/mirror. It never grants
// physical-file or playback access; each caller has its own finite read scope.
// Constructed only by the real guard-owning helper; never serialized.
struct LocalSourceReadContext<'a> {
    owner: &'a profile_lease::Lease,
    cover_owner: local_media::CoverReadOwner<'a>,
    coordinator: &'a config_save_operation::Coordinator,
    library: &'a Path,
    configured: &'a [String],
    validate_config: &'a dyn Fn() -> Result<(), String>,
    inventory: &'a source_availability::PreliminaryInventory,
}
// Read-only variant for the guard-owning local-source context only.
// Existing startup/Save callers retain the original hardening loader.
fn read_local_source_config_with_startup_guard(
    state: &ConfigState,
    transactions: &ConfigTransactionState,
    config_dir: &Path,
    startup_owned: &std::sync::MutexGuard<'_, bool>,
) -> Result<AppConfig, String> {
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
        if **startup_owned {
            managed_save_intent::check_active_startup_reconciliation(config_dir)?;
        } else {
            managed_save_intent::check_startup(config_dir)?;
        }
        // The caller pins, parses and compares persisted bytes immediately
        // below. Do not use the hardening loader here: chmod changes the
        // profile ctime and invalidates the retained D03 snapshot.
    }
    Ok(state
        .0
        .lock()
        .map_err(|_| "Falha ao acessar a configuração atual.".to_string())?
        .clone())
}

fn with_local_source_context<T>(
    app: &AppHandle,
    read: impl FnOnce(
        &source_availability::PreliminaryObservation,
        &dyn Fn() -> Result<source_availability::PreliminaryObservation, String>,
        &LocalSourceReadContext<'_>,
    ) -> Result<T, String>,
) -> Result<(T, source_availability::PreliminaryObservation), String> {
    with_local_source_context_mode(app, false, read)
}

fn with_local_cover_context<T>(
    app: &AppHandle,
    read: impl FnOnce(
        &source_availability::PreliminaryObservation,
        &dyn Fn() -> Result<source_availability::PreliminaryObservation, String>,
        &LocalSourceReadContext<'_>,
    ) -> Result<T, String>,
) -> Result<(T, source_availability::PreliminaryObservation), String> {
    with_local_source_context_mode(app, true, read)
}

fn with_local_source_context_mode<T>(
    app: &AppHandle,
    allow_check_covers: bool,
    read: impl FnOnce(
        &source_availability::PreliminaryObservation,
        &dyn Fn() -> Result<source_availability::PreliminaryObservation, String>,
        &LocalSourceReadContext<'_>,
    ) -> Result<T, String>,
) -> Result<(T, source_availability::PreliminaryObservation), String> {
    music_sources::require_local()?;
    const BUSY: &str = "SOURCE_AVAILABILITY_BUSY";
    const BLOCKED: &str = "SOURCE_AVAILABILITY_BLOCKED";
    const CHANGED: &str = "SOURCE_AVAILABILITY_CHANGED";
    const UNCONFIRMED: &str = "SOURCE_AVAILABILITY_UNCONFIRMED";
    let owner = app.state::<profile_lease::State>().lease().map_err(|_| BLOCKED)?;
    owner.validate().map_err(|_| BLOCKED)?;
    let transactions = app.state::<ConfigTransactionState>();
    // Cover readers hold the actual Check owner's read fence through final validation.
    // Other readers retain startup + exclusive transaction admission unchanged.
    let check_lease = if allow_check_covers { Some(transactions.manual_check_playback.control()?) } else { None };
    let in_check = check_lease.as_ref().is_some_and(|lease| lease.is_check());
    if in_check && !check_lease.as_ref().is_some_and(|lease| lease.cover_ready()) { return Err(BUSY.into()); }
    let startup_owned = if in_check { None } else { Some(transactions.startup_config_read_gate.try_lock().map_err(|error| {
        match error {
            std::sync::TryLockError::WouldBlock => BUSY,
            std::sync::TryLockError::Poisoned(_) => BLOCKED,
        }
    })?) };
    let transaction = if in_check { None } else { Some(transactions.operations.try_begin_background().map_err(|error| {
        match error {
            config_save_operation::BackgroundAdmission::Busy => BUSY,
            config_save_operation::BackgroundAdmission::Blocked => BLOCKED,
        }
    })?) };
    let shutdown = || transactions.shutdown_requested() || transactions.shutdown_started()
        || transactions.operations.shutdown_requested();
    if shutdown() { return Err(BLOCKED.into()); }
    let config_dir = MpdSupervisor::sonante_config_dir();
    let library_dir = MpdSupervisor::library_dir();
    let profile = recovery_fs::Dir::open(&config_dir).map_err(|_| BLOCKED)?;
    let library = recovery_fs::Dir::open(&library_dir).map_err(|_| UNCONFIRMED)?;
    let validate_context = || -> Result<(), String> {
        if shutdown() || (in_check && !check_lease.as_ref().is_some_and(|lease| lease.cover_ready())) { return Err(BLOCKED.into()); }
        owner.validate().map_err(|_| CHANGED)?;
        profile.validate().map_err(|_| CHANGED)?;
        library.validate().map_err(|_| CHANGED)?;
        if profile.id() != owner.root().id()
            || MpdSupervisor::sonante_config_dir() != config_dir
            || MpdSupervisor::library_dir() != library_dir
            || library_dir != config_dir.join("library") {
            return Err(CHANGED.into());
        }
        Ok(())
    };
    validate_context()?;
    let state = app.state::<ConfigState>();
    let read_config = || -> Result<AppConfig,String> {
        if let Some(startup) = &startup_owned {
            return read_local_source_config_with_startup_guard(&state, &transactions, &config_dir, startup).map_err(|_|BLOCKED.into());
        }
        if !in_check || !check_lease.as_ref().is_some_and(|lease| lease.cover_ready())
            || transactions.operations.snapshot()?.is_some_and(|op| op.phase == config_save_operation::Phase::Uncertain) {
            return Err(BLOCKED.into());
        }
        Ok(state.0.lock().map_err(|_|BLOCKED)?.clone())
    };
    let config = read_config()?;
    // Pin bounded persisted bytes and identity as well as the live config.
    // No raw config, paths, errors, or credentials enter the response.
    let config_id = profile.file_identity("config.json").map_err(|_| CHANGED)?;
    let config_bytes = profile.read("config.json", 16 * 1024 * 1024, &shutdown)
        .map_err(|_| UNCONFIRMED)?.ok_or(UNCONFIRMED)?;
    let mut persisted: AppConfig = serde_json::from_slice(&config_bytes).map_err(|_| UNCONFIRMED)?;
    persisted.apply_legacy_migrations();
    if persisted != config || profile.file_identity("config.json").map_err(|_| CHANGED)? != config_id {
        return Err(CHANGED.into());
    }
    validate_context()?;
    // The callback borrows only finite observations and a readonly recheck.
    // It cannot retain our guards or turn a path into a product capability.
    let validate_config = || -> Result<(), String> {
        let current = read_config()?;
        if current != config
            || profile.file_identity("config.json").map_err(|_| CHANGED)? != config_id
            || profile.read("config.json", 16 * 1024 * 1024, &shutdown).map_err(|_| CHANGED)?.as_deref()
                != Some(config_bytes.as_slice()) {
            return Err(CHANGED.into());
        }
        validate_context()
    };
    let value=source_availability::with_cached_read_inventory(&transactions.source_mirror_cache,&library_dir,&config.local_folders,|before,read_origins,inventory| {
        let recheck=||->Result<source_availability::PreliminaryObservation,String> {
            validate_config()?;
            let current=read_origins()?;
            validate_config()?;
            Ok(current)
        };
        let cover_owner = if let Some(transaction) = &transaction { local_media::CoverReadOwner::Transaction(transaction) }
            else { local_media::CoverReadOwner::Check(check_lease.as_ref().ok_or(BLOCKED)?) };
        let context=LocalSourceReadContext {owner:&owner,cover_owner,
            coordinator:transactions.operations.as_ref(),library:&library_dir,configured:&config.local_folders,
            validate_config:&validate_config,inventory};
        let value=read(before,&recheck,&context)?;
        validate_config()?;
        Ok(value)
    })?;
    // This runs AFTER the read scope's final full original topology check.
    // The ordinary startup/transaction guards or actual Check read fence remain
    // owned through this final config/Lease validation.
    validate_config()?;
    Ok(value)
}

// Informational only: the response may report conflicts, never product admission.
// Abandoning IPC does not cancel the worker or release its retained guards.
#[tauri::command]
async fn get_local_source_availability(
    app: AppHandle,
) -> Result<source_availability::PreliminaryObservation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        with_local_source_context(&app, |_, _, _| Ok(())).map(|(_, observation)| observation)
    }).await.map_err(|_| "SOURCE_AVAILABILITY_UNCONFIRMED".to_string())?
}

// These commands are available before config/bootstrap. Analysis is read-only,
// and the State rejects calls after a normal admitted startup.
#[tauri::command]
async fn analyze_library_recovery(app: AppHandle, request_id: String) -> Result<library_recovery::Observation, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<library_recovery::State>().analyze(
            &MpdSupervisor::sonante_config_dir(), &request_id,
            app.state::<profile_lease::State>().lease().ok(),
            || app.state::<ConfigTransactionState>().shutdown_requested(),
        )
    }).await.map_err(|_| library_recovery::UNAVAILABLE.to_string())?
}

#[tauri::command]
fn cancel_library_recovery_analysis(state: State<library_recovery::State>, request_id: String) {
    state.cancel(&request_id);
}

#[tauri::command]
async fn export_library_recovery_report(app: AppHandle, analysis_id: u64) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<library_recovery::State>();
        let shutdown = || app.state::<ConfigTransactionState>().shutdown_requested();
        state.report(analysis_id, shutdown)?;
        // No state/transaction/audio mutex is held while the native dialog runs.
        let Some(path) = rfd::FileDialog::new().set_file_name("sonante-library-diagnostic.json")
            .add_filter("JSON", &["json"]).save_file() else { return Ok(false); };
        let report = state.report(analysis_id, shutdown)?;
        library_recovery::save_report(&path, report.as_bytes())?;
        Ok(true)
    }).await.map_err(|_| library_recovery::UNAVAILABLE.to_string())?
}

#[tauri::command]
fn get_library_recovery_attempt_diagnostic(app:AppHandle,analysis_key:String)->library_recovery::AttemptDiagnosticDto {
    // ConfigTransactionState::shutdown_requested is an atomic lifecycle read.
    // No Coordinator guard is held across State request->last acquisition.
    let Some(transactions)=app.try_state::<ConfigTransactionState>() else {
        return library_recovery::attempt_diagnostic_response(None,analysis_key,false);
    };
    let shutdown=transactions.shutdown_requested();drop(transactions);
    let state=app.try_state::<library_recovery::State>();
    library_recovery::attempt_diagnostic_response(state.as_deref(),analysis_key,shutdown)
}

#[tauri::command]
async fn execute_library_recovery(app:AppHandle,analysis_id:u64,plan_id:String,progress:Option<tauri::ipc::JavaScriptChannelId>,webview:tauri::Webview)->Result<recovery_set::ResultDto,String>{
    let owner=app.state::<profile_lease::State>().lease()?;owner.validate()?;
    let admission=app.state::<library_recovery::State>().admission(analysis_id,&plan_id)?;
    let(_,operation)=app.state::<ConfigTransactionState>().operations.reserve_recovery(admission)?;
    let progress=progress.map(|id|id.channel_on::<_,library_recovery::Progress>(webview));
    tauri::async_runtime::spawn_blocking(move||{
        let result=app.state::<library_recovery::State>().execute_with_progress(&owner,analysis_id,&plan_id,||operation.shutdown_requested(),&|stage|{if let Some(channel)=&progress{let _=channel.send(stage);}});
        drop(owner);
        operation.finish(result.as_ref().map(|_|()).map_err(|e|e.clone()),result.is_err());result
    }).await.map_err(|_|library_recovery::UNAVAILABLE.to_string())?
}
#[tauri::command]
fn reopen_library_after_recovery(app:AppHandle,plan_id:String)->Result<(),String>{
    app.state::<profile_lease::State>().lease()?.validate()?;
    if app.state::<ConfigTransactionState>().shutdown_requested(){return Err(library_recovery::UNAVAILABLE.into())}
    app.state::<library_recovery::State>().allow_reopen(&plan_id,&MpdSupervisor::sonante_config_dir())?;
    app.state::<profile_lease::State>().request_reopen();start_coordinated_shutdown(&app,0);Ok(())
}

fn read_config_after_startup_reconciliation(
    state: &ConfigState,
    transactions: &ConfigTransactionState,
    config_dir: &Path,
) -> Result<AppConfig, String> {
    if let Some(config) = first_run_managed::PendingPreparation::config(config_dir)? { return Ok(config); }
    let startup_owned = transactions
        .startup_config_read_gate
        .lock()
        .map_err(|_| "Controle de leitura da configuração indisponível.".to_string())?;
    read_config_with_startup_guard(state, transactions, config_dir, &startup_owned)
}

// The caller retains the actual startup gate; do not relock it here.
// Old readers keep their original locking wrapper and validation semantics.
fn read_config_with_startup_guard(
    state: &ConfigState,
    transactions: &ConfigTransactionState,
    config_dir: &Path,
    startup_owned: &std::sync::MutexGuard<'_, bool>,
) -> Result<AppConfig, String> {
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
        if **startup_owned {
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
async fn get_local_import_artwork(
    operation_id: String,
    reference: String,
    state: State<'_, ConfigTransactionState>,
) -> Result<Option<String>, local_import_preview::PreviewError> {
    // One cover reader and one page reader, independently throttled: slow
    // filesystem artwork never owns the MPD page slot. Both leases revoke together.
    // No arbitrary socket, physical filename, or general catalog admission.
    let lease = state.operations.reserve_preview_artwork(&operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        local_import_preview::read_artwork(&operation_id, &reference, &lease)
    }).await.map_err(|_| local_import_preview::PreviewError::ReadFailed)?
}

#[tauri::command]
fn get_audio_devices() -> Vec<AudioDevice> {
    if null_output_test_mode() {
        Vec::new()
    } else {
        list_audio_devices()
    }
}

#[tauri::command]
fn is_null_output_test_mode() -> bool {
    null_output_test_mode()
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
    if null_output_test_mode() {
        return Err("NULL_OUTPUT_TEST_HARDWARE_DISABLED".into());
    }
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

// Resume on every confirmed exit, including early returns after rollback. This
// guard drops before the Save transaction/queue reservation, after inner locks.
// Unconfirmed/uncertain exits deliberately leave the capture suspended.
struct AnalyzerSwitchResume<F: FnMut(&analyzer::AnalyzerOwner) -> Result<(), String>> {
    capture: Option<analyzer::AnalyzerOwner>,
    confirmed: bool,
    resume: F,
}

impl<F: FnMut(&analyzer::AnalyzerOwner) -> Result<(), String>> AnalyzerSwitchResume<F> {
    fn new(resume: F) -> Self {
        Self {
            capture: None,
            confirmed: false,
            resume,
        }
    }

    fn after_rollback(&mut self, result: Result<(), String>) -> Result<(), String> {
        self.confirmed = result.is_ok();
        result
    }
}

impl<F: FnMut(&analyzer::AnalyzerOwner) -> Result<(), String>> Drop for AnalyzerSwitchResume<F> {
    fn drop(&mut self) {
        if self.confirmed {
            if let Some(capture) = &self.capture {
                if let Err(error) = (self.resume)(capture) {
                    eprintln!(
                        "[Analyzer] Output/configuration confirmed, but capture could not resume: {}",
                        crate::diagnostics::error_summary(&error)
                    );
                }
            }
        }
    }
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
    let started = Instant::now();
    let result = save_config_inner(new_config, queue_clear_authorization, app, config_state,
        transaction_state, plex_state, sup_state, audio_state, analyzer_state).await;
    local_diagnostics::observe(local_diagnostics::Stage::PreferencesSave, started.elapsed(), &result);
    result
}

async fn save_config_inner(
    mut new_config: AppConfig,
    queue_clear_authorization: Option<QueueClearAuthorization>,
    app: AppHandle,
    config_state: State<'_, ConfigState>,
    transaction_state: State<'_, ConfigTransactionState>,
    plex_state: State<'_, PlexState>,
    sup_state: State<'_, SupervisorState>,
    audio_state: State<'_, AudioState>,
    analyzer_state: State<'_, AnalyzerState>,
) -> Result<config_save_operation::SaveResponse, String> {
    // Refuse unowned/changed profiles before intent invalidation or any Save effects.
    // Keep the actual lease alive across awaits and the synchronous Save branches.
    let profile_owner = app.state::<profile_lease::State>().lease()?;
    profile_owner.validate()?;
    audio_state.0.lock().map_err(|_|"Playback unavailable")?.note_eq_intervention();
    transaction_state.operations.reject_active()?;
    audio_state.0.lock().map_err(|_| "Playback unavailable".to_string())?.invalidate_playback_intents();
    managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
    AppConfig::load()?;
    let observed_cfg = config_state
        .0
        .lock()
        .map_err(|e| format!("Falha ao acessar a configuração atual: {}", e))?
        .clone();

    new_config.validate_direct_choice()?;
    new_config.direct_muted=observed_cfg.direct_muted;
    validate_config_save_scope(&observed_cfg, &new_config)?;

    if observed_cfg.first_run || local_folders_changed(&observed_cfg, &new_config) {
        // Admission precedes operation reservation, staging, MPD commands and commit.
        // Canonicalization/read_dir stay off the async command thread.
        let admission_current = observed_cfg.clone();
        let admission_next = new_config.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if admission_current.first_run {
                local_diagnostics::wizard_step(local_diagnostics::Stage::WizardSourceAdmission, ||
                    validate_local_folder_admission(&admission_current, &admission_next))
            } else { validate_local_folder_admission(&admission_current, &admission_next) }
        })
        .await
        .map_err(|_| "Local folder admission unavailable".to_string())??;
    }

    if is_finishing_first_run(&observed_cfg, &new_config) {
        if queue_clear_authorization.is_some() {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
        }
        if app.try_state::<SqliteLibraryState>().is_some() {
            let worker_app = app.clone();
            let (_, operation) = transaction_state.operations.reserve()?;
            let prepared = tauri::async_runtime::spawn_blocking(move || {
                let uncertain = Cell::new(false);
                let result = local_diagnostics::wizard_step(
                    local_diagnostics::Stage::WizardPreparation,
                    || {
                        let (result, pending) = prepare_initial_sqlite_preferences(
                            &worker_app,
                            &observed_cfg,
                            &new_config,
                            &operation,
                        );
                        uncertain.set(pending);
                        result
                    },
                );
                ((result, uncertain.get()), operation)
            }).await.map_err(|_| "FIRST_RUN_PREPARATION_INCOMPLETE".to_string())?;
            let ((result, uncertain), operation) = prepared;
            operation.finish(result.clone(), uncertain);
            result?;
            return Ok(config_save_operation::SaveResponse::completed(audio::DeviceSwitchRestoreOutcome::Complete));
        }
        let worker_app = app.clone();
        let (_, operation) = transaction_state.operations.reserve()?;
        let prepared = tauri::async_runtime::spawn_blocking(move || {
            let result = local_diagnostics::wizard_step(local_diagnostics::Stage::WizardPreparation, ||
                prepare_initial_preferences(&worker_app, &observed_cfg, &new_config, &operation));
            (result, operation)
        }).await.map_err(|_| "FIRST_RUN_PREPARATION_INCOMPLETE".to_string())?;
        let (result, operation) = prepared;
        let watch = match result {
            Ok(watch) => watch,
            Err(error) => {
                let pending = managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir()).is_err();
                operation.finish(Err(error.clone()), pending);
                return Err(if pending { format!("FIRST_RUN_PREPARATION_INCOMPLETE: {error}") } else { error });
            }
        };
        spawn_initial_catalog(app.clone(), operation, false, Some(watch), false);
        return Ok(config_save_operation::SaveResponse::completed(audio::DeviceSwitchRestoreOutcome::Complete));
    }

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

    let mut _transaction_guard = transaction_state.begin()?;
    {
        let mut audio=audio_state.0.lock().map_err(|_| "Playback unavailable".to_string())?;
        if observed_cfg.sources.unwrap_or_default() != new_config.sources.unwrap_or_default() {
            audio.ensure_queue_persisted()?;
            music_sources::validate_disable(&observed_cfg, &new_config, &audio.get_queue())?;
        }
        audio.invalidate_playback_intents();
    }
    // Admission covers URI resolution as well as effects and rollback.
    let observed_finishing_first_run = is_finishing_first_run(&observed_cfg, &new_config);
    let output_change_expected = !observed_finishing_first_run
        && (observed_cfg.audio_output_type != new_config.audio_output_type
            || observed_cfg.alsa_device != new_config.alsa_device
            || observed_cfg.dop_enabled != new_config.dop_enabled
            || observed_cfg.audio_buffer_size_kb != new_config.audio_buffer_size_kb
            || observed_cfg.replay_gain != new_config.replay_gain);
    // Only explicit device/mode Save may resume; buffer/ReplayGain/DoP-only,
    // folders and startup retain their existing contract.
    let route_save_resume = route_save_resume_requested(&observed_cfg,&new_config);
    let route_save_watch = audio_state.0.lock().map_err(|_|"Playback unavailable")?.watch_eq_transition();
    let switch_queue = if output_change_expected {
        let plex_client = plex_state
            .0
            .lock()
            .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
            .clone();
        let sqlite_library = app.state::<SqliteLibraryState>().clone();
        Some(resolve_device_switch_queue(&audio_state.0, &plex_client, Some(&sqlite_library), &observed_cfg.local_folders).await?)
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

    // Capture the selected physical identity before pausing playback. This is
    // read-only procfs/sysfs validation; it does not compete with MPD for ALSA.
    let direct_device_identity = if route_save_resume
        && new_config.audio_output_type == "alsa"
        && !is_shared_output(&new_config)
    {
        Some(alsa_observation::capture_device_identity(&new_config.alsa_device)
            .map_err(|reason| alsa_transition_error("identity_preflight", reason))?)
    } else { None };
    let release_shared_before_direct = direct_device_identity.is_some()
        && is_shared_to_direct_transition(&current_cfg, &new_config)
        && !current_cfg.first_run;

    if output_change_expected {
        _transaction_guard.begin_audio_transition(
            configured_audio_route(&new_config, crate::audio_route::AudioProcessing::Off)?,
            config_save_operation::AudioTransitionKind::Configuration,
        )?;
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
    let mut restore_outcome = audio::DeviceSwitchRestoreOutcome::Complete;
    let mut playback_snapshot = None;
    let mut provisional_direct_choice: Option<ProvisionalDirectChoice> = None;
    let source_muted = if route_save_resume {
        if is_shared_output(&current_cfg) {
            let backend=sup_state.0.lock().map_err(|_|"OUTPUT_SAVE_VOLUME_UNAVAILABLE")?.shared_volume_backend();
            if backend==Some(shared_volume::SharedVolumeBackend::PipeWire) {
                let value=shared_volume::read_pipewire_volume()?; value.muted||value.value==0
            } else {
                audio_state.0.lock().map_err(|_|"OUTPUT_SAVE_VOLUME_UNAVAILABLE")?.observed_software_volume()?==0
            }
        } else { current_cfg.direct_muted }
    } else { false };
    // Carry intentional silence into a new Direct process before it can start.
    if route_save_resume && new_config.direct_context().is_some() && source_muted {
        new_config.direct_muted=true;
    }
    let mut resume_analyzer = AnalyzerSwitchResume::new(|capture| {
        let Some(_restart_permit) = transaction_state.try_begin_restart() else {
            return Ok(());
        };
        let mut analyzer = analyzer_state
            .0
            .lock()
            .map_err(|e| format!("Falha ao acessar o analyzer após troca de saída: {e}"))?;
        analyzer
            .resume_after_output_switch(capture, &app, || transaction_state.shutdown_requested())
    });

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
        let preparation = {
            let mut audio=audio_state.0.lock().map_err(|e|format!("Falha ao preparar troca de saída: {e}"))?;
            let queue=switch_queue.as_ref().ok_or("Fila da troca de saída indisponível.")?;
            // All output Saves retain an existing logical cursor without starting MPD.
            audio.prepare_reserved_route_save(queue)
        };
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
        let (capture, suspend_result) = {
            let mut analyzer = analyzer_state
                .0
                .lock()
                .map_err(|e| format!("Falha ao acessar o analyzer antes da troca: {}", e))?;
            analyzer.suspend_for_output_switch(&app)
        };
        resume_analyzer.capture = capture;
        if let Err(error) = suspend_result {
            eprintln!(
                "[Analyzer] Failed to suspend analyzer before output switch: {}",
                crate::diagnostics::error_summary(&error)
            );
        }
        // Shared DSP is stopped before Direct/new Shared can open. Returning
        // to Shared never silently reactivates a saved preset.
        // From this point DSP/output resources may change. If any subsequent
        // rollback cannot prove restoration, the shared route snapshot remains
        // explicitly uncertain instead of inferring success from saved config.
        _transaction_guard.mark_audio_transition_uncertain();
        if let Err(error) = equalizer_commands::stop_for_output_switch(&app) {
            return Err(error_with_rollback(error, resume_analyzer.after_rollback(restore_prepared_playback(playback_snapshot, switch_queue.as_ref(), &audio_state))));
        }
        // 2. For Shared -> Direct, reap only our owned MPD, observe natural
        // PipeWire/ALSA release for a bounded interval, then start Direct.
        // No second PCM open is used; MPD remains the sole acquisition owner.
        let start_direct = || -> Result<_, String> {
            sup_state.0.lock()
                .map_err(|e| format!("Falha ao acessar o supervisor do MPD: {e}"))?
                .start(&new_config)
        };
        let switch_result = if release_shared_before_direct {
            let identity = direct_device_identity.as_ref().ok_or("ALSA_DIRECT_ENDPOINT_IDENTITY_MISSING")?;
            start_direct_after_shared_release(
                || sup_state.0.lock()
                    .map_err(|e| format!("Falha ao acessar o supervisor para liberar a rota: {e}"))?
                    .stop(),
                || alsa_observation::wait_for_pcm_release(identity, Duration::from_secs(5))
                    .map_err(|reason| alsa_transition_error("shared_release", reason)),
                start_direct,
            )
        } else {
            start_direct()
        };
        library_changed_during_start =
            confirm_device_switch_start(switch_result.map(|sync| sync.changed), || {
                resume_analyzer.after_rollback(rollback_audio_switch(
                    &current_cfg,
                    playback_snapshot,
                    switch_queue.as_ref(),
                    &sup_state,
                    &audio_state,
                Some(&_transaction_guard),
                ))
            })?;

        // 3. Restaura fila/faixa/posição quando possível; Playing/Paused terminam pausados e Stopped permanece parado
        let restore_result = match audio_state.0.lock() {
            Ok(mut audio) => {
                let queue=switch_queue.as_ref().ok_or("Fila da troca de saída indisponível.")?;
                // No Play/Seek before commit, including buffer/ReplayGain/DoP-only Saves.
                audio.restore_for_route_save(playback_snapshot,queue)
            },
            Err(e) => Err(format!("não foi possível acessar a fila após trocar a saída: {e}")),
        };
        restore_outcome = finish_device_switch_restore(restore_result, || {
            let rollback = resume_analyzer.after_rollback(rollback_audio_switch(
                &current_cfg,
                playback_snapshot,
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
                Some(&_transaction_guard),
            ));
            rollback
        })?;
    }

    let verify_direct_before_commit = direct_device_identity.is_some()
        && route_save_resume
        && restore_outcome == audio::DeviceSwitchRestoreOutcome::Complete
        && playback_snapshot.is_some_and(|snapshot| snapshot.state == audio::PlaybackState::Playing);
    let direct_result = confirm_playing_direct_route_before_commit(
        verify_direct_before_commit,
        || {
            let gate = Arc::clone(app.state::<Arc<direct_playback::Gate>>().inner());
            provisional_direct_choice = Some(ProvisionalDirectChoice::stage(gate, &current_cfg, &new_config));
            let _restart = transaction_state.try_begin_restart().ok_or("ALSA_DIRECT_TRANSITION_CANCELLED")?;
            let mut audio = audio_state.0.lock().map_err(|_| "ALSA_DIRECT_PLAYBACK_UNAVAILABLE")?;
            let queue = switch_queue.as_ref().ok_or("OUTPUT_SAVE_QUEUE_UNAVAILABLE")?;
            match audio.resume_saved_route(
                playback_snapshot,
                queue,
                &route_save_watch,
                &_transaction_guard,
                |_| Ok(()),
            )? {
                "resumed" => Ok(()),
                _ => Err("ALSA_DIRECT_PLAYBACK_NOT_RESUMED".into()),
            }
        },
        || verify_direct_pcm_owned(
            &sup_state.0,
            &new_config,
            direct_device_identity.as_ref().ok_or("ALSA_DIRECT_ENDPOINT_IDENTITY_MISSING")?,
        ),
    );
    let direct_acquisition_confirmed = match direct_result {
        Ok(confirmed) => confirmed,
        Err(error) => {
            let rollback = resume_analyzer.after_rollback(rollback_applied_config_change(
                finishing_first_run,
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
                &_transaction_guard,
            ));
            return Err(error_with_rollback(
                format!("Falha ao confirmar a aquisição Direct antes de salvar a configuração: {error}"),
                rollback,
            ));
        }
    };

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
            let rollback = resume_analyzer.after_rollback(rollback_applied_config_change(
                finishing_first_run,
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
                &_transaction_guard,
            ));
            return Err(error_with_rollback(
                format!("Falha ao atualizar a biblioteca local: {}", library_error),
                rollback,
            ));
        }
    }

    if route_save_resume && source_muted && is_shared_output(&new_config) {
        // Never mute the system sink as an incidental effect of route Save.
        // An owned software mixer can preserve silence; a PW sink must already
        // be observed muted, otherwise refuse and roll back without any Play.
        let preserved=(||->Result<(),String>{
            let backend=sup_state.0.lock().map_err(|_|"OUTPUT_SAVE_MUTE_UNCONFIRMED")?.shared_volume_backend();
            if backend==Some(shared_volume::SharedVolumeBackend::PipeWire) {
                let value=shared_volume::read_pipewire_volume()?;
                if !value.muted&&value.value!=0{return Err("OUTPUT_SAVE_CANNOT_PRESERVE_MUTE_WITHOUT_CHANGING_SYSTEM_VOLUME".into());}
            } else {
                let audio=audio_state.0.lock().map_err(|_|"OUTPUT_SAVE_MUTE_UNCONFIRMED")?;
                audio.set_volume(0)?;
                if audio.observed_software_volume()?!=0{return Err("OUTPUT_SAVE_MUTE_UNCONFIRMED".into());}
            }
            Ok(())
        })();
        if let Err(error)=preserved {
            return Err(error_with_rollback(error,resume_analyzer.after_rollback(rollback_applied_config_change(
                finishing_first_run,audio_hw_changed,folders_changed,&current_cfg,playback_snapshot,switch_queue.as_ref(),&sup_state,&audio_state,&_transaction_guard,
            ))));
        }
    }

    let mut config_guard = match config_state.0.lock() {
        Ok(guard) => guard,
        Err(e) => {
            let original = format!("Falha ao publicar a nova configuração: {}", e);
            let rollback = resume_analyzer.after_rollback(rollback_applied_config_change(
                finishing_first_run,
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
                &_transaction_guard,
            ));
            return Err(error_with_rollback(original, rollback));
        }
    };
    let mut plex_guard = match plex_state.0.lock() {
        Ok(guard) => guard,
        Err(e) => {
            drop(config_guard);
            let original = format!("Falha ao atualizar a configuração do Plex: {}", e);
            let rollback = resume_analyzer.after_rollback(rollback_applied_config_change(
                finishing_first_run,
                audio_hw_changed,
                folders_changed,
                &current_cfg,
                playback_snapshot,
                switch_queue.as_ref(),
                &sup_state,
                &audio_state,
                &_transaction_guard,
            ));
            return Err(error_with_rollback(original, rollback));
        }
    };

    if let Err(save_error) = new_config.save() {
        if matches!(save_error,ConfigSaveError::AfterRenameUnconfirmed(_)){app.state::<Arc<direct_playback::Gate>>().forget_choice();}
        drop(plex_guard);
        drop(config_guard);
        let error = config_save_failure_with_rollback(save_error, || {
            resume_analyzer
                .after_rollback(rollback_applied_config_change(
                    finishing_first_run,
                    audio_hw_changed,
                    folders_changed,
                    &current_cfg,
                    playback_snapshot,
                    switch_queue.as_ref(),
                    &sup_state,
                    &audio_state,
                    &_transaction_guard,
                ))
                .map_err(ConfigSaveError::BeforeRename)
        });
        return Err(config_save_error_message(&error));
    }

    app.state::<Arc<direct_playback::Gate>>().commit_choice(&new_config);
    if let Some(choice) = provisional_direct_choice.as_mut() { choice.finalize(); }
    plex_guard.update_config(&new_config);
    music_sources::install(&new_config);
    if !music_sources::navidrome() { navidrome::suspend(); }
    let target_shared=is_shared_output(&new_config);
    let experimental_enabled = new_config.experimental_features;
    let usage_mode_changed = current_cfg.full_mode() != new_config.full_mode();
    *config_guard = new_config;
    drop(plex_guard);
    drop(config_guard);
    remote_bridge::beta_preference_changed(&app, current_cfg.experimental_features, experimental_enabled);
    if usage_mode_changed && !null_output_test_mode() {
        remote_bridge::usage_mode_changed(&app);
        upnp_bridge::usage_mode_changed(&app);
    }
    if !music_sources::local() { app.state::<LocalWatcherState>().stop_and_join(); }

    resume_analyzer.confirmed = true;
    if output_change_expected {
        // The supervisor accepted the target and config.json has been
        // published. This confirms software route application only; the
        // snapshot still requires independent PCM/DSP evidence for hardware.
        _transaction_guard.confirm_audio_transition();
    }
    if route_save_resume && restore_outcome==audio::DeviceSwitchRestoreOutcome::Complete
        && !direct_acquisition_confirmed
    {
        if let Some(_restart)=transaction_state.try_begin_restart() {
            let backend=sup_state.0.lock().map_err(|_|"OUTPUT_SAVE_COMMITTED_PLAYBACK_UNAVAILABLE")?.shared_volume_backend();
            let mut audio=audio_state.0.lock().map_err(|_|"OUTPUT_SAVE_COMMITTED_PLAYBACK_UNAVAILABLE")?;
            audio.resume_saved_route(playback_snapshot,switch_queue.as_ref().ok_or("OUTPUT_SAVE_QUEUE_UNAVAILABLE")?,&route_save_watch,&_transaction_guard,|audio|{
                if source_muted&&target_shared {
                    if backend==Some(shared_volume::SharedVolumeBackend::PipeWire) {
                        let value=shared_volume::read_pipewire_volume()?;
                        if !value.muted&&value.value!=0{return Err("OUTPUT_SAVE_MUTE_UNCONFIRMED".into());}
                    } else if audio.observed_software_volume()?!=0{return Err("OUTPUT_SAVE_MUTE_UNCONFIRMED".into());}
                }
                Ok(())
            })
                .map_err(|error|format!("OUTPUT_SAVE_COMMITTED_RESUME_FAILED: saída salva; reprodução não confirmada. {error}"))?;
        }
    }
    Ok(config_save_operation::SaveResponse::completed(restore_outcome))
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

// Applied-but-partial is a successful configuration change with an explicit warning.
// Only an actual restoration failure may restart the previous output.
fn finish_device_switch_restore(
    result: Result<audio::DeviceSwitchRestoreOutcome, String>,
    rollback: impl FnOnce() -> Result<(), String>,
) -> Result<audio::DeviceSwitchRestoreOutcome, String> {
    result.map_err(|error| error_with_rollback(format!("Falha ao restaurar reprodução: {}", error), rollback()))
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
    rollback_audio_switch_with(playback_snapshot,
        switch_queue.ok_or("Fila da troca de saída indisponível no rollback.")?,
        &audio_state.0, || Ok(()))
}

fn rollback_audio_switch(
    previous_config: &AppConfig,
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    switch_queue: Option<&ResolvedDeviceSwitchQueue>,
    sup_state: &State<'_, SupervisorState>,
    audio_state: &State<'_, AudioState>,
    route: Option<&config_save_operation::Transaction<'_>>,
) -> Result<(), String> {
    let result = rollback_audio_switch_with(
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
    );
    if let Some(route) = route {
        if result.is_ok() {
            route.fail_audio_transition_after_rollback();
        } else {
            route.mark_audio_transition_uncertain();
        }
    }
    result
}

fn rollback_audio_switch_with(
    playback_snapshot: Option<DeviceSwitchSnapshot>,
    switch_queue: &ResolvedDeviceSwitchQueue,
    audio: &Mutex<AudioEngine>,
    restart_previous: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let result = (|| {
        audio.lock().map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
            .validate_device_switch_queue(switch_queue)?;
        restart_previous()?;
        // Error recovery restores a stopped queue and logical paused cursor only.
        // Playback needs a later explicit user action, never a rollback Play/Seek.
        let outcome = audio.lock().map_err(|e| format!("Falha ao acessar a fila durante rollback: {}", e))?
            .restore_for_route_save(playback_snapshot, switch_queue)?;
        match outcome {
            audio::DeviceSwitchRestoreOutcome::Complete => Ok(()),
            audio::DeviceSwitchRestoreOutcome::PositionNotRestored => Err("Fila restaurada e pausada, mas a posição anterior não foi restaurada.".into()),
        }
    })();
    if result.is_err() {
        if let Ok(mut audio) = audio.lock() { audio.mark_queue_uncertain(); }
    }
    result
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
    route: &config_save_operation::Transaction<'_>,
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
                Some(route),
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

// Names come from the same configured/validated virtual roots exposed by the browser.
// An unresolved source remains unnamed; never fall back to a physical folder path.
fn label_local_album_sources(albums: &mut [audio::LocalAlbum], sources: &[supervisor::LocalLibrarySource]) {
    for album in albums {
        album.source_label = album.source_id.as_ref().and_then(|id| {
            sources.iter().find(|source| &source.id == id).map(|source| source.label.clone())
        });
    }
}

#[tauri::command]
async fn get_local_albums(app: AppHandle, diagnostic_id: Option<u64>) -> Result<Vec<audio::LocalAlbum>, String> {
    music_sources::require_local()?;
    let diagnostic = diagnostics::DevSpan::ipc("catalog.local", diagnostic_id);
    diagnostic.phase("executor.queued");
    tauri::async_runtime::spawn_blocking(move || {
        let mut diagnostic = diagnostic;
        let result = (|| {
            diagnostic.phase("wait.config");
            diagnostic.phase("filesystem.roots");
            let roots = diagnostics::DevSpan::new("catalog.roots");
            let (albums, _) = with_local_source_context(&app, |before, recheck, _context| {
                if before.observed_conflict {
                    return Err("LOCAL_CATALOG_IDENTITY_CONFLICT".into());
                }
                // All bound IDs label indexed metadata, including offline roots.
                // get_local_albums_at uses these IDs for labels, not filtering.
                // Available is not an admission test or a playback capability.
                let sources = before.sources.iter().map(|row| supervisor::LocalLibrarySource {
                    id: row.source_id.clone(), label: row.label.clone(),
                }).collect::<Vec<_>>();
                let source_ids = sources.iter().map(|source| source.id.clone()).collect();
                drop(roots);
                diagnostic.phase("wait.audio");
                // No blocking acquisition of AudioState while owning startup
                // and transaction admission; never hold it during MPD I/O.
                let read_socket = || -> Result<String, String> {
                    let state = app.state::<AudioState>();
                    let audio = state.0.try_lock()
                        .map_err(|_| "Conexão da biblioteca local indisponível.".to_string())?;
                    Ok(audio.local_album_socket_path())
                };
                let socket_path = read_socket()?;
                let expected_socket = MpdSupervisor::socket_path();
                if Path::new(&socket_path) != expected_socket.as_path() {
                    return Err("LOCAL_CATALOG_CONTEXT_CHANGED".into());
                }
                diagnostic.phase("mpd.catalog");
                let mut albums = {
                    let _mpd = diagnostics::DevSpan::new("catalog.mpd");
                    AudioEngine::get_local_albums_at(&socket_path, &source_ids)?
                };
                // Reuse the SAME inventory, which revalidates its original
                // ledger/anchor/mirror snapshots. Never replace it after a fault.
                let after = recheck()?;
                if after.observed_conflict {
                    return Err("LOCAL_CATALOG_IDENTITY_CONFLICT".into());
                }
                if before.sources != after.sources || read_socket()? != socket_path
                    || MpdSupervisor::socket_path() != expected_socket {
                    return Err("LOCAL_CATALOG_CONTEXT_CHANGED".into());
                }
                label_local_album_sources(&mut albums, &sources);
                Ok(albums)
                // Shared context still validates live/persisted config, owner,
                // profile/library and shutdown BEFORE releasing either guard.
            })?;
            Ok(albums)
        })();
        diagnostic.result(&result);
        result
    }).await.map_err(|_| "Consulta da biblioteca local indisponível.".to_string())?
}

#[tauri::command]
async fn search_local_tracks(query: String, app: AppHandle) -> Result<audio::LocalSearchTracks, String> {
    music_sources::require_local()?;
    AudioEngine::validate_local_search_query(&query)?;
    tauri::async_runtime::spawn_blocking(move || {
        // The same owner/config/source/transaction admission and final recheck
        // used by get_local_albums; no playback capability is inferred from rows.
        let (result, _) = with_local_source_context(&app, |before, recheck, _context| {
            if before.observed_conflict { return Err("LOCAL_CATALOG_IDENTITY_CONFLICT".into()); }
            let source_ids = before.sources.iter().map(|row| row.source_id.clone()).collect();
            let read_socket = || -> Result<String, String> {
                Ok(app.state::<AudioState>().0.try_lock()
                    .map_err(|_| "LOCAL_SEARCH_UNAVAILABLE")?.local_album_socket_path())
            };
            let socket = read_socket()?;
            let expected = MpdSupervisor::socket_path();
            if Path::new(&socket) != expected.as_path() { return Err("LOCAL_CATALOG_CONTEXT_CHANGED".into()); }
            let result = AudioEngine::search_local_tracks_at(&socket, &query, &source_ids)?;
            let after = recheck()?;
            if after.observed_conflict || before.sources != after.sources || read_socket()? != socket
                || MpdSupervisor::socket_path() != expected { return Err("LOCAL_CATALOG_CONTEXT_CHANGED".into()); }
            Ok(result)
        })?;
        Ok(result)
    }).await.map_err(|_| "LOCAL_SEARCH_UNAVAILABLE".to_string())?
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
) -> Result<plex::PlexAlbumCatalog, plex::PlexAlbumCatalogError> {
    let client = state.0.lock().unwrap().clone();
    client.get_albums(&section_key, &sort_by).await
}

#[tauri::command]
async fn get_plex_discover(server_id: String, section_key: String, language: String, state: State<'_, PlexState>) -> Result<plex::PlexDiscoverPage, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    client.get_discover(&server_id, &section_key, &language).await
}
#[tauri::command]
async fn get_plex_library_mix(server_id: String, section_key: String, language: String, limit: usize, decade: Option<u32>, style: Option<plex::PlexStyleSelection>, state: State<'_, PlexState>) -> Result<plex::PlexLibraryMixPage, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    if let Some(selection) = style { if decade.is_some() { return Err("PLEX_DISCOVER_INVALID".into()); } return client.get_library_style_mix(&server_id,&section_key,&language,limit,&selection).await; }
    match decade { Some(value) => client.get_library_decade_mix(&server_id, &section_key, &language, limit, value).await, None => client.get_library_mix(&server_id, &section_key, &language, limit).await }
}
#[tauri::command]
async fn get_plex_library_mix_styles(server_id: String, section_key: String, language: String, state: State<'_, PlexState>) -> Result<plex::PlexMixStyles, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    client.get_library_mix_styles(&server_id,&section_key,&language).await
}
#[tauri::command]
async fn get_plex_library_mix_years(server_id: String, section_key: String, language: String, state: State<'_, PlexState>) -> Result<plex::PlexMixYears, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    client.get_library_mix_years(&server_id, &section_key, &language).await
}
#[tauri::command]
async fn get_plex_discover_recent(server_id: String, section_key: String, language: String, window_size: usize, state: State<'_, PlexState>) -> Result<plex::PlexDiscoverRecent, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    client.get_discover_recent(&server_id, &section_key, &language, window_size).await
}
#[tauri::command]
async fn get_plex_discover_children(server_id: String, section_key: String, kind: String, rating_key: String, language: String, state: State<'_, PlexState>) -> Result<plex::PlexDiscoverHub, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    client.get_discover_children(&server_id, &section_key, &kind, &rating_key, &language).await
}

#[tauri::command]
async fn get_plex_discover_artist(server_id: String, section_key: String, rating_key: String, language: String, state: State<'_, PlexState>) -> Result<plex::PlexDiscoverArtistContent, String> {
    let client = state.0.lock().map_err(|_| "PLEX_DISCOVER_UNAVAILABLE")?.clone();
    client.get_discover_artist(&server_id,&section_key,&rating_key,&language).await
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
async fn get_artist_portrait(
    server_id: String,
    rating_key: String,
    state: State<'_, PlexState>,
) -> Result<Option<plex::PlexImageRef>, String> {
    let client = state
        .0
        .lock()
        .map_err(|_| "O estado da conexão Plex está indisponível.".to_string())?
        .clone();
    client.get_artist_portrait(&server_id, &rating_key).await
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
    navidrome::shutdown();
    app_handle.state::<remote::State>().shutdown();
    app_handle.state::<upnp::State>().shutdown();
    mpris::shutdown(app_handle);
    if app_handle.state::<library_recovery::State>().should_persist_shutdown_queue() {
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
    app_handle
        .state::<ProfileServiceRestoreState>()
        .begin_shutdown();
    scrobbling::shutdown(app_handle);
    app_handle.state::<shared_dsp::State>().begin_shutdown();
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
            if transaction_state.initial_plex.drain().is_err() || transaction_state.addition_playback.drain().is_err() {
                eprintln!("[Audio] Initial playback shutdown drain unconfirmed.");
                return;
            }
            sleep_timer::shutdown(&shutdown_app);
            shutdown_controlled_resources(&shutdown_app);
            shutdown_app.state::<shared_dsp::State>().join();
            transaction_state.mark_shutdown_complete();
        }
        if shutdown_app.state::<profile_lease::State>().reopen_requested() {
            shutdown_app.state::<profile_lease::State>().release();
            shutdown_app.restart();
        }
        shutdown_app.exit(exit_code);
    });
}


// Initial startup only: ownership comes from the actual Lease, retained startup
// mutex and Coordinator transaction, never from a preliminary Available row.
// This bridge cannot enroll/retire IDs, reconcile a mirror, or request a rescan.
struct InitialManagedGeneration<'a> {
    owner: &'a profile_lease::Lease,
    transactions: &'a ConfigTransactionState,
    transaction: &'a config_save_operation::Transaction<'a>,
    _startup_owned: &'a std::sync::MutexGuard<'a,bool>,
    config: &'a AppConfig,
    profile: recovery_fs::Dir,
    library: recovery_fs::Dir,
    mirror: local_media::mirror::ReadonlyMirrorSnapshot,
    pins: Vec<(&'static str,recovery_fs::Identity,Vec<u8>)>,
    database: recovery_fs::Identity,
    config_dir: PathBuf,
    library_dir: PathBuf,
    origins: Vec<(String,source_identity::ReadonlyOriginObservation)>,
}
impl InitialManagedGeneration<'_> {
    const ERROR: &'static str="SOURCE_STARTUP_UNCONFIRMED";
    fn observe_origins(&self)->Result<Vec<(String,source_identity::ReadonlyOriginObservation)>,String> {
        use source_identity::{ReadonlyIdentitySnapshot,ReadonlyOriginObservation as O};
        let identity=ReadonlyIdentitySnapshot::open(&self.library_dir).map_err(|_|Self::ERROR)?;
        if !identity.matches_manifest(self.mirror.manifest.registry_id.as_deref().ok_or(Self::ERROR)?,&self.mirror.manifest.sources) {
            return Err(Self::ERROR.into());
        }
        let mut rows=Vec::new();
        for (id,root) in &self.mirror.manifest.sources {
            if self.transactions.shutdown_requested() || self.transactions.operations.shutdown_requested(){return Err(Self::ERROR.into());}
            let observation=identity.observe(id).map_err(|_|Self::ERROR)?;
            match observation {
                O::Available if std::fs::canonicalize(root).map_err(|_|Self::ERROR)?.as_os_str()==root.as_os_str()=>{},
                O::NotFound=>{}, // Known bound identity stays reserved, not removed.
                _=>return Err(Self::ERROR.into()), // Conflict/permission/IO unknowns do not admit startup.
            }
            rows.push((id.clone(),observation));
        }
        identity.revalidate().map_err(|_|Self::ERROR)?;
        Ok(rows)
    }
    fn validate(&self,config:&AppConfig)->Result<(),String> {
        local_diagnostics::timed(local_diagnostics::Stage::StartupSourceAdmission, || self.validate_inner(config))
    }
    fn validate_inner(&self,config:&AppConfig)->Result<(),String> {
        let error=Self::ERROR;
        if config!=self.config || !self.transaction.belongs_to(self.transactions.operations.as_ref())
            || self.transactions.shutdown_requested() || self.transactions.shutdown_started()
            || self.transactions.operations.shutdown_requested() {return Err(error.into());}
        self.owner.validate().map_err(|_|error)?;self.profile.validate().map_err(|_|error)?;self.library.validate().map_err(|_|error)?;
        if self.profile.id()!=self.owner.root().id() || MpdSupervisor::sonante_config_dir()!=self.config_dir
            || MpdSupervisor::library_dir()!=self.library_dir || self.library_dir!=self.config_dir.join("library") {return Err(error.into());}
        managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir()).map_err(|_|error)?;
        let cancelled=||self.transactions.shutdown_requested() || self.transactions.operations.shutdown_requested();
        for (name,id,bytes) in &self.pins {
            let limit=if *name=="local-source-identities.lock" {32} else {16*1024*1024};
            if self.profile.file_identity(name).map_err(|_|error)?.as_ref()!=Some(id)
                || self.profile.read(name,limit,&cancelled).map_err(|_|error)?.as_deref()!=Some(bytes.as_slice())
                || self.profile.file_identity(name).map_err(|_|error)?.as_ref()!=Some(id) {return Err(error.into());}
        }
        if self.profile.file_identity("mpd-v2.db").map_err(|_|error)?.as_ref()!=Some(&self.database) {return Err(error.into());}
        // Keep the ORIGINAL mirror snapshot through startup: library topology,
        // marker, inode and timestamps may not change. Profile directory ctime
        // may legitimately change when Supervisor creates mpd.conf; ledger and
        // config identities/bytes remain pinned, and D03 is rechecked afresh.
        self.mirror.revalidate().map_err(|_|error)?;
        if self.observe_origins()?!=self.origins {return Err(error.into());}
        self.mirror.revalidate().map_err(|_|error)?;
        self.owner.validate().map_err(|_|error)?;
        Ok(())
    }
}
fn start_initial_mpd(
    supervisor:&mut MpdSupervisor,config:&AppConfig,owner_state:&profile_lease::State,
    transactions:&ConfigTransactionState,
)->Result<supervisor::LibrarySyncResult,String> {
    let owner=owner_state.lease()?;owner.validate()?;
    // Same order as startup refresh/readers. No AppHandle or fabricated bool is needed.
    let startup_owned=transactions.startup_config_read_gate.try_lock().map_err(|_|InitialManagedGeneration::ERROR)?;
    let transaction=transactions.begin()?;
    let config_dir=MpdSupervisor::sonante_config_dir();let library_dir=MpdSupervisor::library_dir();
    managed_save_intent::check_startup(&config_dir)?;
    let profile=recovery_fs::Dir::open(&config_dir)?;
    if profile.id()!=owner.root().id(){return Err(InitialManagedGeneration::ERROR.into());}
    let database=profile.file_identity("mpd-v2.db")?;
    let marker=library_dir.join(local_media::mirror::MANIFEST_NAME);
    let marker_present=match std::fs::symlink_metadata(&marker) {
        Ok(_)=>true,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>false,
        Err(_)=>return Err(InitialManagedGeneration::ERROR.into()),
    };
    // First-run/legacy/no-DB behavior stays strict; no partial inventory is
    // converted into a new generation or silently rescanned here.
    if !marker_present || database.is_none(){return supervisor.start(config);}
    let library=recovery_fs::Dir::open(&library_dir)?;
    let marker_bytes=library.read(local_media::mirror::MANIFEST_NAME,16*1024*1024,&||transactions.shutdown_requested())?
        .ok_or(InitialManagedGeneration::ERROR)?;
    let header:local_media::mirror::MirrorManifest=source_identity::readonly_unique_json(&marker_bytes)
        .map_err(|_|InitialManagedGeneration::ERROR)?;
    // Legacy managed formats still use their original strict path; they do not
    // acquire this new D03 existing-generation startup admission.
    if header.schema_version!=2 || header.registry_id.is_none(){return supervisor.start(config);}
    let mut pins=Vec::new();
    for name in ["config.json","local-source-identities.json","local-source-identities.lock"] {
        let id=profile.file_identity(name)?.ok_or(InitialManagedGeneration::ERROR)?;
        let limit=if name=="local-source-identities.lock" {32} else {16*1024*1024};
        let bytes=profile.read(name,limit,&||transactions.shutdown_requested() || transactions.operations.shutdown_requested())?.ok_or(InitialManagedGeneration::ERROR)?;
        if profile.file_identity(name)?.as_ref()!=Some(&id){return Err(InitialManagedGeneration::ERROR.into());}
        pins.push((name,id,bytes));
    }
    let mut persisted:AppConfig=serde_json::from_slice(&pins[0].2).map_err(|_|InitialManagedGeneration::ERROR)?;
    persisted.apply_legacy_migrations();
    if persisted!=*config{return Err(InitialManagedGeneration::ERROR.into());}
    // This proof lives only through startup. Use the existing watched snapshot
    // to avoid rereading every link at all five admission boundaries. Startup
    // captures once and validates after registration; unsupported notifications
    // retain full revalidation. Origins/config/Lease are still checked afresh.
    let mirror=local_diagnostics::timed(local_diagnostics::Stage::StartupSourceSnapshot, || local_media::mirror::ReadonlyMirrorSnapshot::open_for_startup(&library_dir).map_err(|_|InitialManagedGeneration::ERROR.to_string()))?;
    if !local_media::configured_source_binding_matches(&config.local_folders,&mirror.manifest.sources) {
        return Err(InitialManagedGeneration::ERROR.into());
    }
    let mut admission=InitialManagedGeneration{owner:&owner,transactions,transaction:&transaction,_startup_owned:&startup_owned,
        config,profile,library,mirror,pins,database:database.unwrap(),config_dir,library_dir,origins:Vec::new()};
    let origins_clock=Instant::now();
    admission.origins=admission.observe_origins()?;
    local_diagnostics::record(local_diagnostics::Stage::StartupSourceOrigins, origins_clock.elapsed(), None);
    admission.validate(config)?;
    supervisor.start_initial_existing(config,&admission)
}


fn prepare_application_profile(null_output_test: bool) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    let data_home = dirs::data_local_dir().ok_or_else(|| "DATA_DIRECTORY_UNAVAILABLE".to_string())?;
    let (root, isolated_profile) = application_profile_root(
        &home,
        &data_home,
        cfg!(debug_assertions),
        cfg!(feature = "experimental-package"),
        null_output_test,
        std::env::var_os("SONANTE_EXPERIMENTAL_PROFILE_ROOT").as_deref(),
    )?;
    if !root.parent().is_some_and(|parent| parent.is_dir()) {
        std::fs::create_dir_all(root.parent().ok_or("PROFILE_PARENT_UNAVAILABLE")?)
            .map_err(|_| "PROFILE_PARENT_CREATE_FAILED")?;
    }
    for path in [
        root.clone(),
        root.join("config"),
        root.join("data"),
        root.join("cache"),
        root.join("state"),
        root.join("runtime"),
        root.join("logs"),
    ] {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err("PROFILE_DIRECTORY_UNSAFE".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&path).map_err(|_| "PROFILE_DIRECTORY_CREATE_FAILED")?;
            }
            Err(_) => return Err("PROFILE_DIRECTORY_UNAVAILABLE".into()),
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "PROFILE_DIRECTORY_PROTECTION_FAILED")?;
    }
    if isolated_profile {
        for (name, path) in [
            ("XDG_CONFIG_HOME", root.join("config")),
            ("XDG_DATA_HOME", root.join("data")),
            ("XDG_CACHE_HOME", root.join("cache")),
            ("XDG_STATE_HOME", root.join("state")),
        ] {
            std::env::set_var(name, path);
        }
    }
    // XDG_RUNTIME_DIR belongs to the desktop session. PipeWire's ALSA plugin
    // and clients such as pw-cli use it to find the user's session socket.
    // Keep it inherited, and put Sonante's own MPD socket/FIFO in a separate
    // private child directory instead of replacing the session runtime.
    let app_runtime = prepare_064_runtime_dir(
        &root,
        null_output_test,
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
    )?;
    std::env::set_var("SONANTE_064_RUNTIME_DIR", app_runtime);
    std::env::set_var("SONANTE_064_PROFILE_ROOT", &root);
    std::env::set_var("SONANTE_064_PROFILE", "1");
    // Keep the explicit EQ development override supplied by scripts/dev-eq.sh.
    // MpdEngine and CamillaDSP independently require a private executable and
    // the pinned SHA-256 before using either path. The ordinary MPD override is
    // still cleared so ambient development settings cannot change Shared/Direct.
    for key in [
        "MPD_HOST", "MPD_PORT", "LISTEN_PID", "LISTEN_FDS", "LISTEN_FDNAMES",
        "SONANTE_DEV_MPD_PATH",
    ] {
        std::env::remove_var(key);
    }
    if null_output_test {
        for key in ["SONANTE_EQ_MPD_PATH", "SONANTE_EQ_CAMILLA_PATH"] {
            std::env::remove_var(key);
        }
    }
    initialize_sqlite_config_if_missing()?;
    Ok(root)
}

fn application_profile_root(
    home: &Path,
    data_home: &Path,
    development_build: bool,
    experimental_package: bool,
    null_output_test: bool,
    experimental_root: Option<&std::ffi::OsStr>,
) -> Result<(PathBuf, bool), String> {
    if experimental_package {
        let root = PathBuf::from(experimental_root.ok_or("EXPERIMENTAL_PROFILE_ROOT_REQUIRED")?);
        validate_experimental_profile_root(&root)?;
        return Ok((if null_output_test { root.join("null-output-test") } else { root }, true));
    }
    if null_output_test || development_build {
        let local = home.join(".local");
        let share = local.join("share");
        for parent in [&local, &share] {
            let metadata = std::fs::symlink_metadata(parent)
                .map_err(|_| "PROFILE_PARENT_UNAVAILABLE".to_string())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("PROFILE_PARENT_UNSAFE".into());
            }
        }
        let name = if null_output_test {
            "sonante-0.6.4-sqlite-null-test"
        } else {
            "sonante-0.6.4-sqlite"
        };
        return Ok((share.join(name), true));
    }
    Ok((data_home.join("sonante"), false))
}

/// The experimental launcher selects one absolute user-owned profile root.
/// Reject symlinked path components and unsafe writable ancestors before any
/// XDG directory is redirected into it. The ordinary build never calls this.
fn validate_experimental_profile_root(root: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if !root.is_absolute()
        || root.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("EXPERIMENTAL_PROFILE_ROOT_INVALID".into());
    }
    let home = dirs::home_dir().ok_or_else(|| "HOME_UNAVAILABLE".to_string())?;
    let stable_root = home.join(".local/share/sonante-0.6.4-sqlite");
    if root.starts_with(&stable_root) || stable_root.starts_with(root) {
        return Err("EXPERIMENTAL_PROFILE_ROOT_OVERLAPS_STABLE".into());
    }
    let uid = unsafe { libc::geteuid() };
    let mut missing_parent = false;
    for ancestor in root.ancestors().take_while(|path| !path.as_os_str().is_empty()) {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || (metadata.uid() != uid && metadata.uid() != 0)
                    || (metadata.mode() & 0o022 != 0
                        && !(metadata.uid() == 0 && metadata.mode() & 0o1000 != 0))
                {
                    return Err("EXPERIMENTAL_PROFILE_ROOT_UNSAFE".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if ancestor == root {
                    missing_parent = true;
                } else {
                    return Err("EXPERIMENTAL_PROFILE_PARENT_UNAVAILABLE".into());
                }
            }
            Err(_) => return Err("EXPERIMENTAL_PROFILE_ROOT_UNAVAILABLE".into()),
        }
    }
    if !missing_parent {
        let metadata = std::fs::symlink_metadata(root)
            .map_err(|_| "EXPERIMENTAL_PROFILE_ROOT_UNAVAILABLE")?;
        if metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err("EXPERIMENTAL_PROFILE_ROOT_UNSAFE".into());
        }
    }
    Ok(())
}

fn prepare_064_runtime_dir(
    profile_root: &std::path::Path,
    null_output_test: bool,
    session_runtime: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let suffix = if cfg!(feature = "experimental-package") && null_output_test {
        "sonante-exp065-null"
    } else if cfg!(feature = "experimental-package") {
        "sonante-experimental-065"
    } else if null_output_test {
        "sonante-0.6.4-sqlite-null-test"
    } else if cfg!(debug_assertions) {
        "sonante-0.6.4-sqlite"
    } else {
        "sonante"
    };
    let parent = if let Some(raw) = session_runtime {
        let path = PathBuf::from(raw);
        if !path.is_absolute() {
            return Err("SESSION_RUNTIME_INVALID".into());
        }
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|_| "SESSION_RUNTIME_UNAVAILABLE")?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err("SESSION_RUNTIME_UNSAFE".into());
        }
        path
    } else {
        profile_root.join("runtime")
    };
    let runtime = parent.join(suffix);
    match std::fs::symlink_metadata(&runtime) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() } => {}
        Ok(_) => return Err("PROFILE_RUNTIME_UNSAFE".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&runtime).map_err(|_| "PROFILE_RUNTIME_CREATE_FAILED")?;
        }
        Err(_) => return Err("PROFILE_RUNTIME_UNAVAILABLE".into()),
    }
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "PROFILE_RUNTIME_PROTECTION_FAILED")?;
    let metadata = std::fs::symlink_metadata(&runtime)
        .map_err(|_| "PROFILE_RUNTIME_UNAVAILABLE")?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
        || runtime.join("mpd.socket").as_os_str().as_encoded_bytes().len() >= 104
    {
        return Err("PROFILE_RUNTIME_UNSAFE".into());
    }
    Ok(runtime)
}

fn initialize_sqlite_config_if_missing() -> Result<(), String> {
    let config_path = MpdSupervisor::sonante_config_dir().join("config.json");
    initialize_sqlite_config_at(&config_path)
}

fn initialize_sqlite_config_at(config_path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(config_path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Give a new public SQLite profile the normal setup-wizard state.
            // Existing config files are never rewritten here.
            let config = AppConfig::default();
            let bytes = serde_json::to_vec_pretty(&config)
                .map_err(|_| "SQLITE_PROFILE_CONFIG_ENCODE_FAILED".to_owned())?;
            persistence::atomic_write_private_synced(config_path, &bytes, "config.json")
                .map_err(|_| "SQLITE_PROFILE_CONFIG_WRITE_FAILED".to_owned())
        }
        Err(_) => Err("SQLITE_PROFILE_CONFIG_UNAVAILABLE".into()),
    }
}

#[cfg(test)]
mod sqlite_profile_initialization_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn temp_config() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "sonante-064-first-run-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        (root.join("sonante"), root)
    }

    #[test]
    fn empty_sqlite_profile_shows_first_run_and_preserves_completed_config_on_restart() {
        let (config_dir, root) = temp_config();
        let config_path = config_dir.join("config.json");
        initialize_sqlite_config_at(&config_path).unwrap();
        let seeded = AppConfig::load_from_path(&config_path).unwrap();
        assert!(seeded.first_run);
        assert!(seeded.sources.unwrap_or_default().local);

        let mut configured = seeded;
        configured.first_run = false;
        configured.sources = Some(config::EnabledSources {
            local: true,
            plex: false,
            navidrome: false,
        });
        configured.local_folders = vec!["/synthetic/music".into()];
        let configured_bytes = serde_json::to_vec_pretty(&configured).unwrap();
        persistence::atomic_write_private_synced(&config_path, &configured_bytes, "config.json")
            .unwrap();
        initialize_sqlite_config_at(&config_path).unwrap();
        let restarted = AppConfig::load_from_path(&config_path).unwrap();
        assert!(!restarted.first_run);
        assert_eq!(restarted.sources, configured.sources);
        assert_eq!(restarted.local_folders, configured.local_folders);
        assert_eq!(std::fs::read(&config_path).unwrap(), configured_bytes);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn release_profile_uses_public_data_location_without_importing_development_profile() {
        let (_unused, home) = temp_config();
        let data_home = home.join("xdg-data");
        let share = home.join(".local/share");
        std::fs::create_dir_all(&share).unwrap();
        let (release, isolated) = application_profile_root(&home, &data_home, false, false, false, None).unwrap();
        assert_eq!(release, data_home.join("sonante"));
        assert!(!isolated);
        assert_ne!(release, share.join("sonante-0.6.4-sqlite"));
        assert_ne!(release, share.join("sonante-0.6.4-sqlite-null-test"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn development_and_null_profiles_remain_separate_from_public_data() {
        let (_unused, home) = temp_config();
        let data_home = home.join("xdg-data");
        let share = home.join(".local/share");
        std::fs::create_dir_all(&share).unwrap();
        let (development, development_isolated) = application_profile_root(&home, &data_home, true, false, false, None).unwrap();
        let (null_test, null_isolated) = application_profile_root(&home, &data_home, true, false, true, None).unwrap();
        assert_eq!(development, share.join("sonante-0.6.4-sqlite"));
        assert_eq!(null_test, share.join("sonante-0.6.4-sqlite-null-test"));
        assert!(development_isolated && null_isolated);
        assert_ne!(development, data_home.join("sonante"));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn experimental_profile_root_requires_absolute_non_symlink_private_location() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (_unused, root) = temp_config();
        let candidate = root.join("candidate-profile");
        validate_experimental_profile_root(&candidate).unwrap();
        std::fs::create_dir(&candidate).unwrap();
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o700)).unwrap();
        validate_experimental_profile_root(&candidate).unwrap();
        assert_eq!(
            validate_experimental_profile_root(std::path::Path::new("relative/profile"))
                .unwrap_err(),
            "EXPERIMENTAL_PROFILE_ROOT_INVALID"
        );
        assert_eq!(
            validate_experimental_profile_root(&candidate.join("../outside")).unwrap_err(),
            "EXPERIMENTAL_PROFILE_ROOT_INVALID"
        );
        let link = root.join("profile-link");
        symlink(&candidate, &link).unwrap();
        assert_eq!(
            validate_experimental_profile_root(&link).unwrap_err(),
            "EXPERIMENTAL_PROFILE_ROOT_UNSAFE"
        );
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            validate_experimental_profile_root(&candidate).unwrap_err(),
            "EXPERIMENTAL_PROFILE_ROOT_UNSAFE"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn experimental_runtime_preserves_session_pipewire_runtime_and_separates_profiles() {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt};
        let (_unused, root) = temp_config();
        let session = root.join("session-runtime");
        std::fs::DirBuilder::new().mode(0o700).create(&session).unwrap();
        let regular = prepare_064_runtime_dir(&root, false, Some(session.as_os_str())).unwrap();
        let null_test = prepare_064_runtime_dir(&root, true, Some(session.as_os_str())).unwrap();
        assert_eq!(regular.parent(), Some(session.as_path()));
        assert_eq!(null_test.parent(), Some(session.as_path()));
        assert_ne!(regular, null_test);
        assert_eq!(std::fs::metadata(&regular).unwrap().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&regular).unwrap().uid(), unsafe { libc::geteuid() });
        assert!(!session.join("mpd.socket").exists());
        std::fs::remove_dir_all(&regular).unwrap();
        std::fs::remove_dir_all(&null_test).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn experimental_runtime_rejects_unsafe_session_runtime_without_fallback() {
        use std::os::unix::fs::DirBuilderExt;
        let (_unused, root) = temp_config();
        let unsafe_session = root.join("unsafe-runtime");
        std::fs::DirBuilder::new().mode(0o755).create(&unsafe_session).unwrap();
        assert_eq!(
            prepare_064_runtime_dir(&root, false, Some(unsafe_session.as_os_str())).unwrap_err(),
            "SESSION_RUNTIME_UNSAFE"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let null_output_test = requested_null_output_test_mode()
        .expect("invalid Sonante null-output test mode");
    let profile_root = prepare_application_profile(null_output_test)
        .expect("could not prepare Sonante application profile");
    if cfg!(feature = "experimental-package") {
        shared_dsp::diagnostics::set_experimental_profile_root(&profile_root);
    }
    // SQLite backs Local in public and development profiles. Desktop
    // integrations are admitted only after the private MPD and profile
    // configuration are ready; each service still requires an explicit opt-in.
    let sqlite_mode = true;
    let sqlite_data = profile_root.join("data").join("catalog");
    match std::fs::symlink_metadata(&sqlite_data) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => panic!("isolated SQLite catalog path is unsafe"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&sqlite_data)
                .expect("could not create isolated SQLite catalog directory");
        }
        Err(_) => panic!("could not inspect isolated SQLite catalog directory"),
    }
    std::fs::set_permissions(
        &sqlite_data,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .expect("could not protect isolated SQLite catalog directory");
    let sqlite_database = sqlite_data.join("catalog.sqlite3");
    match std::fs::symlink_metadata(&sqlite_database) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => panic!("isolated SQLite database path is unsafe"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => panic!("could not inspect isolated SQLite database path"),
    }
    let sqlite_library = SqliteLibraryState::open(&sqlite_database)
        .expect("could not open isolated SQLite catalog");
    std::fs::set_permissions(
        &sqlite_database,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .expect("could not protect isolated SQLite database");
    let startup_clock = std::time::Instant::now();
    let config_clock = std::time::Instant::now();
    let socket_path = MpdSupervisor::socket_path().to_string_lossy().to_string();
    // Read the stop marker before loading configuration or starting the supervisor.
    // A pending/invalid record must never trigger rescan, reconcile or rollback.
    let mut managed_startup_allowed = false;
    let profile_owner = profile_lease::State::new(&MpdSupervisor::sonante_config_dir());
    let pending_config = profile_owner.lease().and_then(|owner| {
        owner.validate()?;
        let config = first_run_managed::PendingPreparation::config(&MpdSupervisor::sonante_config_dir())?;
        if config.is_some() { first_run_managed::PendingPreparation::open(&owner)?; }
        Ok(config)
    });
    let resume_initial = matches!(&pending_config, Ok(Some(_)));
    let (initial_config, config_valid) = match pending_config.and_then(|pending| match pending {
        Some(config) => Ok(config),
        None => managed_save_intent::guard_startup(
        &MpdSupervisor::sonante_config_dir(),
        || {
            profile_owner.lease()?.validate()?;
            managed_startup_allowed = true;
            AppConfig::load()
        },
    ),
    }) {
        Ok(config) => (config, true),
        Err(error) => {
            local_diagnostics::record(local_diagnostics::Stage::StartupConfig, config_clock.elapsed(), Some(&error));
            eprintln!(
                "[Persistence] Failed to load configuration: {}",
                crate::diagnostics::error_summary(&error)
            );
            (AppConfig::default(), false)
        }
    };

    music_sources::install(&initial_config);
    if config_valid { local_diagnostics::record(local_diagnostics::Stage::StartupConfig, config_clock.elapsed(), None); }
    let should_start_mpd = !resume_initial
        && should_start_mpd_for_null_test(config_valid, &initial_config, null_output_test);
    let startup_rescan = if should_start_mpd && !null_output_test {
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

    let transactions=ConfigTransactionState::new();
    let mut initial_catalog_operation = if resume_initial {
        Some(transactions.operations.reserve().expect("fresh initial catalogue reservation").1)
    } else { None };
    let playback_gate=Arc::new(direct_playback::Gate::new(transactions.operations.clone()));
    playback_gate.commit_choice(&initial_config);
    let mut supervisor = MpdSupervisor::new(&socket_path);
    supervisor.attach_playback_gate(playback_gate.clone());
    let mpd_started = if should_start_mpd {
        match local_diagnostics::timed(local_diagnostics::Stage::StartupEngine, || {
            if null_output_test {
                supervisor.start(&initial_config)
            } else {
                start_initial_mpd(&mut supervisor,&initial_config,&profile_owner,&transactions)
            }
        }) {
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
    let start_scrobbling = should_start_scrobbling_for_profile(
        sqlite_mode,
        config_valid,
        mpd_started,
        null_output_test,
    );

    let mut audio_engine = AudioEngine::new(
        &socket_path,
        &MpdSupervisor::library_dir().to_string_lossy(),
    );
    audio_engine.attach_direct_playback(playback_gate.clone());
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
    let remote_state = if sqlite_mode {
        remote::State::isolated_profile()
    } else {
        remote::State::default()
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        .setup(move |app| {
            if start_scrobbling {
                scrobbling::start(app.handle());
            }
            if let Err(reason) = system_tray::install(app.handle()) {
                eprintln!("[Tray] Optional system tray disabled: {reason}.");
            }
            Ok(())
        })
        .manage(AudioState::new(audio_engine))
        .manage(AnalyzerState(Mutex::new(audio_analyzer)))
        .manage(PlexState(Mutex::new(plex_client)))
        .manage(SupervisorState(Mutex::new(supervisor)))
        .manage(ConfigState(Mutex::new(initial_config)))
        .manage(transactions)
        .manage(ProfileServiceRestoreState::default())
        .manage(playback_gate)
        .manage(library_recovery::State::new(!config_valid))
        .manage(profile_owner)
        .manage(LocalWatcherState::new())
        .manage(index_coverage::State::default())
        .manage(shared_dsp::State::new())
        .manage(listening_history::HistoryState(std::sync::Mutex::new(listening_history::HistoryStore::default())))
        .manage(radio::State::default())
        .manage(sleep_timer::State::default())
        .manage(scrobbling::State::default())
        .manage(smart_playlists::State::default())
        .manage(PlaylistState(Mutex::new(PlaylistStore::default())))
        .manage(mpris::MprisState::default())
        .manage(upnp::State::default())
        .manage(remote_state)
        .manage(remote_bridge::MediaState::default())
        .manage(sqlite_library)
        .invoke_handler(tauri::generate_handler![
            local_diagnostics::get_local_diagnostics,
            analyze_library_recovery,
            cancel_library_recovery_analysis,
            get_library_recovery_attempt_diagnostic,
            export_library_recovery_report,
            execute_library_recovery,
            reopen_library_after_recovery,
            get_playback_status,
            get_mpd_status_snapshot,
            claim_audio_analyzer_host,
            release_audio_analyzer_host,
            start_audio_analyzer,
            stop_audio_analyzer,
            toggle_playback,
            change_playback_option,
            next_track,
            previous_track,
            seek_playback,
            play_uris,
            remote_bridge::get_remote_control,
            remote_bridge::set_remote_control,
            remote_bridge::pair_remote_control,
            remote_bridge::revoke_remote_control,
            remote_bridge::remote_control_qr,
            upnp_bridge::get_upnp_receiver,
            upnp_bridge::set_upnp_receiver,
            play_tracks,
            enqueue_tracks,
            get_queue_edit_snapshot,
            listening_history,
            navidrome::navidrome_status,
            navidrome::navidrome_configuration,
            navidrome::navidrome_search,
            navidrome::navidrome_artist,
            navidrome::navidrome_explore,
            navidrome::navidrome_song_navigation,
            navidrome::navidrome_player_navigation,
            navidrome::navidrome_favorites,
            navidrome::navidrome_set_favorite,
            navidrome::navidrome_connect,
            navidrome::navidrome_disconnect,
            navidrome::navidrome_albums,
            navidrome::navidrome_edition_catalog,
            navidrome::navidrome_album,
            navidrome::navidrome_cover,
            navidrome::navidrome_song_cover_id,
            navidrome::navidrome_play_album,
            search_radio,
            radio_filters,
            radio_favorites,
            play_radio,
            stop_radio,
            scrobbling::get_scrobbling,
            scrobbling::set_scrobbling,
            sleep_timer::get_sleep_timer,
            sleep_timer::set_sleep_timer,
            history_plex_navigation,
            edit_queue,
            admit_playback_intent,
            cancel_playback_intent,
            playback_intent_current,
            set_volume,
            set_shared_mute,
            set_direct_mute,
            confirm_direct_volume_choice,
            get_queue,
            play_queue_index,
            clear_queue,
            set_window_title,
            list_local_directory,
            get_local_album_tracks,
            get_edition_profile,
            ab_commands::ab_tracks,
            ab_commands::ab_begin,
            ab_commands::ab_load,
            ab_commands::ab_decode,
            ab_commands::ab_start,
            ab_commands::ab_alive,
            ab_commands::ab_end,
            ab_commands::ab_cancel,
            get_edition_cache_namespace,
            get_local_album_doctor,
            search_online_album_metadata,
            get_online_album_metadata_details,
            resolve_local_library_path,
            get_local_track_tags,
            get_local_file_info,
            player_source_info::get_player_source_info,
            player_source_info::get_album_source_info,
            get_local_album_tag_inspection,
            preflight_local_album_tag_batch,
            apply_local_album_tag_batch,
            update_local_track_tags,
            get_local_cover,
            get_local_covers,
            get_online_album_cover,
            get_online_cover_cache_status,
            get_artwork_enrichment_progress,
            save_artwork_enrichment_progress,
            get_local_albums,
            search_local_tracks,
            get_favorites,
            toggle_favorite,
            smart_playlists::smart_playlist_store,
            smart_playlists::smart_playlist_sources,
            smart_playlists::smart_playlist_catalog,
            smart_playlists::smart_playlist_preview,
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
            get_local_watch_needs_check,
            get_local_index_coverage,
            get_config,
            get_local_source_availability,
            equalizer_commands::get_equalizer,
            equalizer_commands::pick_equalizer_preset,
            equalizer_commands::export_equalizer_preset,
            equalizer_commands::edit_equalizer,
            equalizer_commands::apply_equalizer,
            equalizer_commands::compare_equalizer,
            save_config,
            get_config_save_operation,
            get_local_import_preview,
            get_local_import_artwork,
            get_audio_devices,
            is_null_output_test_mode,
            check_direct_device_availability,
            get_plex_libraries,
            resolve_player_plex_navigation,
            get_plex_albums,
            get_plex_collections,
            get_plex_discover,
            get_plex_discover_recent,
            get_plex_library_mix,
            get_plex_library_mix_years,
            get_plex_library_mix_styles,
            get_plex_discover_children,
            get_plex_discover_artist,
            get_collection_albums,
            get_artist_portrait,
            get_artist_albums,
            get_artist_top_tracks,
            get_album_tracks,
            search_plex,
            get_plex_image,
            plex_create_pin,
            plex_check_pin,
            plex_get_servers,
            open_external_url,
            sqlite_library::sqlite_library_choose_folder,
            sqlite_library::sqlite_library_cancel_folder_selection,
            sqlite_library::sqlite_library_sources,
            sqlite_library::sqlite_library_legacy_source_migration_status,
            sqlite_library::sqlite_library_import_legacy_sources,
            sqlite_library::sqlite_library_local_albums,
            sqlite_library::sqlite_library_local_album_tracks,
            sqlite_library::sqlite_library_search_tracks,
            sqlite_library::sqlite_library_album_cover,
            sqlite_library::sqlite_library_album_for_track,
            sqlite_library::sqlite_library_add_and_index,
            sqlite_library::sqlite_library_verify_source,
            sqlite_library::sqlite_library_reindex_source,
            sqlite_library::sqlite_library_remove_source,
            sqlite_library::sqlite_library_open_view,
            sqlite_library::sqlite_library_albums,
            sqlite_library::sqlite_library_artists,
            sqlite_library::sqlite_library_albums_for_artist,
            sqlite_library::sqlite_library_tracks_for_album,
            sqlite_library::sqlite_library_album_doctor,
            sqlite_library_tag_album_inspection,
            sqlite_library_tag_album_preflight,
            sqlite_library_tag_album_apply,
        ])
        .on_window_event(|window, event| {
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                if window.label() == "now-playing" {
                    if let Some(state) = window.app_handle().try_state::<AnalyzerState>() {
                        if let Ok(mut analyzer) = state.0.lock() {
                            if let Err(error) = analyzer.stop_for_host_close(window.label(), window.app_handle()) {
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

    diagnostics::install_main_thread_probe(app.handle().clone());
    let restore_started = AtomicBool::new(false);
    app.run(move |app_handle, event| {
        match event {
            RunEvent::Ready => {
                local_diagnostics::record(local_diagnostics::Stage::StartupReady, startup_clock.elapsed(), None);
                if let Some(operation) = initial_catalog_operation.take() {
                    spawn_initial_catalog(
                        app_handle.clone(),
                        operation,
                        true,
                        None,
                        resume_initial,
                    );
                }
                if should_start_mpris_for_profile(
                    sqlite_mode,
                    managed_startup_allowed,
                    resume_initial,
                    mpd_started,
                    null_output_test,
                ) {
                    mpris::start(app_handle.clone());
                }
                let restore_services = should_restore_profile_services(
                    sqlite_mode,
                    config_valid,
                    mpd_started,
                    null_output_test,
                    managed_startup_allowed || resume_initial,
                );
                let transactions = app_handle.state::<ConfigTransactionState>();
                let restore_state = app_handle.state::<ProfileServiceRestoreState>();
                restore_state.run_once_if(
                    restore_services,
                    transactions.shutdown_requested() || transactions.shutdown_started(),
                    || {
                        if !transactions.shutdown_requested()
                            && !transactions.shutdown_started()
                        {
                            upnp_bridge::restore(app_handle);
                        }
                        if !transactions.shutdown_requested()
                            && !transactions.shutdown_started()
                        {
                            remote_bridge::restore(app_handle);
                        }
                    },
                );
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

fn should_start_mpris_for_profile(
    sqlite_mode: bool,
    config_admitted: bool,
    resume_initial: bool,
    mpd_started: bool,
    null_output_test: bool,
) -> bool {
    if null_output_test {
        return false;
    }
    if sqlite_mode {
        config_admitted && mpd_started
    } else {
        config_admitted || resume_initial
    }
}

fn should_start_scrobbling_for_profile(
    sqlite_mode: bool,
    config_admitted: bool,
    mpd_started: bool,
    null_output_test: bool,
) -> bool {
    !null_output_test && (!sqlite_mode || (config_admitted && mpd_started))
}

fn should_restore_profile_services(
    sqlite_mode: bool,
    config_admitted: bool,
    mpd_started: bool,
    null_output_test: bool,
    startup_admitted: bool,
) -> bool {
    if null_output_test {
        return false;
    }
    if sqlite_mode {
        config_admitted && mpd_started && startup_admitted
    } else {
        startup_admitted
    }
}

fn should_restore_after_resume(
    resume_initial: bool,
    preparation_succeeded: bool,
    mpd_ready: bool,
    config_admitted: bool,
    null_output_test: bool,
    startup_admitted: bool,
    shutdown_requested: bool,
) -> bool {
    resume_initial
        && preparation_succeeded
        && !shutdown_requested
        && should_restore_profile_services(
            true,
            config_admitted,
            mpd_ready,
            null_output_test,
            startup_admitted,
        )
}

fn profile_configuration_is_admitted(app: &AppHandle) -> bool {
    app.state::<profile_lease::State>()
        .lease()
        .and_then(|owner| {
            owner.validate()?;
            managed_save_intent::check_startup(&MpdSupervisor::sonante_config_dir())?;
            AppConfig::load().map(|_| ())
        })
        .is_ok()
}

fn supervised_mpd_is_ready(app: &AppHandle) -> bool {
    let state = app.state::<SupervisorState>();
    let Ok(mut supervisor) = state.0.lock() else {
        return false;
    };
    matches!(
        supervisor.observe_health(),
        Ok(MpdProcessObservation::Running)
    ) && supervisor.observed_process_pid().is_some()
}

fn restore_profile_services_after_resume(app: &AppHandle) {
    let transactions = app.state::<ConfigTransactionState>();
    let Some(_restart_permit) = transactions.try_begin_restart() else {
        return;
    };
    let shutting_down = || {
        transactions.shutdown_requested() || transactions.shutdown_started()
    };
    if shutting_down() {
        return;
    }

    // The successful resume path already started MPD through MpdSupervisor.
    // Re-observe that owned child here so a scan result alone cannot authorize
    // restoring any listener or observer.
    let mpd_ready = supervised_mpd_is_ready(app);
    let config_admitted = profile_configuration_is_admitted(app);
    let null_output_test = null_output_test_mode();
    let eligible = should_restore_after_resume(
        true,
        true,
        mpd_ready,
        config_admitted,
        null_output_test,
        true,
        shutting_down(),
    );
    let state = app.state::<ProfileServiceRestoreState>();
    state.run_once_if(eligible, shutting_down(), || {
        if !shutting_down() {
            upnp_bridge::restore(app);
        }
        if !shutting_down() {
            remote_bridge::restore(app);
        }
        if !shutting_down()
            && should_start_scrobbling_for_profile(
                true,
                config_admitted,
                mpd_ready,
                null_output_test,
            )
        {
            scrobbling::start(app);
        }
    });
}

#[cfg(test)]
mod profile_mpris_startup_tests {
    use super::should_start_mpris_for_profile;

    #[test]
    fn sqlite_mpris_requires_admitted_profile_and_its_supervised_mpd() {
        assert!(should_start_mpris_for_profile(true, true, false, true, false));
        assert!(!should_start_mpris_for_profile(true, false, false, true, false));
        assert!(!should_start_mpris_for_profile(true, true, false, false, false));
        assert!(!should_start_mpris_for_profile(true, true, false, true, true));
    }

    #[test]
    fn standard_profile_keeps_its_existing_managed_startup_policy() {
        assert!(should_start_mpris_for_profile(false, true, false, false, false));
        assert!(should_start_mpris_for_profile(false, false, true, false, false));
        assert!(!should_start_mpris_for_profile(false, false, false, false, false));
    }
}

#[cfg(test)]
mod profile_service_startup_tests {
    use super::{should_restore_profile_services, should_start_scrobbling_for_profile};

    #[test]
    fn sqlite_scrobbling_waits_for_valid_profile_and_private_mpd_and_never_starts_in_null_mode() {
        assert!(should_start_scrobbling_for_profile(true, true, true, false));
        assert!(!should_start_scrobbling_for_profile(true, false, true, false));
        assert!(!should_start_scrobbling_for_profile(true, true, false, false));
        assert!(!should_start_scrobbling_for_profile(true, true, true, true));
        // Preserve the original profile's actor policy outside the SQLite build.
        assert!(should_start_scrobbling_for_profile(false, false, false, false));
        assert!(!should_start_scrobbling_for_profile(false, true, true, true));
    }

    #[test]
    fn network_services_restore_only_after_admitted_non_null_mpd_startup() {
        assert!(should_restore_profile_services(true, true, true, false, true));
        assert!(!should_restore_profile_services(true, false, true, false, true));
        assert!(!should_restore_profile_services(true, true, false, false, true));
        assert!(!should_restore_profile_services(true, true, true, true, true));
        assert!(!should_restore_profile_services(true, true, true, false, false));
        // Preserve the original admission rule outside this SQLite profile.
        assert!(should_restore_profile_services(false, false, false, false, true));
    }
}

#[cfg(test)]
mod resume_profile_service_restore_tests {
    use super::{
        should_restore_after_resume, should_restore_profile_services,
        ProfileServiceRestoreState,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn ready_without_mpd_does_not_restore_but_successful_resume_restores_once() {
        let state = ProfileServiceRestoreState::default();
        let restores = AtomicUsize::new(0);

        // The existing Ready decision sees no MPD during resume_initial.
        let initial_ready = should_restore_profile_services(true, true, false, false, true);
        assert!(!initial_ready);
        assert!(!state.run_once_if(initial_ready, false, || {
            restores.fetch_add(1, Ordering::SeqCst);
        }));

        // After successful preparation, the supervised MPD is checked again.
        let resumed = should_restore_after_resume(true, true, true, true, false, true, false);
        assert!(resumed);
        assert!(state.run_once_if(resumed, false, || {
            restores.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(!state.run_once_if(resumed, false, || {
            restores.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_resume_shutdown_invalid_profile_and_null_output_do_not_restore() {
        let state = ProfileServiceRestoreState::default();
        let restores = AtomicUsize::new(0);
        for allowed in [
            should_restore_after_resume(false, true, true, true, false, true, false),
            should_restore_after_resume(true, false, true, true, false, true, false),
            should_restore_after_resume(true, true, false, true, false, true, false),
            should_restore_after_resume(true, true, true, false, false, true, false),
            should_restore_after_resume(true, true, true, true, true, true, false),
            should_restore_after_resume(true, true, true, true, false, false, false),
            should_restore_after_resume(true, true, true, true, false, true, true),
        ] {
            assert!(!state.run_once_if(allowed, false, || {
                restores.fetch_add(1, Ordering::SeqCst);
            }));
        }
        assert_eq!(restores.load(Ordering::SeqCst), 0);

        state.begin_shutdown();
        assert!(!state.run_once_if(true, false, || {
            restores.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(restores.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn service_restore_uses_latest_preferences_and_respects_each_service_gate() {
        let state = ProfileServiceRestoreState::default();
        let eligible = should_restore_after_resume(true, true, true, true, false, true, false);
        let mut current = [true, true, true];
        current[1] = false; // Remote control was disabled while preparation ran.
        let started = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

        assert!(state.run_once_if(eligible, false, || {
            for (enabled, counter) in current.into_iter().zip(&started) {
                if enabled {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        assert_eq!(started[0].load(Ordering::SeqCst), 1);
        assert_eq!(started[1].load(Ordering::SeqCst), 0);
        assert_eq!(started[2].load(Ordering::SeqCst), 1);
    }

    #[test]
    fn normal_ready_restore_is_once_and_does_not_require_resume_preparation() {
        let state = ProfileServiceRestoreState::default();
        let restores = AtomicUsize::new(0);
        let eligible = should_restore_profile_services(true, true, true, false, true);
        assert!(state.run_once_if(eligible, false, || {
            restores.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(!state.run_once_if(eligible, false, || {
            restores.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(restores.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
include!("app_tests.rs");

#[cfg(test)]
mod playback_intent_regressions {
    use super::*;
    #[test]
    fn manual_check_allows_latest_playback_but_failure_and_shutdown_reject_effects() {
        for closing in [false, true] {
            let audio = Mutex::new(audio::reliability_tests::fixture());
            let tx = ConfigTransactionState::new();
            let owner = tx.begin().unwrap();
            let session = tx.manual_check_playback.begin(&owner, tx.operations.as_ref(), &Default::default()).unwrap();
            let old = admit_or_validate_playback(&audio, &tx, None).unwrap();
            let latest = admit_or_validate_playback(&audio, &tx, None).unwrap();
            assert_eq!(finish_playback_intent(&audio, &tx, &old, |_| -> Result<(),String> { panic!("superseded effect") }).unwrap_err(), "PLAYBACK_SUPERSEDED");
            let mut applied = false;
            finish_playback_intent(&audio, &tx, &latest, |_| { applied = true; Ok(()) }).unwrap();
            assert!(applied);
            if closing { tx.request_shutdown(); } else { session.revoke(); }
            assert!(finish_playback_intent(&audio, &tx, &latest, |_| -> Result<(),String> { panic!("effect after revoke/shutdown") }).is_err());
            assert!(admit_or_validate_playback(&audio, &tx, None).is_err());
        }
    }

    #[test]
    fn delayed_backend_resolution_cannot_override_newer_local_or_playlist_or_error() {
        for resolver_failed in [false,true] {
            let audio=Mutex::new(audio::reliability_tests::fixture());
            let transactions=ConfigTransactionState::new();
            let old=admit_or_validate_playback(&audio,&transactions,None).unwrap();
            let effects=std::cell::Cell::new(0);
            let prepare=async {
                let new=admit_or_validate_playback(&audio,&transactions,None).unwrap();
                finish_playback_intent(&audio,&transactions,&new, |_| {effects.set(effects.get()+1);Ok(())}).unwrap();
                if resolver_failed {Err("old Plex resolver failure".into())} else {Ok(())}
            };
            let result=tauri::async_runtime::block_on(resolve_and_apply_playback_intent(&audio,&transactions,&old,prepare,|_,_|{effects.set(99);Ok(())}));
            assert_eq!(result.unwrap_err(),"PLAYBACK_SUPERSEDED");assert_eq!(effects.get(),1);
        }
    }
    #[test]
    fn direct_volume_intervention_supersedes_delayed_local_and_plex_play_without_queue_changes() {
        for resolver_failed in [false, true] {
            let audio = Mutex::new(audio::reliability_tests::fixture());
            let transactions = ConfigTransactionState::new();
            let token = admit_or_validate_playback(&audio, &transactions, None).unwrap();
            let queue = audio.lock().unwrap().get_queue();
            let prepare = async {
                let _transaction = transactions.begin().unwrap();
                invalidate_direct_volume_playback(&audio).unwrap();
                if resolver_failed { Err("old resolver failed".into()) } else { Ok(()) }
            };
            let result = tauri::async_runtime::block_on(resolve_and_apply_playback_intent(&audio, &transactions, &token, prepare,
                |_, _| -> Result<(), String> { panic!("late playback must never run at prepared unity gain") }));
            assert_eq!(result.unwrap_err(), "PLAYBACK_SUPERSEDED");
            assert_eq!(audio.lock().unwrap().get_queue(), queue);
        }
    }

    #[test]
    fn direct_volume_admission_refuses_busy_and_shutdown_instead_of_queueing_a_gesture() {
        let transactions = ConfigTransactionState::new();
        let active = transactions.begin().unwrap();
        assert!(matches!(begin_volume_transaction(&transactions), Err(ref error) if error == config_save_operation::BUSY));
        drop(active);
        assert!(begin_volume_transaction(&transactions).is_ok());
        transactions.request_shutdown();
        assert!(begin_volume_transaction(&transactions).is_err());
    }

    #[test]
    fn stale_direct_volume_dialog_does_not_cancel_a_current_playback_intent() {
        let audio = Mutex::new(audio::reliability_tests::fixture());
        let transactions = ConfigTransactionState::new();
        let play = admit_or_validate_playback(&audio, &transactions, None).unwrap();
        let session = direct_volume::Session::initialize(|command| Ok(if command=="status" {vec!["state: stop".into(), "volume: 0".into()]} else {vec![]})).unwrap();
        let context = direct_volume::Context::new(VolumeBackend::DirectEquipment, Some(session.clone()));
        assert!(admit_direct_volume_playback(&context, "stale:token", &audio).is_err());
        assert!(audio.lock().unwrap().validate_playback_intent(&play).is_ok());
        admit_direct_volume_playback(&context, &session.token(), &audio).unwrap();
        assert!(audio.lock().unwrap().validate_playback_intent(&play).is_err());
    }

    #[test]
    fn shutdown_or_configuration_invalidation_stops_prepared_effect_without_stopping_music() {
        for shutdown in [false,true] {
            let audio=Mutex::new(audio::reliability_tests::fixture());
            let transactions=ConfigTransactionState::new();
            let token=admit_or_validate_playback(&audio,&transactions,None).unwrap();
            if shutdown { transactions.request_shutdown(); } else {audio.lock().unwrap().invalidate_playback_intents();}
            assert_eq!(finish_playback_intent(&audio,&transactions,&token, |_| -> Result<(),String> {panic!("must not execute prepared effect")}).unwrap_err(),"PLAYBACK_SUPERSEDED");
        }
    }
}

#[cfg(test)]
#[test]
fn accepted_intent_keeps_error_feedback_current_after_mpd_failure() {
    let audio=Mutex::new(audio::reliability_tests::fixture());
    let transactions=ConfigTransactionState::new();
    let token=admit_or_validate_playback(&audio,&transactions,None).unwrap();
    let tracks=audio.lock().unwrap().get_queue();
    let result=finish_playback_intent(&audio,&transactions,&token, |audio| audio.play_tracks(tracks,vec!["fixture.flac".into()],0));
    assert_eq!(result.unwrap_err(),"PLAYBACK_QUEUE_UNCERTAIN");
    assert!(audio.lock().unwrap().validate_playback_intent(&token).is_ok(),"own failure must remain visible until a new intent/context");
}


#[cfg(test)]
mod local_updates_tests;

#[cfg(test)]
#[test]
fn eq_resume_new_play_caller_revokes_during_eq_queue_reservation(){
    let audio=Mutex::new(audio::reliability_tests::fixture());let tx=ConfigTransactionState::new();let watch=audio.lock().unwrap().watch_eq_transition();let _operation=tx.begin().unwrap();let _reservation=audio.lock().unwrap().capture_device_switch_queue().unwrap();
    assert!(admit_or_validate_playback(&audio,&tx,None).is_err());assert!(!watch.current());
}

#[cfg(test)]
mod explicit_incremental_check_tests {
    use super::*;
    use local_media::mirror::ManagedRefreshPlan as Plan;
    fn sources() -> std::collections::BTreeMap<String,PathBuf> {
        [("Music".into(),PathBuf::from("/fixture/music")),("Other".into(),PathBuf::from("/fixture/other"))].into()
    }
    #[test]
    fn scoped_incremental_check_same_path_update_never_expands_to_other_sources() {
        assert_eq!(scoped_incremental_check_plan(Plan::Targeted {directories:vec![],changed_files:0},&sources(),Some("Music")).unwrap(),
            Plan::Targeted {directories:vec!["Music".into()],changed_files:0});
        for plan in [Plan::Full,Plan::Targeted {directories:vec!["Other/Album".into()],changed_files:1},
            Plan::Targeted {directories:vec!["Musicology".into()],changed_files:1}] {
            assert!(scoped_incremental_check_plan(plan,&sources(),Some("Music")).is_err());
        }
        assert!(scoped_incremental_check_plan(Plan::MetadataOnly,&sources(),Some("missing")).is_err());
    }
    #[test]
    fn explicit_incremental_check_preserves_scoped_large_and_cue_updates() {
        let plan=Plan::Targeted {directories:vec!["Music/Album".into()],changed_files:40};
        assert_eq!(explicit_incremental_check_plan(plan.clone(),&sources()).unwrap(),plan);
    }
    #[test]
    fn explicit_incremental_check_checks_same_path_changes_by_source() {
        assert_eq!(explicit_incremental_check_plan(Plan::Targeted {directories:vec![],changed_files:0},&sources()).unwrap(),
            Plan::Targeted {directories:vec!["Music".into(),"Other".into()],changed_files:0});
    }
    #[test]
    fn explicit_incremental_check_refuses_global_rescan_without_relabeling_it() {
        assert_eq!(explicit_incremental_check_plan(Plan::Full,&sources()).unwrap_err(),"LOCAL_INCREMENTAL_CHECK_UNCONFIRMED");
        assert_eq!(explicit_incremental_check_plan(Plan::MetadataOnly,&sources()).unwrap(),Plan::MetadataOnly);
    }
}
