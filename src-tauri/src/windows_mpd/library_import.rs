//! Windows adapter for the original config-save/provisional discovery contract.
//! No filesystem, MPD, playback capability or persistent recovery state here.
use super::super::protocol::Result;
use super::{source_scope, Record};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[path = "library_import_image.rs"]
mod image;
const BUSY: &str = "CONFIG_SAVE_BUSY";
const UNCONFIRMED: &str = "windows_library_check_unconfirmed";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Preparing,
    Indexing,
    Validating,
    Completed,
    Failed,
    Uncertain,
}
impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Uncertain)
    }
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Snapshot {
    pub id: String,
    pub phase: Phase,
    pub error: Option<&'static str>,
    pub confirmed_playback: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum SaveResponse {
    Completed,
    Accepted { operation: Snapshot },
}
#[derive(Clone, Serialize)]
pub(crate) struct AlbumDiscovery {
    provisional_group_key: String,
    source_id: String,
    album: Option<String>,
    artist: Option<String>,
    discs: Vec<String>,
    observed_tracks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    artwork_ref: Option<String>,
}
#[derive(Clone, Serialize)]
pub(crate) struct Preview {
    operation_id: String,
    initial: bool,
    provisional: bool,
    playable: bool,
    complete: bool,
    sampled_tracks: usize,
    sample_limit: usize,
    albums: Vec<AlbumDiscovery>,
}
struct Row {
    key: String,
    source: String,
    album: String,
    artist: String,
    disc: u32,
    album_id: String,
    artwork: bool,
}
struct Operation {
    snapshot: Snapshot,
    epoch: Option<u64>,
    sources: Vec<String>,
    initial: bool,
    committed: bool,
    observed: Phase,
    rows: VecDeque<Row>,
}
#[derive(Default)]
struct Inner {
    next: u64,
    owner: Option<u64>,
    transaction: bool,
    shutdown: bool,
    uncertain: bool,
    operation: Option<Operation>,
    preview_busy: bool,
    preview_not_before: Option<Instant>,
    artwork_busy: bool,
    artwork_not_before: Option<Instant>,
}
#[derive(Default)]
pub(crate) struct Coordinator {
    inner: Mutex<Inner>,
}
pub(crate) struct Guard {
    coordinator: Arc<Coordinator>,
    token: u64,
    indexing: bool,
    finished: bool,
}
pub(super) struct WorkerLease {
    coordinator: Arc<Coordinator>,
    epoch: u64,
}
pub(crate) struct PreviewLease {
    coordinator: Arc<Coordinator>,
    id: String,
    epoch: u64,
}
pub(crate) struct ArtworkLease {
    coordinator: Arc<Coordinator>,
    id: String,
    epoch: u64,
    group: String,
    album: String,
    source: String,
}
pub(super) fn thumbnail(bytes: &[u8]) -> Result<Option<String>> {
    image::thumbnail(bytes)
}
fn fail(op: &mut Operation, code: &'static str) {
    op.snapshot.phase = if code == "windows_config_uncertain" {
        Phase::Uncertain
    } else {
        Phase::Failed
    };
    op.snapshot.error = Some(code);
    op.observed = op.snapshot.phase;
    op.rows.clear();
}
impl Coordinator {
    pub(crate) fn admit(self: &Arc<Self>, indexing: bool, initial: bool) -> Result<Guard> {
        let mut i = self.inner.lock().map_err(|_| "library_state")?;
        if i.shutdown {
            return Err("shutting_down");
        }
        if i.uncertain {
            return Err("windows_config_uncertain");
        }
        if i.owner.is_some() {
            return Err(BUSY);
        }
        i.next = i.next.checked_add(1).ok_or("windows_config_uncertain")?;
        let token = i.next;
        i.owner = Some(token);
        i.transaction = true;
        if indexing {
            i.operation = Some(Operation {
                snapshot: Snapshot {
                    id: token.to_string(),
                    phase: Phase::Preparing,
                    error: None,
                    confirmed_playback: false,
                },
                epoch: None,
                sources: Vec::new(),
                initial,
                committed: false,
                observed: Phase::Indexing,
                rows: VecDeque::new(),
            });
        }
        Ok(Guard {
            coordinator: self.clone(),
            token,
            indexing,
            finished: false,
        })
    }
    pub(super) fn worker(self: &Arc<Self>, epoch: u64) -> WorkerLease {
        WorkerLease {
            coordinator: self.clone(),
            epoch,
        }
    }
    pub(crate) fn require_idle(&self) -> Result<()> {
        let i = self.inner.lock().map_err(|_| "library_state")?;
        if i.shutdown {
            return Err("shutting_down");
        }
        if i.uncertain {
            return Err("windows_config_uncertain");
        }
        if i.owner.is_some() {
            return Err(BUSY);
        }
        Ok(())
    }
    pub(crate) fn snapshot(&self) -> Result<Option<Snapshot>> {
        Ok(self
            .inner
            .lock()
            .map_err(|_| "library_state")?
            .operation
            .as_ref()
            .map(|op| op.snapshot.clone()))
    }
    pub(crate) fn supersede(&self, epoch: u64) {
        if let Ok(mut i) = self.inner.lock() {
            if let Some(op) = i
                .operation
                .as_mut()
                .filter(|o| !o.snapshot.phase.terminal() && o.epoch != Some(epoch))
            {
                fail(op, UNCONFIRMED);
                if !i.transaction {
                    i.owner = None;
                }
            }
        }
    }
    pub(crate) fn validating(&self, epoch: u64) {
        if let Ok(mut i) = self.inner.lock() {
            if let Some(op) = i
                .operation
                .as_mut()
                .filter(|o| o.epoch == Some(epoch) && !o.snapshot.phase.terminal())
            {
                op.observed = Phase::Validating;
                op.rows.clear();
                if op.committed {
                    op.snapshot.phase = Phase::Validating;
                }
            }
        }
    }
    pub(crate) fn finish(&self, epoch: u64, error: Option<&'static str>) {
        if let Ok(mut i) = self.inner.lock() {
            let transaction = i.transaction;
            if let Some(op) = i
                .operation
                .as_mut()
                .filter(|o| o.epoch == Some(epoch) && !o.snapshot.phase.terminal())
            {
                op.rows.clear();
                if let Some(code) = error {
                    fail(op, code);
                } else {
                    op.observed = Phase::Completed;
                    if op.committed {
                        op.snapshot.phase = Phase::Completed;
                    }
                }
                if !transaction {
                    i.owner = None;
                }
            }
        }
    }
    pub(crate) fn shutdown(&self) {
        if let Ok(mut i) = self.inner.lock() {
            i.shutdown = true;
            if let Some(op) = i
                .operation
                .as_mut()
                .filter(|o| !o.snapshot.phase.terminal())
            {
                fail(op, "LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN");
            }
            if !i.transaction {
                i.owner = None;
            }
        }
    }
    pub(super) fn feed(&self, epoch: u64, batch: &[Record]) {
        let Ok(mut i) = self.inner.lock() else { return };
        let Some(op) = i.operation.as_mut().filter(|o| {
            o.epoch == Some(epoch) && !o.snapshot.phase.terminal() && o.observed == Phase::Indexing
        }) else {
            return;
        };
        for record in batch {
            let source = source_scope::source_id(&record.root);
            if !op.sources.contains(&source) {
                continue;
            }
            let mut hash = Sha256::new();
            hash.update(b"sonante-provisional-group-v1\0");
            hash.update(op.snapshot.id.as_bytes());
            hash.update([0]);
            hash.update(record.album.id.as_bytes());
            let key = format!("{:x}", hash.finalize());
            for track in &record.tracks {
                if op.rows.len() == 128 {
                    op.rows.pop_front();
                }
                op.rows.push_back(Row {
                    key: key.clone(),
                    source: source.clone(),
                    album: bounded(&record.album.title),
                    artist: bounded(&record.album.artist),
                    disc: track.disc,
                    album_id: record.album.id.clone(),
                    artwork: record.cover.is_some(),
                });
            }
        }
    }
    pub(crate) fn artwork(
        self: &Arc<Self>,
        id: &str,
        reference: &str,
    ) -> Result<Arc<ArtworkLease>> {
        let mut i = self.inner.lock().map_err(|_| "library_state")?;
        if i.shutdown {
            return Err("shutting_down");
        }
        let op = i
            .operation
            .as_ref()
            .filter(|o| o.snapshot.id == id && o.committed && o.snapshot.phase == Phase::Indexing)
            .ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        if reference.len() > 128 {
            return Err("invalid_argument");
        }
        let prefix = format!("cover-v1:{id}:");
        let group = reference
            .strip_prefix(&prefix)
            .filter(|g| {
                g.len() == 64
                    && g.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .ok_or("invalid_argument")?;
        let row = op
            .rows
            .iter()
            .find(|r| r.key == group && r.artwork && op.sources.contains(&r.source))
            .ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        let (epoch, album, source) = (
            op.epoch.ok_or("LOCAL_IMPORT_PREVIEW_STALE")?,
            row.album_id.clone(),
            row.source.clone(),
        );
        if i.artwork_busy || i.artwork_not_before.is_some_and(|t| Instant::now() < t) {
            return Err("LOCAL_IMPORT_PREVIEW_BUSY");
        }
        i.artwork_busy = true;
        i.artwork_not_before = Some(Instant::now() + Duration::from_millis(500));
        Ok(Arc::new(ArtworkLease {
            coordinator: self.clone(),
            id: id.into(),
            epoch,
            group: group.into(),
            album,
            source,
        }))
    }
    pub(crate) fn preview(self: &Arc<Self>, id: &str) -> Result<PreviewLease> {
        let mut i = self.inner.lock().map_err(|_| "library_state")?;
        let op = i
            .operation
            .as_ref()
            .filter(|o| o.snapshot.id == id && o.committed && o.snapshot.phase == Phase::Indexing)
            .ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        let epoch = op.epoch.ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        if i.shutdown {
            return Err("shutting_down");
        }
        if i.preview_busy || i.preview_not_before.is_some_and(|t| Instant::now() < t) {
            return Err("LOCAL_IMPORT_PREVIEW_BUSY");
        }
        i.preview_busy = true;
        i.preview_not_before = Some(Instant::now() + Duration::from_millis(500));
        Ok(PreviewLease {
            coordinator: self.clone(),
            id: id.into(),
            epoch,
        })
    }
}
impl Guard {
    pub(super) fn bind(&self, epoch: u64, roots: &[PathBuf]) -> Result<()> {
        let mut i = self.coordinator.inner.lock().map_err(|_| "library_state")?;
        if i.shutdown {
            return Err("shutting_down");
        }
        if i.owner != Some(self.token) {
            return Err("windows_config_uncertain");
        }
        if !self.indexing {
            return Ok(());
        }
        let op = i
            .operation
            .as_mut()
            .filter(|o| {
                o.snapshot.id == self.token.to_string()
                    && !o.snapshot.phase.terminal()
                    && o.epoch.is_none()
            })
            .ok_or("windows_config_uncertain")?;
        op.epoch = Some(epoch);
        op.sources = roots.iter().map(|r| source_scope::source_id(r)).collect();
        Ok(())
    }
    pub(crate) fn commit(mut self) -> Result<SaveResponse> {
        let result = (|| {
            let mut i = self.coordinator.inner.lock().map_err(|_| "library_state")?;
            if i.owner != Some(self.token) {
                return Err("windows_config_uncertain");
            }
            if i.shutdown {
                return Err("LOCAL_FOLDERS_SAVE_INTERRUPTED_FOR_SHUTDOWN");
            }
            i.transaction = false;
            if !self.indexing {
                i.owner = None;
                return Ok(SaveResponse::Completed);
            }
            let op = i.operation.as_mut().ok_or("windows_config_uncertain")?;
            op.committed = true;
            if op.epoch.is_none() && !op.snapshot.phase.terminal() {
                fail(op, UNCONFIRMED);
            }
            if !op.snapshot.phase.terminal() {
                op.snapshot.phase = op.observed;
            }
            let snapshot = op.snapshot.clone();
            if snapshot.phase.terminal() {
                i.owner = None;
            }
            // Even a fast failed scan is an accepted operation with a real terminal
            // outcome; returning Completed would hide its failure from the UI monitor.
            if snapshot.phase == Phase::Completed {
                Ok(SaveResponse::Completed)
            } else {
                Ok(SaveResponse::Accepted {
                    operation: snapshot,
                })
            }
        })();
        if let Err(code) = result.as_ref() {
            self.fail_inner(code);
        }
        self.finished = true;
        result
    }
    pub(crate) fn fail(mut self, code: &'static str) {
        self.fail_inner(code);
        self.finished = true;
    }
    fn fail_inner(&self, code: &'static str) {
        if let Ok(mut i) = self.coordinator.inner.lock() {
            if i.owner != Some(self.token) {
                return;
            }
            i.transaction = false;
            i.owner = None;
            if code == "windows_config_uncertain" {
                i.uncertain = true;
            }
            if self.indexing {
                if let Some(op) = i.operation.as_mut() {
                    fail(op, code);
                }
            }
        }
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        if !self.finished {
            self.fail_inner("worker_failed");
        }
    }
}
impl PreviewLease {
    pub(crate) fn read(&self) -> Result<Preview> {
        let i = self.coordinator.inner.lock().map_err(|_| "library_state")?;
        let op = i
            .operation
            .as_ref()
            .filter(|o| {
                !i.shutdown
                    && o.snapshot.id == self.id
                    && o.epoch == Some(self.epoch)
                    && o.committed
                    && o.snapshot.phase == Phase::Indexing
            })
            .ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        let mut groups: BTreeMap<String, AlbumDiscovery> = BTreeMap::new();
        for row in &op.rows {
            if groups.len() >= 48 && !groups.contains_key(&row.key) {
                continue;
            }
            let group = groups
                .entry(row.key.clone())
                .or_insert_with(|| AlbumDiscovery {
                    provisional_group_key: row.key.clone(),
                    source_id: row.source.clone(),
                    album: Some(row.album.clone()).filter(|v| !v.is_empty()),
                    artist: Some(row.artist.clone()).filter(|v| !v.is_empty()),
                    discs: Vec::new(),
                    observed_tracks: 0,
                    artwork_ref: None,
                });
            if row.artwork {
                group.artwork_ref = Some(format!("cover-v1:{}:{}", self.id, row.key));
            }
            group.observed_tracks += 1;
            let disc = row.disc.to_string();
            if !group.discs.contains(&disc) {
                group.discs.push(disc);
            }
        }
        Ok(Preview {
            operation_id: self.id.clone(),
            initial: op.initial,
            provisional: true,
            playable: false,
            complete: false,
            sampled_tracks: op.rows.len(),
            sample_limit: 128,
            albums: groups.into_values().collect(),
        })
    }
}
impl ArtworkLease {
    pub(super) fn binding(&self) -> (&str, u64, &str) {
        (&self.album, self.epoch, &self.source)
    }
    pub(super) fn ensure_current(&self) -> Result<()> {
        let i = self.coordinator.inner.lock().map_err(|_| "library_state")?;
        if i.shutdown {
            return Err("LOCAL_IMPORT_PREVIEW_STALE");
        }
        let op = i
            .operation
            .as_ref()
            .filter(|o| {
                o.snapshot.id == self.id
                    && o.epoch == Some(self.epoch)
                    && o.committed
                    && o.snapshot.phase == Phase::Indexing
            })
            .ok_or("LOCAL_IMPORT_PREVIEW_STALE")?;
        if !op.rows.iter().any(|r| {
            r.key == self.group
                && r.album_id == self.album
                && r.source == self.source
                && r.artwork
                && op.sources.contains(&r.source)
        }) {
            return Err("LOCAL_IMPORT_PREVIEW_STALE");
        }
        Ok(())
    }
}
impl Drop for ArtworkLease {
    fn drop(&mut self) {
        if let Ok(mut i) = self.coordinator.inner.lock() {
            i.artwork_busy = false;
        }
    }
}
impl Drop for WorkerLease {
    fn drop(&mut self) {
        if let Ok(mut i) = self.coordinator.inner.lock() {
            let transaction = i.transaction;
            if let Some(op) = i.operation.as_mut().filter(|o| {
                o.epoch == Some(self.epoch)
                    && !o.snapshot.phase.terminal()
                    && o.observed != Phase::Completed
            }) {
                fail(op, "worker_failed");
                if !transaction {
                    i.owner = None;
                }
            }
        }
    }
}
impl Drop for PreviewLease {
    fn drop(&mut self) {
        if let Ok(mut i) = self.coordinator.inner.lock() {
            i.preview_busy = false;
        }
    }
}
fn bounded(value: &str) -> String {
    // The shared browser contract limits JavaScript UTF-16 string length.
    let mut units = 0;
    value
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= 256
        })
        .collect()
}
#[cfg(test)]
#[path = "library_import_tests.rs"]
mod tests;
