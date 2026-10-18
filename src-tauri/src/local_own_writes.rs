//! Coordinate only the existing tag editor's refresh with filesystem events.
//! Exact metadata receipts never suppress a subsequent external replacement.
use crate::local_changes::{Batch, MAX_DIRTY_PATHS};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

fn stamp(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata
        .is_file()
        .then(|| crate::tags::revision_for_metadata(&metadata))
}
#[derive(Clone, PartialEq, Eq)]
struct Entry {
    active: usize,
    completed: Option<String>,
}
#[derive(Default)]
pub(crate) struct OwnWrites {
    entries: Mutex<BTreeMap<PathBuf, Entry>>,
}
pub(crate) struct Lease {
    state: Arc<OwnWrites>,
    paths: Vec<PathBuf>,
}

impl OwnWrites {
    pub(crate) fn begin(
        self: &Arc<Self>,
        library: &Path,
        uris: &[String],
    ) -> Result<Lease, String> {
        let paths = uris
            .iter()
            .map(|uri| {
                if !crate::local_media::is_safe_relative_mpd_uri(uri) {
                    return Err("LOCAL_TAG_WATCH_PATH_INVALID".into());
                }
                let path = fs::canonicalize(library.join(uri))
                    .map_err(|_| "LOCAL_TAG_WATCH_PATH_UNAVAILABLE")?;
                if !path.is_absolute() || fs::canonicalize(&path).ok().as_ref() != Some(&path) {
                    return Err("LOCAL_TAG_WATCH_PATH_INVALID".into());
                }
                Ok(path)
            })
            .collect::<Result<std::collections::BTreeSet<_>, String>>()?
            .into_iter()
            .collect::<Vec<_>>();
        if paths.len() > MAX_DIRTY_PATHS {
            return Err("LOCAL_TAG_WATCH_BUSY".into());
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| "LOCAL_TAG_WATCH_UNAVAILABLE")?;
        // Completed receipts are disposable; never evict an active writer.
        while entries.len() + paths.iter().filter(|p| !entries.contains_key(*p)).count()
            > MAX_DIRTY_PATHS
        {
            let key = entries
                .iter()
                .find(|(p, e)| e.active == 0 && !paths.contains(p))
                .map(|(p, _)| p.clone())
                .ok_or("LOCAL_TAG_WATCH_BUSY")?;
            entries.remove(&key);
        }
        for path in &paths {
            let entry = entries.entry(path.clone()).or_insert(Entry {
                active: 0,
                completed: None,
            });
            entry.active += 1;
            entry.completed = None;
        }
        Ok(Lease {
            state: self.clone(),
            paths,
        })
    }

    /// None means an own writer is still active or its receipt changed while
    /// sampling. The watcher retains the entire batch under its normal retry.
    pub(crate) fn filter(&self, mut batch: Batch) -> Option<Batch> {
        let snapshot = {
            let entries = self.entries.lock().ok()?;
            batch
                .paths
                .iter()
                .filter_map(|p| entries.get(p).map(|e| (p.clone(), e.clone())))
                .collect::<BTreeMap<_, _>>()
        };
        if snapshot.values().any(|e| e.active != 0) {
            return None;
        }
        let handled = snapshot
            .iter()
            .filter(|(p, e)| e.completed.is_some() && stamp(p) == e.completed)
            .map(|(p, _)| p.clone())
            .collect::<Vec<_>>();
        let entries = self.entries.lock().ok()?;
        if batch
            .paths
            .iter()
            .any(|p| entries.get(p) != snapshot.get(p))
        {
            return None;
        }
        for path in handled {
            batch.paths.remove(&path);
        }
        Some(batch)
    }
}

impl Lease {
    /// Receipt is the revision returned from the tag writer's OWN file fd,
    /// never a later path sample that could already belong to another writer.
    pub(crate) fn complete(self, accepted: bool, revision: &str) {
        if accepted && !revision.is_empty() {
            if let Ok(mut entries) = self.state.entries.lock() {
                for path in &self.paths {
                    if let Some(entry) = entries.get_mut(path) {
                        entry.completed = Some(revision.to_owned());
                    }
                }
            }
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.state.entries.lock() {
            for path in &self.paths {
                if let Some(entry) = entries.get_mut(path) {
                    entry.active = entry.active.saturating_sub(1);
                }
            }
        }
    }
}
