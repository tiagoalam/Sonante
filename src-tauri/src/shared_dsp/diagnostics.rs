//! Opt-in, private diagnostics for the integrated EQ activation path.
//! Records only fixed stage/reason labels and explicitly sanitized summaries.
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Mutex, OnceLock, RwLock,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

const LOG_NAME: &str = "eq-activation-diagnostics.jsonl";
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);
static NEXT_EVENT: AtomicU64 = AtomicU64::new(1);
static ACTIVE_TRACE: AtomicU64 = AtomicU64::new(0);
static RUN_ID: OnceLock<String> = OnceLock::new();
static ACTIVE_STARTED: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static WRITER: OnceLock<Option<SyncSender<Vec<u8>>>> = OnceLock::new();
static PROFILE_LOG_DIRECTORY: OnceLock<RwLock<Option<std::path::PathBuf>>> = OnceLock::new();
static LAST_GRAPH_EVENT: OnceLock<Mutex<Option<(u64, &'static str, std::time::Instant)>>> =
    OnceLock::new();

fn enabled() -> bool {
    enabled_for(std::env::var("SONANTE_EQ_DIAGNOSTICS").ok().as_deref())
}

fn enabled_for(value: Option<&str>) -> bool {
    value == Some("1")
}

pub(crate) fn is_enabled() -> bool {
    enabled()
}

/// Bind diagnostics to the profile selected by the application bootstrap.
/// The experimental package's XDG environment is intentionally private, but
/// using the resolved root directly also avoids depending on process-global
/// XDG state after startup.
pub(crate) fn set_experimental_profile_root(root: &std::path::Path) {
    let directory = resolve_log_directory(Some(root), || Err("normal profile not requested".to_owned()))
        .expect("experimental profile root always resolves");
    *PROFILE_LOG_DIRECTORY
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(directory);
}

fn diagnostic_directory() -> Result<std::path::PathBuf, &'static str> {
    if let Some(directory) = PROFILE_LOG_DIRECTORY
        .get_or_init(|| RwLock::new(None))
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return Ok(directory);
    }
    resolve_log_directory(None, crate::persistence::ensure_sonante_config_dir)
        .map_err(|_| "profile_unavailable")
}

fn resolve_log_directory<F>(
    experimental_root: Option<&std::path::Path>,
    normal_directory: F,
) -> Result<std::path::PathBuf, String>
where
    F: FnOnce() -> Result<std::path::PathBuf, String>,
{
    match experimental_root {
        Some(root) => Ok(root.join("logs")),
        None => normal_directory(),
    }
}

pub(crate) struct Attempt {
    id: u64,
    started: std::time::Instant,
    active: bool,
}

impl Attempt {
    pub(crate) fn begin() -> Self {
        let id = NEXT_TRACE.fetch_add(1, Ordering::Relaxed).max(1);
        let active = enabled();
        if active {
            ACTIVE_TRACE.store(id, Ordering::Release);
            *ACTIVE_STARTED
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(std::time::Instant::now());
            emit_for(id, "activation.received", "ok", Some(0), json!({}));
        }
        Self {
            id,
            started: std::time::Instant::now(),
            active,
        }
    }

    pub(crate) fn event(
        &self,
        stage: &'static str,
        outcome: &'static str,
        elapsed_ms: Option<u128>,
        details: Value,
    ) {
        if self.active {
            emit_for(self.id, stage, outcome, elapsed_ms, details);
        }
    }

    pub(crate) fn finish(&mut self, outcome: &'static str, code: &'static str) {
        if !self.active {
            return;
        }
        self.event(
            "activation.finished",
            outcome,
            Some(self.started.elapsed().as_millis()),
            json!({"code": code}),
        );
        // Keep the trace active after the command returns so later health,
        // rollback, and owned-process cleanup events remain correlated.
        self.active = false;
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if self.active {
            self.finish("interrupted", "activation_returned_early");
        }
    }
}

pub(crate) fn event(
    stage: &'static str,
    outcome: &'static str,
    elapsed_ms: Option<u128>,
    details: Value,
) {
    if !enabled() {
        return;
    }
    let trace = ACTIVE_TRACE.load(Ordering::Acquire);
    if trace != 0 {
        emit_for(trace, stage, outcome, elapsed_ms, details);
    }
}

