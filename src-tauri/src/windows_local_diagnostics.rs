//! Passive, bounded measurements. No config, media, paths or credentials.
use serde::Serialize;
#[cfg(windows)]
use serde::Deserialize;
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    StartupConfig,
    StartupEngine,
    StartupSourceSnapshot,
    StartupSourceCapture,
    StartupSourceWatch,
    StartupSourceVerify,
    StartupSourceOrigins,
    StartupSourceAdmission,
    StartupEngineResolve,
    StartupEngineConfig,
    StartupEngineRuntime,
    StartupEngineVerify,
    StartupEngineWait,
    StartupEngineCompatibilityProbe,
    StartupEngineCompatibilityRetry,
    QueueResolve,
    QueueApply,
    StartupReady,
    QueueRestoreAttempt,
    PreferencesSave,
    WizardPreparation,
    WizardSourceAdmission,
    WizardWatch,
    WizardLibraryPrepare,
    WizardLibraryPublish,
    WizardEngine,
    WizardPreferences,
    WizardBackground,
    #[cfg(windows)]
    LibraryCatalog,
    #[cfg(windows)]
    AudioRoute,
}
#[derive(Clone, Serialize)]
pub(crate) struct Entry {
    stage: Stage,
    elapsed_ms: u64,
    outcome: &'static str,
    error_summary: Option<String>,
    #[cfg(windows)]
    #[serde(skip_serializing_if = "Option::is_none")]
    library_event: Option<LibraryEvent>,
    #[cfg(windows)]
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_route_event: Option<AudioRouteEvent>,
}

/// Windows Local catalog evidence contains only fixed labels and counts.
#[cfg(windows)]
#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum LibraryEvent {
    SnapshotMissing,
    SnapshotRestored { root_count: usize, album_count: usize, track_count: usize, provisional: bool, metadata_warning_count: usize },
    SnapshotIncompatible { reason: SnapshotIncompatibility, saved_root_count: usize, configured_root_count: usize },
    SnapshotRejected { reason: SnapshotRejection },
    SnapshotSaveSkipped { reason: SnapshotSaveSkip, warning_count: usize },
    SnapshotSaved { root_count: usize, album_count: usize, track_count: usize, provisional: bool, metadata_warning_count: usize },
    SnapshotSaveRejected { reason: SnapshotWriteFailure },
    ScanStarted { origin: LibraryScanOrigin, root_count: usize },
    ScanFinished { album_count: usize, track_count: usize, limited: bool, warnings: LibraryWarningCounts },
    ScanFailed { code: LibraryScanFailure },
    CachedVerification { result: CachedVerificationResult },
}

/// Format-only WASAPI Direct evidence; contains no endpoint identity or media data.
#[cfg(windows)]
#[derive(Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(crate) enum AudioRouteEvent {
    DirectPcmNegotiated {
        source: crate::windows_audio::negotiation::PcmFormat,
        plugin_input: crate::windows_audio::negotiation::PcmFormat,
        selected: crate::windows_audio::negotiation::PcmFormat,
        render_container_bits: u32,
        render_valid_bits: u32,
        converted: bool,
    },
    DirectDopNegotiated {
        source_rate: u32,
        source_channels: u32,
        transport: &'static str,
        transport_rate: u32,
        transport_channels: u32,
        render_container_bits: u32,
        render_valid_bits: u32,
    },
    DirectDopRejected {
        source_rate: u32,
        source_channels: u32,
        transport: &'static str,
        transport_rate: u32,
        transport_channels: u32,
        reason: &'static str,
    },
}

#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SnapshotIncompatibility { Version, RootCount, RootSet, RootRepresentation }
#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SnapshotRejection { UnsafeFile, ReadFailure, InvalidJson, Checksum, InvalidPayload }
#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SnapshotSaveSkip { Warnings, Unqualified, AddedState, CatalogState }
#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SnapshotWriteFailure { CatalogShape, RootIdentity, RecordValidation, Serialization, SizeLimit, Destination, Lock, AtomicWrite }
#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LibraryScanOrigin { StartupSnapshotUnavailable, RootConfiguration, ExplicitRefresh, ExplicitCheck, SourceReenabled }
#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LibraryScanFailure { NetworkUnavailable, OtherSourceUnavailable, Timeout, WorkerFailed, ScopeChanged, Other }
#[cfg(windows)]
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CachedVerificationResult { Started, Accepted, Preserved }
#[cfg(windows)]
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) struct LibraryWarningCounts {
    pub depth_limit: usize,
    pub directory_path: usize,
    pub directory_read: usize,
    pub entry_read: usize,
    pub metadata_read: usize,
    pub file_containment: usize,
    pub metadata_decode: usize,
}
#[cfg(windows)]
impl LibraryWarningCounts {
    pub(crate) fn total(&self) -> usize {
        self.depth_limit + self.directory_path + self.directory_read + self.entry_read
            + self.metadata_read + self.file_containment + self.metadata_decode
    }
    pub(crate) fn structural_total(&self) -> usize {
        self.depth_limit + self.directory_path + self.directory_read + self.entry_read
            + self.metadata_read + self.file_containment
    }
    pub(crate) fn metadata_only(&self) -> bool { self.structural_total() == 0 }
}
static ENTRIES: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
#[cfg(windows)]
const MAX_LIBRARY_EVENTS: usize = 64;
#[cfg(windows)]
static LIBRARY_EVENTS: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
#[cfg(windows)]
const MAX_AUDIO_ROUTE_EVENTS: usize = 32;
#[cfg(windows)]
static AUDIO_ROUTE_EVENTS: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();

