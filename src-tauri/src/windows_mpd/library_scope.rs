//! Existing source selector/check contract, with a private staging catalog.
//! Source IDs are presentation scopes, never media paths or write authority.
use super::*;
pub(super) const OTHER_UNAVAILABLE: &str = "LOCAL_INCREMENTAL_CHECK_OTHER_SOURCE_UNAVAILABLE";
#[derive(Clone)]
pub(super) struct Summary {
    pub hash: [u8; 32],
    pub entries: usize,
}
pub(super) fn source_id(root: &Path) -> String {
    format!("windows-root-{}", id(&root.to_string_lossy()))
}
pub(super) fn source_label(root: &Path, roots: &[PathBuf]) -> String {
    let label = root_label(&[root.to_path_buf()]).unwrap_or_else(|| "Music".into());
    if roots
        .iter()
        .filter(|r| root_label(&[(*r).clone()]).as_deref() == Some(label.as_str()))
        .count()
        > 1
    {
        crate::windows_file_info::display_path(root)
    } else {
        label
    }
}
/// Bounded aggregate change hint. Cached roots retain their prior digest even
/// when unrelated physical files change; only an explicit check discovers them.
pub(super) fn signature(
    roots: &[PathBuf],
    summaries: &BTreeMap<PathBuf, Summary>,
) -> Option<[u8; 32]> {
    if roots.len() > 32
        || roots.len() != summaries.len()
        || summaries
            .values()
            .try_fold(0usize, |n, s| n.checked_add(s.entries))?
            > 500_000
    {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"sonante-windows-catalog-roots-v1\0");
    for (root, summary) in summaries {
        if !roots.contains(root) {
            return None;
        }
        let root = root.as_os_str().as_encoded_bytes();
        hash.update((root.len() as u64).to_le_bytes());
        hash.update(root);
        hash.update(summary.hash);
    }
    Some(hash.finalize().into())
}
impl Library {
    /// Explicit user reconciliation of a restored cache. The scan is staged
    /// separately so an offline root or partial result cannot erase the cache.
    pub(super) fn begin_cached_scan_locked(&self) -> Result<Status> {
        if self.stopping.load(Ordering::Acquire) { return Err("shutting_down"); }
        let mut staged = self.data.lock().map_err(|_| "library_state")?.clone();
        if staged.status.phase != "cached" || staged.cached_signature.is_none() || !staged.sources.local || staged.roots.is_empty() {
            return Err("windows_library_check_unconfirmed");
        }
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::CachedVerification {
                result: crate::local_diagnostics::CachedVerificationResult::Started,
            }, None);
        let permit = scan_io::Admission::acquire()?;
        let roots = staged.roots.clone();
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        staged.signature = None;
        staged.cached_signature = None;
        staged.root_summaries.clear();
        staged.albums.clear();
        staged.status = Status {
            epoch, revision: 0, phase: "scanning".into(), root_name: root_label(&roots),
            albums: 0, tracks: 0, warnings: 0, metadata_warnings: 0, error: None,
        };
        {
            let mut live = self.data.lock().map_err(|_| "library_state")?;
            if live.status.phase != "cached" || live.cached_signature.is_none() { return Err("library_changed"); }
            live.status.epoch = epoch;
            live.status.revision = live.status.revision.wrapping_add(1);
            live.status.phase = "scanning".into();
            live.status.error = None;
            live.signature = None;
            live.editions_revision = live.editions_revision.wrapping_add(1);
        }
        self.checks.notify();
        let candidate = Self { data: Arc::new(Mutex::new(staged)), ..self.clone() };
        let live = self.clone();
        if std::thread::Builder::new().name("sonante-windows-cached-library-check".into()).spawn(move || {
            let _permit = permit;
            let failed = live.clone();
            let scope = match scan_io::Scope::start(move || failed.scan_failed(epoch, "index_timeout")) {
                Ok(scope) => scope,
                Err(_) => { live.scan_failed(epoch, "worker_failed"); return; }
            };
            candidate.scan(roots, epoch, scope, Some(&live));
        }).is_err() {
            self.scan_failed(epoch, "worker_failed");
            return Err("worker_failed");
        }
        self.status()
    }

    /// Caller holds change and has checked the qualified source/expected roots.
    /// Unlike a full scan, partial results stay private until every gate passes.
    pub(super) fn begin_source_locked(&self, root: PathBuf) -> Result<Status> {
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        let mut staged = self.data.lock().map_err(|_| "library_state")?.clone();
        if staged.signature.is_none()
            || !staged.sources.local
            || !staged.roots.contains(&root)
            || signature(&staged.roots, &staged.root_summaries) != staged.signature
        {
            return Err("windows_library_check_unconfirmed");
        }
        let roots = staged.roots.clone();
        let selected = root.clone();
        // This worker has no publication authority; a late result cannot start
        // a scan or persist preferences. Keep the existing six-second budget.
        crate::windows_io::read(std::time::Duration::from_secs(6), move |job| {
            for configured in roots {
                if !job.current() {
                    return Err("index_timeout");
                }
                let code = if configured == selected {
                    "library_unavailable"
                } else {
                    OTHER_UNAVAILABLE
                };
                if canonical_root_read(&configured, || job.current()).as_ref() != Ok(&configured) {
                    return Err(code);
                }
            }
            if !job.current() {
                return Err("index_timeout");
            }
            Ok(())
        })
        .map_err(io_wait_error)??;
        if self.stopping.load(Ordering::Acquire) {
            return Err("shutting_down");
        }
        let permit = scan_io::Admission::acquire()?;
        staged.albums.retain(|_, a| a.root != root);
        staged.root_summaries.remove(&root);
        staged.signature = None;
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        staged.status.epoch = epoch;
        staged.status.phase = "scanning".into();
        staged.status.warnings = 0;
        staged.status.error = None;
        staged.status.albums = staged.albums.len();
        staged.status.tracks = staged.albums.values().map(|a| a.tracks.len()).sum();
        {
            let mut live = self.data.lock().map_err(|_| "library_state")?;
            live.status.epoch = epoch;
            live.status.phase = "scanning".into();
            live.status.error = None;
            live.signature = None;
            live.editions_revision = live.editions_revision.wrapping_add(1);
        }
        self.checks.notify();
        let candidate = Self {
            data: Arc::new(Mutex::new(staged)),
            ..self.clone()
        };
        let live = self.clone();
        if std::thread::Builder::new()
            .name("sonante-windows-source-check".into())
            .spawn(move || {
                let _permit = permit;
                let failed = live.clone();
                let scope =
                    match scan_io::Scope::start(move || failed.scan_failed(epoch, "index_timeout"))
                    {
                        Ok(scope) => scope,
                        Err(_) => {
                            live.scan_failed(epoch, "worker_failed");
                            return;
                        }
                    };
                candidate.scan(vec![root], epoch, scope, Some(&live));
                live.finish_source_scan(&candidate, epoch);
            })
            .is_err()
        {
            self.scan_failed(epoch, "worker_failed");
            return Err("worker_failed");
        }
        self.status()
    }
    fn finish_source_scan(&self, candidate: &Library, epoch: u64) {
        let Ok(_change) = self.change.lock() else {
            return;
        };
        self.finish_source_scan_locked(candidate, epoch);
    }
    pub(super) fn finish_source_scan_locked(&self, candidate: &Library, epoch: u64) {
        let Ok(mut next) = candidate.data.lock() else {
            return;
        };
        let Ok(mut live) = self.data.lock() else {
            return;
        };
        if self.stopping.load(Ordering::Acquire)
            || self.epoch.load(Ordering::Acquire) != epoch
            || live.status.epoch != epoch
            || live.status.phase != "scanning"
        {
            return;
        }
        if self.current(epoch)
            && next.status.phase == "ready"
            && next.status.warnings == 0
            && next.status.error.is_none()
            && next.signature.is_some()
            && live.roots == next.roots
        {
            // Copy catalog fields only: unrelated mode/beta/online preferences
            // may have been saved during the scan and must not be overwritten.
            live.albums = std::mem::take(&mut next.albums);
            live.root_summaries = std::mem::take(&mut next.root_summaries);
            live.signature = next.signature;
            live.cached_signature = None;
            live.added_error = next.added_error;
            live.status = next.status.clone();
            live.status.revision = live.status.revision.wrapping_add(1);
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::CachedVerification {
                    result: crate::local_diagnostics::CachedVerificationResult::Accepted,
                }, None);
        } else {
            // No partial success or partial catalog replacement. Preserve the
            // former albums and report a failure requiring a qualified refresh.
            live.status.phase = if live.cached_signature.is_some() { "cached" } else { "failed" }.into();
            live.signature = None;
            live.status.error = Some(
                next.status
                    .error
                    .unwrap_or("windows_library_check_unconfirmed"),
            );
            crate::local_diagnostics::record_library_event(
                crate::local_diagnostics::LibraryEvent::CachedVerification {
                    result: crate::local_diagnostics::CachedVerificationResult::Preserved,
                }, live.status.error);
        }
        self.checks.notify();
    }
    pub(super) fn validate_retained_sources(
        &self,
        scanned: &[PathBuf],
        epoch: u64,
        scope: &scan_io::Scope,
    ) -> Result<()> {
        let (roots, targets) = {
            let s = self.data.lock().map_err(|_| "library_state")?;
            let roots = s
                .roots
                .iter()
                .filter(|r| !scanned.contains(r))
                .cloned()
                .collect::<Vec<_>>();
            let targets = s
                .albums
                .values()
                .filter(|a| !scanned.contains(&a.root))
                .flat_map(|a| {
                    a.tracks
                        .iter()
                        .map(|t| (a.root.clone(), t.path.clone()))
                        .chain(a.cover.iter().map(|p| (a.root.clone(), p.clone())))
                })
                .collect::<Vec<_>>();
            (roots, targets)
        };
        let current = || self.current(epoch) && scope.checkpoint();
        for root in roots {
            if !current()
                || canonical_root_read(&root, &current).as_ref() != Ok(&root)
                || !current()
            {
                return Err(OTHER_UNAVAILABLE);
            }
        }
        // Validate only the old published files; no read_dir, tag parser,
        // metadata rediscovery, source expansion or Added event for these roots.
        for (root, path) in targets {
            if !current() {
                return Err(OTHER_UNAVAILABLE);
            }
            crate::windows_file_info::with_bound_file_while(&root, &path, &current, |_file, _| {
                Ok(())
            })
            .map_err(|_| OTHER_UNAVAILABLE)?;
            if !current() {
                return Err(OTHER_UNAVAILABLE);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "library_scope_tests.rs"]
mod tests;
