//! Passive, bounded measurements. No config, media, paths or credentials.
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Serialize)]
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
    QueueResolve,
    QueueApply,
    StartupReady,
    QueueRestoreAttempt,
}
#[derive(Clone, Serialize)]
pub(crate) struct Entry {
    stage: Stage,
    elapsed_ms: u64,
    outcome: &'static str,
    error_summary: Option<String>,
}
static ENTRIES: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
pub(crate) fn record(stage: Stage, elapsed: Duration, error: Option<&str>) {
    let entry = Entry {
        stage,
        elapsed_ms: elapsed.as_millis().min(u128::from(u64::MAX)) as u64,
        outcome: if error.is_some() {
            "error"
        } else {
            "completed"
        },
        error_summary: error.map(crate::diagnostics::error_summary),
    };
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
}