fn push_bounded<T>(entries: &mut VecDeque<T>, value: T, limit: usize) {
    if entries.len() >= limit {
        entries.pop_front();
    }
    entries.push_back(value);
}

fn entry(stage: Stage, elapsed: Duration, error: Option<&str>) -> Entry {
    Entry {
        stage,
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
        outcome: if error.is_some() {
            "error"
        } else {
            "completed"
        },
        error_summary: error.map(crate::diagnostics::error_summary),
        #[cfg(windows)]
        library_event: None,
        #[cfg(windows)]
        audio_route_event: None,
    }
}
pub(crate) fn record(stage: Stage, elapsed: Duration, error: Option<&str>) {
    let entry = entry(stage, elapsed, error);
    if let Ok(mut entries) = ENTRIES.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        push_bounded(&mut entries, entry, 32);
    }
}
#[cfg(windows)]
pub(crate) fn record_library_event(event: LibraryEvent, error: Option<&str>) {
    let mut value = entry(Stage::LibraryCatalog, Duration::ZERO, error);
    value.library_event = Some(event);
    if let Ok(mut entries) = LIBRARY_EVENTS.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        push_bounded(&mut entries, value, MAX_LIBRARY_EVENTS);
    }
}
#[cfg(windows)]
pub(crate) fn record_audio_route_event(event: AudioRouteEvent) {
    let mut value = entry(Stage::AudioRoute, Duration::ZERO, None);
    value.audio_route_event = Some(event);
    if let Ok(mut entries) = AUDIO_ROUTE_EVENTS.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        push_bounded(&mut entries, value, MAX_AUDIO_ROUTE_EVENTS);
    }
}
pub(crate) fn timed<T>(stage: Stage, run: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let start = Instant::now();
    let result = run();
    record(
        stage,
        start.elapsed(),
        result.as_ref().err().map(String::as_str),
    );
    result
}
/// Same result and call count as the operation; diagnostics never retain the raw error.
/// These fixed enum labels are also useful before Settings becomes accessible.
pub(crate) fn wizard_step<T>(stage: Stage, run: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let started = Instant::now();
    let result = run();
    observe(stage, started.elapsed(), &result);
    result
}