pub(crate) fn safe_error_code(error: &str) -> &'static str {
    match error {
        "EQ_GraphInvalid" => "graph_invalid",
        "EQ_NodeMissing" => "node_missing",
        "EQ_SinkUnavailable" => "sink_unavailable",
        "EQ_RuntimeMissing" | "SONANTE_RUNTIME_MISSING" => "runtime_missing",
        "EQ_RuntimeUntrusted" | "SONANTE_RUNTIME_UNTRUSTED" => "runtime_untrusted",
        "EQ_RuntimeStartFailed" => "runtime_start_failed",
        "EQ_RuntimeExited" => "runtime_exited",
        "EQ_ControlFailed" => "control_failed",
        "EQ_ApplyFailed" => "apply_failed",
        "EQ_PipeWireUnavailable" => "pipewire_unavailable",
        "EQ_Cancelled" | "EQ_CANCELLED" => "cancelled",
        "EQ_CleanupFailed" => "cleanup_failed",
        "EQ_ContinuityLost" => "continuity_lost",
        "EQ_SHARED_ONLY" => "not_shared_route",
        "EQ_LIBRARY_UNAVAILABLE" => "library_unavailable",
        "EQ_PIPE_UNSAFE" => "pipe_unsafe",
        "EQ_PIPE_UNAVAILABLE" => "pipe_unavailable",
        "MPD_STARTUP_PROBE_TIMEOUT" => "mpd_probe_timeout",
        "MPD_STARTUP_PROBE_UNCONFIRMED" => "mpd_probe_unconfirmed",
        "MPD_STARTUP_PROBE_CLEANUP_UNCONFIRMED" => "mpd_cleanup_unconfirmed",
        "EQ_MPD_RUNTIME_MISSING" => "mpd_runtime_missing",
        "EQ_MPD_RUNTIME_UNAVAILABLE" => "mpd_runtime_unavailable",
        "EQ_CAMILLA_RUNTIME_MISSING" => "camilla_runtime_missing",
        "EQ_CAMILLA_RUNTIME_UNTRUSTED" => "camilla_runtime_untrusted",
        "EQ_PRESET_REQUIRED" => "preset_required",
        "EQ_PRESET_INVALID" => "preset_invalid",
        "EQ_UNSAFE_HEADROOM" => "headroom_rejected",
        "EQ_AUDIO_UNAVAILABLE" => "audio_state_unavailable",
        "EQ_SUPERVISOR_UNAVAILABLE" => "supervisor_unavailable",
        _ => "unclassified",
    }
}

/// Graph checks can run every few milliseconds while PipeWire publishes links.
/// Keep the first failure and periodic snapshots, without flooding the profile.
pub(crate) fn graph_event(reason: &'static str, details: Value) {
    if !enabled() {
        return;
    }
    let trace = ACTIVE_TRACE.load(Ordering::Acquire);
    if trace == 0 {
        return;
    }
    let lock = LAST_GRAPH_EVENT.get_or_init(|| Mutex::new(None));
    let now = std::time::Instant::now();
    let mut last = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let should_emit = last.as_ref().is_none_or(|(old_trace, old_reason, at)| {
        *old_trace != trace || *old_reason != reason || now.duration_since(*at).as_millis() >= 500
    });
    if should_emit {
        *last = Some((trace, reason, now));
        emit_for(
            trace,
            "pipewire.graph_check",
            "error",
            None,
            json!({"reason": reason, "snapshot": details}),
        );
    }
}

fn emit_for(
    trace: u64,
    stage: &'static str,
    outcome: &'static str,
    elapsed_ms: Option<u128>,
    details: Value,
) {
    let lock = WRITE_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let since_start_ms = if ACTIVE_TRACE.load(Ordering::Acquire) == trace {
        ACTIVE_STARTED
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|started| started.elapsed().as_millis())
    } else {
        None
    };
    let row = json!({
        "run": RUN_ID.get_or_init(|| format!("{}-{}", unsafe { libc::getpid() }, SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis())),
        "timestamp_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),
        "event": NEXT_EVENT.fetch_add(1, Ordering::Relaxed),
        "trace": trace,
        "stage": stage,
        "outcome": outcome,
        "elapsed_ms": elapsed_ms,
        "since_start_ms": since_start_ms,
        "details": details,
    });
    let Ok(mut bytes) = serde_json::to_vec(&row) else {
        return;
    };
    bytes.push(b'\n');

    let Some(writer) = WRITER.get_or_init(start_writer).as_ref() else {
        return;
    };
    // A full or disconnected writer drops this row; neither condition can
    // block the caller or change the audio operation.
    let _ = writer.try_send(bytes);
}

fn start_writer() -> Option<SyncSender<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(128);
    thread::Builder::new()
        .name("sonante-eq-diagnostics".into())
        .spawn(move || {
            while let Ok(bytes) = receiver.recv() {
                let result = diagnostic_directory()
                    .map(|directory| directory.join(LOG_NAME))
                    .and_then(|path| append_private(&path, &bytes));
                if let Err(reason) = result {
                    eprintln!("[EQ-Diagnostics] log_write={reason}");
                }
            }
        })
        .ok()?;
    Some(sender)
}

