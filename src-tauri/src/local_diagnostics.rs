//! Passive, bounded measurements. No config, media, paths or credentials.
use serde::Serialize;
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
    WizardPreparationManagedMode,
    WizardPreparationSqliteMode,
    WizardPreparationProfile,
    WizardPreparationStartupGuard,
    WizardPreparationConfig,
    WizardPreparationValidation,
    WizardPreparationSqliteSources,
    WizardPreparationMpd,
    WizardPreparationPersist,
    WizardPreparationCleanup,
    WizardSourceAdmission,
    WizardWatch,
    WizardLibraryPrepare,
    WizardLibraryPublish,
    WizardEngine,
    WizardPreferences,
    WizardBackground,
}
#[derive(Clone, Serialize)]
pub(crate) struct Entry {
    stage: Stage,
    elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_us: Option<u64>,
    outcome: &'static str,
    error_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_observation: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_error_summary: Option<String>,
}
static ENTRIES: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
fn entry(stage: Stage, elapsed: Duration, error: Option<&str>) -> Entry {
    Entry {
        stage,
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
        elapsed_us: None,
        outcome: if error.is_some() {
            "error"
        } else {
            "completed"
        },
        error_summary: error.map(crate::diagnostics::error_summary),
        failure_reason: None,
        last_observation: None,
        cleanup_outcome: None,
        cleanup_error_summary: None,
    }
}
pub(crate) fn record(stage: Stage, elapsed: Duration, error: Option<&str>) {
    let entry = entry(stage, elapsed, error);
    if let Ok(mut entries) = ENTRIES.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        if entries.len() == 32 {
            entries.pop_front();
        }
        entries.push_back(entry);
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

/// Records one terminal MPD readiness result, without adding per-poll events.
/// Optional fields keep older diagnostic consumers and records compatible.
pub(crate) fn record_startup_wait(
    elapsed: Duration,
    error: Option<&str>,
    failure_reason: Option<&'static str>,
    last_observation: Option<&'static str>,
    cleanup_outcome: Option<&'static str>,
    cleanup_error: Option<&str>,
) {
    let entry = startup_wait_entry(
        elapsed,
        error,
        failure_reason,
        last_observation,
        cleanup_outcome,
        cleanup_error,
    );
    if let Ok(mut entries) = ENTRIES.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
        if entries.len() == 32 {
            entries.pop_front();
        }
        entries.push_back(entry);
    }
}

fn startup_wait_entry(
    elapsed: Duration,
    error: Option<&str>,
    failure_reason: Option<&'static str>,
    last_observation: Option<&'static str>,
    cleanup_outcome: Option<&'static str>,
    cleanup_error: Option<&str>,
) -> Entry {
    let mut entry = entry(Stage::StartupEngineWait, elapsed, error);
    entry.elapsed_us = Some(elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
    entry.failure_reason = failure_reason;
    entry.last_observation = last_observation;
    entry.cleanup_outcome = cleanup_outcome;
    entry.cleanup_error_summary = cleanup_error.map(crate::diagnostics::error_summary);
    entry
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
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn wizard_preparation_substages_are_stable_and_sanitize_original_errors() {
        let stages = [
            Stage::WizardPreparationManagedMode,
            Stage::WizardPreparationSqliteMode,
            Stage::WizardPreparationProfile,
            Stage::WizardPreparationStartupGuard,
            Stage::WizardPreparationConfig,
            Stage::WizardPreparationValidation,
            Stage::WizardPreparationSqliteSources,
            Stage::WizardPreparationMpd,
            Stage::WizardPreparationPersist,
            Stage::WizardPreparationCleanup,
        ];
        let entries = stages.map(|stage| entry(stage, Duration::from_micros(9),
            Some("private failure /home/user/music token=secret")));
        let encoded = serde_json::to_string(&entries).unwrap();
        for stage in ["wizard_preparation_managed_mode", "wizard_preparation_sqlite_mode",
            "wizard_preparation_profile", "wizard_preparation_startup_guard",
            "wizard_preparation_config", "wizard_preparation_validation",
            "wizard_preparation_sqlite_sources", "wizard_preparation_mpd",
            "wizard_preparation_persist", "wizard_preparation_cleanup"] {
            assert!(encoded.contains(stage), "missing {stage}: {encoded}");
        }
        for private in ["/home", "music", "token", "secret"] {
            assert!(!encoded.contains(private));
        }
        assert!(encoded.contains("error_hint=unclassified"));
    }

    #[test]
    fn startup_wait_diagnostic_keeps_submillisecond_precision_and_only_safe_categories() {
        let failure = "MPD exited with private path /home/user/music";
        let entry = startup_wait_entry(
            Duration::from_micros(723),
            Some(failure),
            Some("process_exited"),
            None,
            Some("failed"),
            Some("Permission denied /home/user/private"),
        );
        let encoded = serde_json::to_string(&entry).unwrap();
        assert!(encoded.contains("\"elapsed_ms\":0"));
        assert!(encoded.contains("\"elapsed_us\":723"));
        assert!(encoded.contains("\"failure_reason\":\"process_exited\""));
        assert!(encoded.contains("\"cleanup_outcome\":\"failed\""));
        assert!(encoded.contains("\"cleanup_error_summary\":\"error_hint=permission_denied\""));
        assert!(!encoded.contains("/home/user/music"));
        assert!(encoded.contains("\"error_summary\":\"error_hint=unclassified\""));
    }

}
