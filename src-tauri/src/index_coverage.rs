//! Coverage of one observed indexing window, not a decoder/playability certificate.
use crate::{audio::AudioEngine, local_media};
use serde::Serialize;
use std::os::unix::fs::MetadataExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
pub(crate) const INVALID: &str = "LOCAL_INDEX_COVERAGE_INVALID";
pub(crate) const CHANGED: &str = "LOCAL_INDEX_COVERAGE_CHANGED";
pub(crate) const LOST: &str = "LOCAL_INDEX_SURVIVORS_MISSING";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Audio,
    Cue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn from(m: &fs::Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Selected {
    kind: Kind,
    physical: PathBuf,
    stamp: Stamp,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub generation: String,
    roots: BTreeMap<String, (PathBuf, u64, u64)>,
    selected: BTreeMap<String, Selected>,
    excluded: BTreeMap<String, usize>,
    unaddressable: BTreeMap<String, usize>,
    /// None is the entire scanned generation; Some is exactly the observed dirty paths.
    scope: Option<BTreeSet<String>>,
}
impl Inventory {
    pub fn new(
        generation: &str,
        sources: &BTreeMap<String, PathBuf>,
        full: bool,
    ) -> Result<Self, String> {
        let mut roots = BTreeMap::new();
        for (id, path) in sources {
            let m = fs::symlink_metadata(path).map_err(|_| CHANGED)?;
            if !m.is_dir() || m.file_type().is_symlink() {
                return Err(CHANGED.into());
            }
            roots.insert(id.clone(), (path.clone(), m.dev(), m.ino()));
        }
        Ok(Self {
            generation: generation.into(),
            roots,
            selected: BTreeMap::new(),
            excluded: BTreeMap::new(),
            unaddressable: BTreeMap::new(),
            scope: (!full).then(BTreeSet::new),
        })
    }
    pub fn observe(
        &mut self,
        id: &str,
        relative: &Path,
        physical: &Path,
        kind: Kind,
        metadata: &fs::Metadata,
    ) -> Result<(), String> {
        if !self.roots.contains_key(id) || !metadata.is_file() || metadata.file_type().is_symlink()
        {
            return Err(CHANGED.into());
        }
        let uri = Path::new(id).join(relative);
        let Some(uri) = uri
            .to_str()
            .filter(|u| local_media::is_safe_relative_mpd_uri(u))
        else {
            *self.unaddressable.entry(id.into()).or_default() += 1;
            return Ok(());
        };
        if let Some(scope) = self.scope.as_mut() {
            scope.insert(uri.into());
        }
        self.selected.insert(
            uri.into(),
            Selected {
                kind,
                physical: physical.into(),
                stamp: Stamp::from(metadata),
            },
        );
        Ok(())
    }
    pub fn source_ids(&self) -> BTreeSet<String> {
        self.roots.keys().cloned().collect()
    }
    pub fn exclude(&mut self, id: &str, count: usize) {
        *self.excluded.entry(id.into()).or_default() += count;
    }
    pub fn verify(&self) -> Result<(), String> {
        for (path, dev, ino) in self.roots.values() {
            let m = fs::symlink_metadata(path).map_err(|_| CHANGED)?;
            if !m.is_dir() || m.file_type().is_symlink() || m.dev() != *dev || m.ino() != *ino {
                return Err(CHANGED.into());
            }
        }
        // Metadata only, once per observed selected entry. No read_dir, media open,
        // header read or content hash; dirty scopes never walk an entire source.
        for selected in self.selected.values() {
            let m = fs::symlink_metadata(&selected.physical).map_err(|_| CHANGED)?;
            if !m.is_file() || m.file_type().is_symlink() || Stamp::from(&m) != selected.stamp {
                return Err(CHANGED.into());
            }
        }
        Ok(())
    }
    fn observes(&self, uri: &str) -> bool {
        self.scope.as_ref().is_none_or(|paths| {
            paths.contains(uri) || cue_parent(uri).is_some_and(|p| paths.contains(p))
        })
    }
}
fn cue_parent(uri: &str) -> Option<&str> {
    let (parent, name) = uri.rsplit_once('/')?;
    let number = name.strip_prefix("track")?;
    (!number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
        && parent.to_ascii_lowercase().ends_with(".cue"))
    .then_some(parent)
}
#[derive(Clone, Debug)]
pub(crate) struct Catalog {
    pub records: std::collections::HashSet<String>,
    files: BTreeSet<String>,
}
impl Catalog {
    pub fn from_records(records: std::collections::HashSet<String>) -> Result<Self, String> {
        let mut files = BTreeSet::new();
        for record in &records {
            let (kind, uri) = record.split_once(": ").ok_or(INVALID)?;
            if !matches!(kind, "file" | "directory" | "playlist")
                || !local_media::is_safe_relative_mpd_uri(uri)
            {
                return Err(INVALID.into());
            }
            if kind == "file" {
                if !local_media::is_catalog_track_uri(uri) {
                    return Err(INVALID.into());
                }
                files.insert(uri.into());
            }
        }
        Ok(Self { records, files })
    }
    pub fn read(socket: &str) -> Result<Self, String> {
        Self::from_records(
            AudioEngine::read_validated_local_catalog_at(socket)
                .map_err(|_| INVALID.to_string())?,
        )
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Confirmed,
    Partial,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq, Default)]
pub(crate) struct Counts {
    pub source: String,
    pub audio_files: usize,
    pub cue_groups: usize,
    pub cataloged_audio: usize,
    pub virtual_tracks: usize,
    pub omitted: usize,
    pub policy_excluded: usize,
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Report {
    pub outcome: Outcome,
    pub generation: String,
    pub scope: &'static str,
    pub sources: Vec<Counts>,
}
/// This comparison has no filesystem/IPC calls. It never infers corruption.
pub(crate) fn compare(
    inventory: &Inventory,
    before: &Catalog,
    previous_ids: &BTreeSet<String>,
    after: &Catalog,
) -> Result<Report, String> {
    for record in &before.records {
        let uri = record.split_once(": ").ok_or(INVALID)?.1;
        let id = uri.split('/').next().ok_or(INVALID)?;
        if !previous_ids.contains(id) {
            return Err(INVALID.into());
        }
    }
    for record in &after.records {
        let uri = record.split_once(": ").ok_or(INVALID)?.1;
        let id = uri.split('/').next().ok_or(INVALID)?;
        if !inventory.roots.contains_key(id) {
            return Err(INVALID.into());
        }
    }
    let mut covered = BTreeSet::new();
    let mut virtual_counts: BTreeMap<String, usize> = BTreeMap::new();
    for uri in &after.files {
        if !inventory.observes(uri) {
            continue;
        }
        if inventory
            .selected
            .get(uri)
            .is_some_and(|s| s.kind == Kind::Audio)
        {
            covered.insert(uri.clone());
        } else if let Some(parent) = cue_parent(uri).filter(|p| {
            inventory
                .selected
                .get(*p)
                .is_some_and(|s| s.kind == Kind::Cue)
        }) {
            covered.insert(parent.into());
            *virtual_counts.entry(parent.into()).or_default() += 1;
        } else {
            return Err(INVALID.into());
        }
    }
    for uri in &before.files {
        let id = uri.split('/').next().ok_or(INVALID)?;
        if !inventory.roots.contains_key(id) {
            continue;
        } // Explicit retired source, never rebound.
        let selected = inventory.selected.contains_key(uri)
            || cue_parent(uri).is_some_and(|p| inventory.selected.contains_key(p));
        // Full inventory positively proves physical/CUE policy removals. Narrow
        // dirty scopes preserve all prior records outside the observed paths.
        if (selected || !inventory.observes(uri)) && !after.files.contains(uri) {
            return Err(LOST.into());
        }
    }
    let mut counts: BTreeMap<String, Counts> = inventory
        .roots
        .keys()
        .map(|id| {
            (
                id.clone(),
                Counts {
                    source: id.clone(),
                    policy_excluded: *inventory.excluded.get(id).unwrap_or(&0),
                    omitted: *inventory.unaddressable.get(id).unwrap_or(&0),
                    ..Default::default()
                },
            )
        })
        .collect();
    for (uri, selected) in &inventory.selected {
        let id = uri.split('/').next().ok_or(INVALID)?;
        let c = counts.get_mut(id).ok_or(INVALID)?;
        match selected.kind {
            Kind::Audio => {
                c.audio_files += 1;
                if covered.contains(uri) {
                    c.cataloged_audio += 1
                }
            }
            Kind::Cue => {
                c.cue_groups += 1;
                c.virtual_tracks += virtual_counts.get(uri).copied().unwrap_or(0)
            }
        }
        if !covered.contains(uri) {
            c.omitted += 1
        }
    }
    let sources: Vec<_> = counts.into_values().collect();
    Ok(Report {
        outcome: if sources.iter().any(|c| c.omitted > 0) {
            Outcome::Partial
        } else {
            Outcome::Confirmed
        },
        generation: inventory.generation.clone(),
        scope: if inventory.scope.is_some() {
            "paths"
        } else {
            "full"
        },
        sources,
    })
}
pub(crate) struct Check {
    inventory: Inventory,
    before: Catalog,
    previous_ids: BTreeSet<String>,
}
impl Check {
    pub(crate) fn previous_records(&self)->std::collections::HashSet<String>{self.before.records.clone()}

    pub fn prepare(
        inventory: Inventory,
        socket: &str,
        previous_ids: BTreeSet<String>,
    ) -> Result<Self, String> {
        inventory.verify()?;
        let before = Catalog::read(socket)?;
        Ok(Self {
            inventory,
            before,
            previous_ids,
        })
    }
    pub fn from_catalog(
        inventory: Inventory,
        before: Catalog,
        previous_ids: BTreeSet<String>,
    ) -> Result<Self, String> {
        inventory.verify()?;
        Ok(Self {
            inventory,
            before,
            previous_ids,
        })
    }
    pub fn finish_at(&self, socket: &str, library: &Path) -> Result<(Report, Catalog), String> {
        if self.inventory.generation != "legacy-dirty" {
            let manifest =
                local_media::mirror::read_manifest(library).map_err(|_| CHANGED.to_string())?;
            let sources = self
                .inventory
                .roots
                .iter()
                .map(|(id, (root, _, _))| (id.clone(), root.clone()))
                .collect();
            if manifest.generation_id != self.inventory.generation || manifest.sources != sources {
                return Err(CHANGED.into());
            }
            crate::source_identity::verify_if_initialized(library, &sources)
                .map_err(|_| CHANGED.to_string())?;
        }
        self.finish(socket)
    }
    pub fn verify_recovered(&self, socket: &str) -> Result<(), String> {
        let after = Catalog::read(socket)?;
        for record in &after.records {
            let uri = record.split_once(": ").ok_or(INVALID)?.1;
            if !self
                .previous_ids
                .contains(uri.split('/').next().ok_or(INVALID)?)
            {
                return Err("LOCAL_INDEX_RECOVERY_UNCONFIRMED".into());
            }
        }
        if !self.before.files.is_subset(&after.files) {
            return Err("LOCAL_INDEX_RECOVERY_UNCONFIRMED".into());
        }
        Ok(())
    }
    pub fn finish(&self, socket: &str) -> Result<(Report, Catalog), String> {
        let after = Catalog::read(socket)?;
        self.inventory.verify()?;
        Ok((
            compare(&self.inventory, &self.before, &self.previous_ids, &after)?,
            after,
        ))
    }
}
#[derive(Clone, Debug, Serialize, Default)]
pub(crate) struct Snapshot {
    pub revision: u64,
    pub report: Option<Report>,
}
#[derive(Default)]
struct Session {
    next: u64,
    active: u64,
    binding: Vec<String>,
    snapshot: Snapshot,
}
#[derive(Default)]
pub(crate) struct State(Mutex<Session>);
impl State {
    pub fn begin(&self) -> Result<u64, String> {
        let mut s = self.0.lock().map_err(|_| INVALID)?;
        s.next = s.next.checked_add(1).ok_or(INVALID)?;
        s.active = s.next;
        Ok(s.active)
    }
    pub fn publish(&self, token: u64, folders: &[String], report: Report) -> Option<Snapshot> {
        let mut s = self.0.lock().ok()?;
        if s.active != token {
            return None;
        }
        s.binding = folders.to_vec();
        s.snapshot = Snapshot {
            revision: token,
            report: Some(report),
        };
        Some(s.snapshot.clone())
    }
    pub fn inherit(&self, token: u64, folders: &[String]) -> Option<Snapshot> {
        let mut s = self.0.lock().ok()?;
        if s.active != token || s.binding == folders {
            return None;
        }
        s.binding = folders.to_vec();
        s.snapshot = Snapshot {
            revision: token,
            report: None,
        };
        Some(s.snapshot.clone())
    }
    pub fn snapshot(&self, folders: &[String]) -> Snapshot {
        self.0
            .lock()
            .map(|s| {
                if s.binding == folders {
                    s.snapshot.clone()
                } else {
                    Snapshot {
                        revision: s.next,
                        report: None,
                    }
                }
            })
            .unwrap_or_default()
    }
}
#[cfg(test)]
pub(crate) mod tests;