pub(crate) fn observe<T>(stage: Stage, elapsed: Duration, result: &Result<T, String>) {
    record(stage, elapsed, result.as_ref().err().map(String::as_str));
    if let Err(error) = result {
        use std::io::Write;
        // A closed or broken diagnostic stream must never replace the actual failure.
        let _ = writeln!(std::io::stderr(), "[Preferences] stage={stage:?} {}", crate::diagnostics::error_summary(error));
    }
}
#[derive(Serialize)]
pub(crate) struct Report {
    schema: u32,
    version: &'static str,
    entries: Vec<Entry>,
    #[cfg(windows)]
    library_catalog_events: Vec<Entry>,
    #[cfg(windows)]
    audio_route_events: Vec<Entry>,
}
#[tauri::command]
pub(crate) fn get_local_diagnostics() -> Report {
    Report {
        schema: 1,
        version: env!("CARGO_PKG_VERSION"),
        entries: ENTRIES
            .get()
            .and_then(|entries| {
                entries
                    .lock()
                    .ok()
                    .map(|entries| entries.iter().cloned().collect())
            })
            .unwrap_or_default(),
        #[cfg(windows)]
        library_catalog_events: LIBRARY_EVENTS
            .get()
            .and_then(|entries| entries.lock().ok().map(|entries| entries.iter().cloned().collect()))
            .unwrap_or_default(),
        #[cfg(windows)]
        audio_route_events: AUDIO_ROUTE_EVENTS
            .get()
            .and_then(|entries| entries.lock().ok().map(|entries| entries.iter().cloned().collect()))
            .unwrap_or_default(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn library_catalog_events_expose_only_fixed_reasons_and_counts() {
        let event = LibraryEvent::SnapshotRestored {
            root_count: 1,
            album_count: 12,
            track_count: 47,
            provisional: false,
            metadata_warning_count: 0,
        };
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.contains("snapshot_restored"));
        assert!(serialized.contains("\"album_count\":12"));
        for private in ["\\\\server\\share", "C:\\Users", "album title", "token="] {
            assert!(!serialized.contains(private));
        }
        let warnings = LibraryWarningCounts {
            metadata_decode: 2,
            directory_read: 3,
            ..Default::default()
        };
        assert_eq!(warnings.total(), 5);
    }
    #[cfg(windows)]
    #[test]
    fn library_catalog_event_history_is_bounded_and_keeps_the_newest_events() {
        let mut events = VecDeque::new();
        for index in 0..(MAX_LIBRARY_EVENTS + 9) {
            push_bounded(&mut events, index, MAX_LIBRARY_EVENTS);
        }
        assert_eq!(events.len(), MAX_LIBRARY_EVENTS);
        assert_eq!(events.front(), Some(&9));
        assert_eq!(events.back(), Some(&(MAX_LIBRARY_EVENTS + 8)));
    }
    #[cfg(windows)]
    #[test]
    fn direct_pcm_observation_serializes_only_validated_format_data() {
        let event = AudioRouteEvent::DirectPcmNegotiated {
            source: crate::windows_audio::negotiation::PcmFormat { rate: 44100, encoding: crate::windows_audio::negotiation::PcmEncoding::S16, channels: 2 },
            plugin_input: crate::windows_audio::negotiation::PcmFormat { rate: 44100, encoding: crate::windows_audio::negotiation::PcmEncoding::S16, channels: 2 },
            selected: crate::windows_audio::negotiation::PcmFormat { rate: 48000, encoding: crate::windows_audio::negotiation::PcmEncoding::S16, channels: 2 },
            render_container_bits: 16,
            render_valid_bits: 16,
            converted: true,
        };
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(serialized.contains("direct_pcm_negotiated"));
        assert!(serialized.contains("\"rate\":48000"));
        assert!(serialized.contains("\"converted\":true"));
        for private in ["\\\\server\\share", "C:\\Users", "album title", "token="] {
            assert!(!serialized.contains(private));
        }
    }
    #[cfg(windows)]
    #[test]
    fn direct_dop_acceptance_and_rejection_events_are_format_only() {
        let accepted = AudioRouteEvent::DirectDopNegotiated {
            source_rate: 352800, source_channels: 2, transport: "dop",
            transport_rate: 176400, transport_channels: 2,
            render_container_bits: 32, render_valid_bits: 24,
        };
        let rejected = AudioRouteEvent::DirectDopRejected {
            source_rate: 352800, source_channels: 2, transport: "dop",
            transport_rate: 176400, transport_channels: 2, reason: "endpoint_rejected",
        };
        let text = format!("{}{}", serde_json::to_string(&accepted).unwrap(), serde_json::to_string(&rejected).unwrap());
        assert!(text.contains("direct_dop_negotiated"));
        assert!(text.contains("direct_dop_rejected"));
        assert!(text.contains("endpoint_rejected"));
        assert!(text.contains("176400"));
        for private in ["\\\\server\\share", "C:\\Users", "track.flac", "token="] {
            assert!(!text.contains(private));
        }
    }
    #[test]
    fn passive_measurements_preserve_result_and_never_store_private_error_text() {
        let mut calls = 0;
        let result: Result<(), String> = timed(Stage::StartupConfig, || {
            calls += 1;
            Err("https://secret/?password=private (os error 2)".into())
        });
        assert_eq!(calls, 1);
        assert!(result.unwrap_err().contains("password"));
        for _ in 0..100 {
            record(Stage::StartupReady, Duration::from_millis(12), None);
        }
        record(
            Stage::StartupEngine,
            Duration::from_millis(7),
            Some("https://secret/?password=private (os error 2)"),
        );
        let serialized = serde_json::to_string(&get_local_diagnostics()).unwrap();
        assert_eq!(get_local_diagnostics().entries.len(), 32);
        assert!(serialized.contains("os_code=2"));
        for value in ["https", "secret", "password", "private"] {
            assert!(!serialized.contains(value));
        }
    }
    #[test]
    fn wizard_steps_preserve_results_and_report_only_fixed_labels_and_safe_errors() {
        let mut calls = 0;
        let failure = "SONANTE_RUNTIME_MISSING: /home/private https://host/?token=secret (os error 2)";
        let result: Result<(), String> = wizard_step(Stage::WizardEngine, || {
            calls += 1;
            Err(failure.into())
        });
        assert_eq!(calls, 1);
        assert_eq!(result.unwrap_err(), failure);
        assert_eq!(wizard_step(Stage::WizardPreferences, || Ok(42)).unwrap(), 42);
        // Test the actual sanitizer independently of the bounded process-wide
        // queue: parallel tests may legitimately append or evict its entries.
        let entries = [entry(Stage::WizardEngine, Duration::ZERO, Some(failure)),
            entry(Stage::WizardPreferences, Duration::ZERO, None)];
        let encoded = serde_json::to_string(&entries).unwrap();
        for secret in ["/home", "private", "https://", "token", "secret"] {
            assert!(!encoded.contains(secret), "{encoded}");
        }
        assert!(encoded.contains("wizard_engine"));
        assert!(encoded.contains("wizard_preferences"));
        assert!(encoded.contains("SONANTE_RUNTIME_MISSING"));
        assert!(encoded.contains("os_code=2"));
    }

}
