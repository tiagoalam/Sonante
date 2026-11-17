//! Confirmation of the existing read-only scan; no MPD, watcher or media writes.
use super::*;
use std::sync::Condvar;
use std::time::{Duration, Instant, UNIX_EPOCH};
const UNCONFIRMED: &str = "windows_library_check_unconfirmed";
const SCOPE_CHANGED: &str = "LOCAL_INCREMENTAL_CHECK_SCOPE_CHANGED";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_SIGNATURES: usize = 500_000;
#[derive(Default)]
pub(super) struct Signals {
    active: AtomicBool,
    wake: Mutex<u64>,
    changed: Condvar,
}
impl Signals {
    // This tiny gate is independent of catalog JSON/projection work. A waiter
    // captures the revision before reading catalog state and releases each
    // mutex before acquiring the other, so close need not wait for that state.
    pub(super) fn notify(&self) {
        match self.wake.lock() {
            Ok(mut wake) => {
                *wake = wake.wrapping_add(1);
                self.changed.notify_all();
            }
            Err(_) => {
                eprintln!("Windows library: confirmation_signal_unavailable");
                self.changed.notify_all();
            }
        }
    }
    pub(super) fn revision(&self) -> Result<u64> {
        self.wake
            .lock()
            .map(|wake| *wake)
            .map_err(|_| "library_state")
    }
    pub(super) fn wait(&self, seen: u64, timeout: Duration) -> Result<()> {
        let wake = self.wake.lock().map_err(|_| "library_state")?;
        if *wake == seen {
            drop(
                self.changed
                    .wait_timeout(wake, timeout)
                    .map_err(|_| "library_state")?,
            );
        }
        Ok(())
    }
}
struct Admission(Arc<Signals>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::Release);
    }
}
/// A change hint, not a file-content hash, media identity or authorization proof.
/// Only already-inspected metadata is consumed. Sorting avoids directory order
/// influencing the result; memory is bounded by 500000 * 32 bytes (plus Vec).
pub(super) struct Signature {
    hashes: Vec<[u8; 32]>,
    valid: bool,
}
impl Signature {
    pub(super) fn new() -> Self {
        Self {
            hashes: Vec::new(),
            valid: true,
        }
    }
    pub(super) fn relevant(path: &Path) -> bool {
        audio(path)
            || path
                .file_name()
                .and_then(|v| v.to_str())
                .is_some_and(|name| {
                    [
                        "cover.jpg",
                        "folder.jpg",
                        "front.jpg",
                        "cover.png",
                        "folder.png",
                        "front.png",
                    ]
                    .iter()
                    .any(|candidate| name.eq_ignore_ascii_case(candidate))
                })
    }
    pub(super) fn observe(&mut self, path: &Path, metadata: &fs::Metadata) -> Option<usize> {
        if !self.valid || !metadata.is_file() || linked(metadata) || !Self::relevant(path) {
            return None;
        }
        if self.hashes.len() >= MAX_SIGNATURES {
            self.valid = false;
            return None;
        }
        let Ok(modified) = metadata.modified() else {
            self.valid = false;
            return None;
        };
        let (negative, duration) = match modified.duration_since(UNIX_EPOCH) {
            Ok(value) => (false, value),
            Err(value) => (true, value.duration()),
        };
        let path = path.as_os_str().as_encoded_bytes();
        let mut hash = Sha256::new();
        hash.update(b"sonante-windows-catalog-entry-v1\0");
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path);
        hash.update(metadata.len().to_le_bytes());
        hash.update([u8::from(negative)]);
        hash.update(duration.as_secs().to_le_bytes());
        hash.update(duration.subsec_nanos().to_le_bytes());
        let slot = self.hashes.len();
        self.hashes.push(hash.finalize().into());
        Some(slot)
    }
    pub(super) fn record(&mut self, slot: Option<usize>, record: &Record) {
        let Some(slot) = slot else {
            return;
        };
        struct Writer(Sha256);
        impl std::io::Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = Writer(Sha256::new());
        writer.0.update(b"sonante-windows-catalog-metadata-v1\0");
        writer.0.update(self.hashes[slot]);
        // Scanner records contain one track. Stream serialization into the
        // digest; no serialized metadata buffer or extra media reads are used.
        let durations: Vec<_> = record
            .tracks
            .iter()
            .map(|track| (&track.id, record.durations.get(&track.id)))
            .collect();
        if serde_json::to_writer(
            &mut writer,
            &(&record.album, &record.tracks, durations, &record.search),
        )
        .is_err()
        {
            self.valid = false;
            return;
        }
        self.hashes[slot] = writer.0.finalize().into();
    }
    pub(super) fn finish(mut self) -> Option<[u8; 32]> {
        if !self.valid {
            return None;
        }
        self.hashes.sort_unstable();
        let mut hash = Sha256::new();
        hash.update(b"sonante-windows-catalog-scan-v1\0");
        hash.update((self.hashes.len() as u64).to_le_bytes());
        for entry in self.hashes {
            hash.update(entry);
        }
        Some(hash.finalize().into())
    }
}
fn ui_root(root: &Path) -> String {
    crate::windows_file_info::display_path(root)
}
impl Library {
    pub fn check_changes(
        &self,
        source_id: Option<&str>,
        expected_folders: Option<&[String]>,
    ) -> Result<bool> {
        self.confirm_scan(source_id, expected_folders, true, TIMEOUT)
    }
    pub fn rescan_confirmed(&self) -> Result<()> {
        self.confirm_scan(None, None, false, TIMEOUT).map(|_| ())
    }
    fn confirm_scan(
        &self,
        source_id: Option<&str>,
        expected_folders: Option<&[String]>,
        compare: bool,
        timeout: Duration,
    ) -> Result<bool> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        self.imports.require_idle()?;
        if self
            .checks
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("engine_busy");
        }
        let _admission = Admission(self.checks.clone());
        let (before, epoch) = {
            // Never wait behind folder admission/canonicalization, and never
            // retain the folder-change mutex while awaiting scan completion.
            let _change = match self.change.try_lock() {
                Ok(lock) => lock,
                Err(std::sync::TryLockError::WouldBlock) => return Err("engine_busy"),
                Err(_) => return Err("library_state"),
            };
            self.imports.require_idle()?;
            let (roots, before, was_cached) = {
                let state = self.data.lock().map_err(|_| "library_state")?;
                let roots = &state.roots;
                if roots.is_empty() { return Err("library_empty"); }
                if source_id.is_some_and(|id| id != "windows-local" && !roots.iter().any(|root| source_scope::source_id(root) == id))
                    || expected_folders
                        .is_some_and(|folders| folders != roots.iter().map(|root| ui_root(root)).collect::<Vec<_>>())
                {
                    return Err(SCOPE_CHANGED);
                }
                if state.status.phase == "scanning" {
                    return Err("engine_busy");
                }
                let was_cached = state.status.phase == "cached";
                let before = if compare {
                    if was_cached {
                        Some(state.cached_signature.ok_or(UNCONFIRMED)?)
                    } else {
                        if state.status.phase != "ready" || state.status.warnings != 0 || state.status.error.is_some() {
                            return Err(UNCONFIRMED);
                        }
                        Some(state.signature.ok_or(UNCONFIRMED)?)
                    }
                } else {
                    None
                };
                (roots.clone(), before, was_cached)
            };
            let target = source_id.filter(|id| *id != "windows-local")
                .and_then(|id| roots.iter().find(|r| source_scope::source_id(r) == id)).cloned();
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::ScanStarted {
                    origin: crate::local_diagnostics::LibraryScanOrigin::ExplicitCheck,
                    root_count: roots.len(),
                }, None);
            let started = if was_cached { self.begin_cached_scan_locked()? }
                else if let Some(root) = target { self.begin_source_locked(root)? }
                else { self.begin_locked(roots, false)? };
            (before, started.epoch)
        };
        let after = self.wait_for_scan(epoch, timeout)?;
        Ok(before.is_some_and(|before| before != after))
    }
    fn wait_for_scan(&self, epoch: u64, timeout: Duration) -> Result<[u8; 32]> {
        let deadline = Instant::now() + timeout;
        loop {
            let seen = self.checks.revision()?;
            {
                let state = self.data.lock().map_err(|_| "library_state")?;
                if self.stopping.load(Ordering::Acquire) {
                    return Err("shutting_down");
                }
                if state.status.epoch != epoch || self.epoch.load(Ordering::Acquire) != epoch {
                    return Err(SCOPE_CHANGED);
                }
                match state.status.phase.as_str() {
                    "ready" if state.status.warnings == 0 && state.status.error.is_none() => {
                        return state.signature.ok_or(UNCONFIRMED)
                    }
                    "scanning" => {}
                    "cached" if state.status.error.is_some() => return Err(state.status.error.unwrap_or(UNCONFIRMED)),
                    "failed" if state.status.error == Some(source_scope::OTHER_UNAVAILABLE) => return Err(source_scope::OTHER_UNAVAILABLE),
                    _ => return Err(UNCONFIRMED),
                }
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("windows_library_check_timeout")?;
            // notify between catalog read and wait changes seen, so that wake
            // cannot be lost. Neither wait nor signal holds the catalog mutex.
            self.checks.wait(seen, remaining)?;
        }
    }
}
#[cfg(test)]
#[path = "library_check_tests.rs"]
mod tests;