fn append_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("profile_path_invalid")?;
    let parent_meta = fs::symlink_metadata(parent).map_err(|_| "profile_unavailable")?;
    if !parent_meta.is_dir()
        || parent_meta.file_type().is_symlink()
        || parent_meta.uid() != unsafe { libc::geteuid() }
        || parent_meta.mode() & 0o077 != 0
    {
        return Err("profile_not_private");
    }
    let mut options = OpenOptions::new();
    options
        .create(true)
        .append(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    let mut file = options.open(path).map_err(|_| "open_failed")?;
    let metadata = file.metadata().map_err(|_| "metadata_failed")?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err("unsafe_file");
    }
    if metadata.len().saturating_add(bytes.len() as u64) > MAX_LOG_BYTES {
        return Err("size_limit");
    }
    if metadata.mode() & 0o777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "permissions_failed")?;
    }
    let path_meta = fs::symlink_metadata(path).map_err(|_| "path_recheck_failed")?;
    let file_meta = file.metadata().map_err(|_| "metadata_recheck_failed")?;
    if path_meta.file_type().is_symlink()
        || !path_meta.is_file()
        || (path_meta.dev(), path_meta.ino()) != (file_meta.dev(), file_meta.ino())
        || file_meta.uid() != unsafe { libc::geteuid() }
        || file_meta.nlink() != 1
        || file_meta.mode() & 0o777 != 0o600
    {
        return Err("file_changed");
    }
    file.write_all(&bytes)
        .and_then(|_| file.sync_data())
        .map_err(|_| "write_failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_labels_and_trace_are_fixed_and_drop_finishes_attempt() {
        assert_eq!(LOG_NAME, "eq-activation-diagnostics.jsonl");
        let first = NEXT_TRACE.fetch_add(1, Ordering::Relaxed);
        let second = NEXT_TRACE.fetch_add(1, Ordering::Relaxed);
        assert!(second > first);
        assert_eq!(safe_error_code("EQ_GraphInvalid"), "graph_invalid");
        assert_eq!(
            safe_error_code("private path /home/user/runtime"),
            "unclassified"
        );
    }

    #[test]
    fn opt_in_policy_is_independent_of_debug_assertions() {
        assert!(!enabled_for(None));
        assert!(!enabled_for(Some("0")));
        assert!(!enabled_for(Some("true")));
        assert!(enabled_for(Some("1")));
    }

    #[test]
    fn full_diagnostic_queue_drops_events_without_waiting() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        sender.try_send(vec![1]).unwrap();
        assert!(matches!(sender.try_send(vec![2]), Err(mpsc::TrySendError::Full(_))));
    }

    #[test]
    fn diagnostic_append_uses_private_mode_and_rejects_symlink_target() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join(LOG_NAME);
        append_private(&path, b"first\n").unwrap();
        append_private(&path, b"second\n").unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(fs::read(&path).unwrap(), b"first\nsecond\n");

        let target = directory.path().join("target");
        fs::write(&target, b"do not touch").unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(append_private(&link, b"unsafe\n"), Err("open_failed"));
        assert_eq!(fs::read(target).unwrap(), b"do not touch");
    }

    #[test]
    fn profile_resolution_uses_bootstrap_root_for_experimental_and_default_config_for_normal() {
        let temporary = tempfile::tempdir().unwrap();
        let experimental = temporary.path().join("sonante-experimental");
        let stable = temporary.path().join("stable-config/sonante");
        fs::create_dir_all(experimental.join("logs")).unwrap();
        fs::create_dir_all(&stable).unwrap();
        fs::set_permissions(experimental.join("logs"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&stable, fs::Permissions::from_mode(0o700)).unwrap();

        let resolved = resolve_log_directory(Some(&experimental), || Ok(stable.clone())).unwrap();
        assert_eq!(resolved, experimental.join("logs"));
        let selected = resolved.join(LOG_NAME);
        append_private(&selected, b"experimental\n").unwrap();
        assert_eq!(fs::read(&selected).unwrap(), b"experimental\n");
        assert!(!stable.join(LOG_NAME).exists());

        let normal_directory = resolve_log_directory(None, || Ok(stable.clone())).unwrap();
        assert_eq!(normal_directory, stable);
        let normal = normal_directory.join(LOG_NAME);
        append_private(&normal, b"normal\n").unwrap();
        assert_eq!(fs::read(normal).unwrap(), b"normal\n");
    }
}
