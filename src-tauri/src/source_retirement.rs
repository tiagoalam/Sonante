//! Source-level retirement. Survivors stay at the same path and inode.
//! Private journal bytes never cross IPC; observations carry only finite data.
use crate::{
    config::AppConfig,
    local_media::mirror::{MirrorManifest, MANIFEST_NAME},
    profile_lease::Lease,
    recovery_fs::{self, Dir, Identity, CHANGED, INCOMPLETE, UNSAFE},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::Read,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
};

pub(crate) const PENDING: &str = ".sonante-source-retirement.pending";
const PREPARING: &str = ".sonante-source-retirement.preparing";
const LAST: &str = ".sonante-source-retirement.last";
const HISTORY: &str = ".sonante-source-retirement.history";
const LIMIT: u64 = 128 * 1024 * 1024;
pub(crate) const BLOCKED: &str = "LOCAL_SOURCE_RETIREMENT_PENDING";
type Validator<'a> = dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String> + 'a;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileProof {
    id: Identity,
    hash: String,
    bytes: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    id: String,
    profile: Identity,
    library: Identity,
    creator: u32,
    start: String,
    scope: String,
    old: MirrorManifest,
    next: MirrorManifest,
    roots: BTreeMap<String, Identity>,
    removed: BTreeMap<String, String>,
    controls: BTreeMap<String, Option<FileProof>>,
    backups: BTreeMap<String, FileProof>,
    swaps: BTreeMap<String, FileProof>,
    before: BTreeSet<String>,
    after: BTreeSet<String>,
    ledger: String,
    anchor: String,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Completion {
    plan: String,
    restored: bool,
    controls: BTreeMap<String, Option<FileProof>>,
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|_| INCOMPLETE.into())
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|_| INCOMPLETE.into())
}
fn bytes(dir: &Dir, name: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    dir.read(name, LIMIT, cancel)?.ok_or(INCOMPLETE.into())
}
fn proof(dir: &Dir, name: &str, cancel: &dyn Fn() -> bool) -> Result<Option<FileProof>, String> {
    let Some(data) = dir.read(name, LIMIT, cancel)? else {
        return Ok(None);
    };
    Ok(Some(FileProof {
        id: dir.file_identity(name)?.ok_or(CHANGED)?,
        hash: recovery_fs::hash(&data),
        bytes: data.len() as u64,
    }))
}
fn id_is_safe(id: &str) -> bool {
    crate::local_media::is_safe_relative_mpd_uri(id) && Path::new(id).components().count() == 1
}
fn names(dir: &Dir) -> Result<Vec<String>, String> {
    fs::read_dir(format!("/proc/self/fd/{}", dir.file.as_raw_fd()))
        .map_err(|_| UNSAFE)?
        .map(|e| {
            e.map_err(|_| UNSAFE.into())
                .and_then(|e| e.file_name().into_string().map_err(|_| UNSAFE.into()))
        })
        .collect()
}
fn source_roots(
    library: &Dir,
    manifest: &MirrorManifest,
) -> Result<BTreeMap<String, Identity>, String> {
    let mut roots = BTreeMap::new();
    for name in names(library)? {
        if name == MANIFEST_NAME {
            continue;
        }
        if !manifest.sources.contains_key(&name) || !id_is_safe(&name) {
            return Err(UNSAFE.into());
        }
        roots.insert(name.clone(), library.child(&name)?.id());
    }
    if roots.len() != manifest.sources.len() {
        return Err(CHANGED.into());
    }
    Ok(roots)
}
fn append(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}
// Hash only the selected virtual subtree. No physical music file is opened,
// canonicalized or stat'ed, and no surviving subtree is walked.
fn tree_hash(dir: &Dir, physical: &Path, cancel: &dyn Fn() -> bool) -> Result<String, String> {
    fn walk(
        dir: &Dir,
        physical: &Path,
        prefix: &Path,
        depth: usize,
        digest: &mut Sha256,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        if depth > 128 {
            return Err(UNSAFE.into());
        }
        let mut entries = names(dir)?;
        entries.sort();
        for name in entries {
            recovery_fs::check(cancel)?;
            let path = dir.entry(&name)?;
            let relative = prefix.join(&name);
            let first = fs::symlink_metadata(&path).map_err(|_| CHANGED)?;
            if first.uid() != unsafe { libc::geteuid() } {
                return Err(UNSAFE.into());
            }
            append(digest, relative.to_str().ok_or(UNSAFE)?.as_bytes());
            append(digest, &first.dev().to_le_bytes());
            append(digest, &first.ino().to_le_bytes());
            if first.is_dir() {
                append(digest, b"directory");
                walk(
                    &dir.child(&name)?,
                    physical,
                    &relative,
                    depth + 1,
                    digest,
                    cancel,
                )?;
            } else if first.file_type().is_symlink() {
                let target = fs::read_link(&path).map_err(|_| CHANGED)?;
                let rel = target.strip_prefix(physical).map_err(|_| UNSAFE)?;
                if !target.is_absolute()
                    || rel.as_os_str().is_empty()
                    || rel
                        .components()
                        .any(|c| !matches!(c, std::path::Component::Normal(_)))
                    || crate::local_media::classify_path(&target)
                        == crate::local_media::LocalMediaClass::Ignored
                {
                    return Err(UNSAFE.into());
                }
                append(digest, b"link");
                append(digest, target.to_str().ok_or(UNSAFE)?.as_bytes());
            } else {
                return Err(UNSAFE.into());
            }
            let last = fs::symlink_metadata(&path).map_err(|_| CHANGED)?;
            if (
                first.dev(),
                first.ino(),
                first.ctime(),
                first.ctime_nsec(),
                first.mode(),
                first.len(),
            ) != (
                last.dev(),
                last.ino(),
                last.ctime(),
                last.ctime_nsec(),
                last.mode(),
                last.len(),
            ) {
                return Err(CHANGED.into());
            }
        }
        dir.validate()
    }
    let mut digest = Sha256::new();
    walk(dir, physical, Path::new(""), 0, &mut digest, cancel)?;
    Ok(format!("{:x}", digest.finalize()))
}
fn catalog_after(
    before: &HashSet<String>,
    old: &MirrorManifest,
    next: &MirrorManifest,
) -> Result<BTreeSet<String>, String> {
    let mut result = BTreeSet::new();
    for record in before {
        let (kind, uri) = record.split_once(": ").ok_or(INCOMPLETE)?;
        if !matches!(kind, "file" | "directory" | "playlist")
            || !crate::local_media::is_safe_relative_mpd_uri(uri)
        {
            return Err(INCOMPLETE.into());
        }
        let id = uri.split('/').next().ok_or(INCOMPLETE)?;
        if !old.sources.contains_key(id) {
            return Err(INCOMPLETE.into());
        }
        if next.sources.contains_key(id) {
            result.insert(record.clone());
        }
    }
    Ok(result)
}
fn record_id(record: &Record) -> Result<String, String> {
    let mut value = record.clone();
    value.id.clear();
    Ok(format!(
        "retirement-v1-{}",
        recovery_fs::hash(&encode(&value)?)
    ))
}
fn validate_record(record: &Record) -> Result<(), String> {
    let controls = [
        "config.json",
        "mpd-v2.db",
        "mpd.conf",
        "queue_cache.json",
        "manifest",
    ];
    let swaps = ["config-swap", "manifest-swap", "db-restore"];
    let backup_names = [
        "config-old",
        "db-old",
        "manifest-old",
        "config-proposed",
        "manifest-proposed",
        "queue-old",
    ];
    if record.controls.len() != controls.len()
        || controls.iter().any(|n| !record.controls.contains_key(*n))
        || record.swaps.len() != swaps.len()
        || swaps.iter().any(|n| !record.swaps.contains_key(*n))
        || record
            .backups
            .keys()
            .any(|n| !backup_names.contains(&n.as_str()))
        || backup_names[..5]
            .iter()
            .any(|n| !record.backups.contains_key(*n))
        || record.roots.keys().ne(record.old.sources.keys())
    {
        return Err(INCOMPLETE.into());
    }
    if record.version != 1
        || record.id != record_id(record)?
        || record.old.registry_id.is_none()
        || record.old.registry_id != record.next.registry_id
        || record.removed.is_empty()
        || record.next.sources.len() + record.removed.len() != record.old.sources.len()
        || record
            .next
            .sources
            .iter()
            .any(|(id, root)| record.old.sources.get(id) != Some(root))
        || record.old.sources.keys().any(|id| {
            !id_is_safe(id)
                || (!record.next.sources.contains_key(id) && !record.removed.contains_key(id))
        })
    {
        return Err(INCOMPLETE.into());
    }
    crate::local_media::mirror::validate_manifest(&record.old)?;
    crate::local_media::mirror::validate_manifest(&record.next)?;
    if catalog_after(
        &record.before.iter().cloned().collect(),
        &record.old,
        &record.next,
    )? != record.after
    {
        return Err(INCOMPLETE.into());
    }
    Ok(())
}
// Check only configured source roots, never their music contents.
fn config_matches_manifest(config: &AppConfig, manifest: &MirrorManifest) -> Result<(), String> {
    let roots = crate::local_media::mirror::canonical_configured_roots(
        &config
            .local_folders
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
    )
    .map_err(|_| CHANGED)?
    .into_iter()
    .collect::<BTreeSet<_>>();
    if roots != manifest.sources.values().cloned().collect() {
        return Err(CHANGED.into());
    }
    Ok(())
}
fn verify_backups(dir: &Dir, record: &Record, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    for (name, want) in &record.backups {
        if proof(dir, name, cancel)?.as_ref() != Some(want) {
            return Err(CHANGED.into());
        }
    }
    let old_cfg: AppConfig = decode(&bytes(dir, "config-old", cancel)?)?;
    let next_cfg: AppConfig = decode(&bytes(dir, "config-proposed", cancel)?)?;
    config_matches_manifest(&old_cfg, &record.old)?;
    config_matches_manifest(&next_cfg, &record.next)?;
    let mut proposal = old_cfg;
    proposal.local_folders = next_cfg.local_folders.clone();
    if proposal != next_cfg
        || decode::<MirrorManifest>(&bytes(dir, "manifest-old", cancel)?)? != record.old
        || decode::<MirrorManifest>(&bytes(dir, "manifest-proposed", cancel)?)? != record.next
    {
        return Err(INCOMPLETE.into());
    }
    for (control, copy) in [
        ("config.json", "config-old"),
        ("mpd-v2.db", "db-old"),
        ("manifest", "manifest-old"),
    ] {
        if record.controls[control]
            .as_ref()
            .is_none_or(|p| p.hash != record.backups[copy].hash)
        {
            return Err(INCOMPLETE.into());
        }
    }
    for (swap, copy) in [
        ("config-swap", "config-proposed"),
        ("manifest-swap", "manifest-proposed"),
        ("db-restore", "db-old"),
    ] {
        if record.swaps[swap].hash != record.backups[copy].hash {
            return Err(INCOMPLETE.into());
        }
    }
    Ok(())
}
fn verify_ledger(root: &Dir, record: &Record, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    if recovery_fs::hash(&bytes(root, "local-source-identities.json", cancel)?) != record.ledger
        || recovery_fs::hash(&bytes(root, "local-source-identities.lock", cancel)?) != record.anchor
    {
        return Err(CHANGED.into());
    }
    crate::source_identity::check_manifest(
        &root.path.join("library"),
        record.old.registry_id.as_deref().ok_or(INCOMPLETE)?,
        &record.old.sources,
    )?;
    crate::source_identity::verify_origins(&root.path.join("library"), &record.old.sources)
}
fn topology(
    root: &Dir,
    dir: &Dir,
    record: &Record,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let library = root.child("library")?;
    if library.id() != record.library {
        return Err(CHANGED.into());
    }
    let mut allowed = record.old.sources.keys().cloned().collect::<BTreeSet<_>>();
    allowed.insert(MANIFEST_NAME.into());
    if names(&library)?.iter().any(|name| !allowed.contains(name)) {
        return Err(UNSAFE.into());
    }
    let quarantine = dir.child("sources")?;
    if names(&quarantine)?
        .iter()
        .any(|id| !record.removed.contains_key(id))
    {
        return Err(UNSAFE.into());
    }
    for (id, identity) in &record.roots {
        recovery_fs::check(cancel)?;
        if record.removed.contains_key(id) {
            let live = library.exists(id)?;
            let held = quarantine.exists(id)?;
            if live == held {
                return Err(CHANGED.into());
            }
            let source = if live {
                library.child(id)?
            } else {
                quarantine.child(id)?
            };
            if source.id() != *identity
                || tree_hash(&source, &record.old.sources[id], cancel)? != record.removed[id]
            {
                return Err(CHANGED.into());
            }
        } else if library.child(id)?.id() != *identity {
            return Err(CHANGED.into());
        }
    }
    verify_ledger(root, record, cancel)
}
fn read_record(root: &Dir, cancel: &dyn Fn() -> bool) -> Result<(Dir, Record), String> {
    for name in [
        crate::managed_save_intent::NAME,
        crate::recovery_set::PENDING,
        crate::first_run_managed::PENDING,
        crate::recovery_set::SEAL,
        "library.next",
    ] {
        if root.exists(name)? {
            return Err(INCOMPLETE.into());
        }
    }
    let dir = root.child(PENDING)?;
    let record: Record = decode(&bytes(&dir, "record.json", cancel)?)?;
    validate_record(&record)?;
    if root.id() != record.profile {
        return Err(CHANGED.into());
    }
    verify_backups(&dir, &record, cancel)?;
    topology(root, &dir, &record, cancel)?;
    Ok((dir, record))
}
fn current_controls(
    root: &Dir,
    cancel: &dyn Fn() -> bool,
) -> Result<BTreeMap<String, Option<FileProof>>, String> {
    ["config.json", "mpd-v2.db", "queue_cache.json", "mpd.conf"]
        .into_iter()
        .map(|name| Ok((name.into(), proof(root, name, cancel)?)))
        .collect()
}
fn exchange(
    left: &Dir,
    name: &str,
    want: &FileProof,
    right: &Dir,
    staged: &str,
    with: &FileProof,
) -> Result<(), String> {
    if proof(left, name, &|| false)?.as_ref() != Some(want)
        || proof(right, staged, &|| false)?.as_ref() != Some(with)
    {
        return Err(CHANGED.into());
    }
    crate::local_media::mirror::atomic_exchange(&left.entry(name)?, &right.entry(staged)?)?;
    left.sync()?;
    right.sync()?;
    if proof(left, name, &|| false)?.as_ref() != Some(with)
        || proof(right, staged, &|| false)?.as_ref() != Some(want)
    {
        return Err(CHANGED.into());
    }
    Ok(())
}
pub(crate) struct Transaction {
    dir: Dir,
    record: Record,
    record_proof: FileProof,
}
impl Transaction {
    fn reopen(root: &Dir, cancel: &dyn Fn() -> bool) -> Result<Self, String> {
        let (dir, record) = read_record(root, cancel)?;
        let record_proof = proof(&dir, "record.json", cancel)?.ok_or(INCOMPLETE)?;
        Ok(Self {
            dir,
            record,
            record_proof,
        })
    }
    fn validate(&self, owner: &Lease, cancel: &dyn Fn() -> bool) -> Result<(), String> {
        owner.validate()?;
        if owner.root().id() != self.record.profile
            || proof(&self.dir, "record.json", cancel)?.as_ref() != Some(&self.record_proof)
        {
            return Err(CHANGED.into());
        }
        verify_backups(&self.dir, &self.record, cancel)?;
        topology(owner.root(), &self.dir, &self.record, cancel)
    }
    pub(crate) fn removed_ids(&self) -> Vec<String> {
        self.record.removed.keys().cloned().collect()
    }
    pub(crate) fn after(&self) -> HashSet<String> {
        self.record.after.iter().cloned().collect()
    }
    pub(crate) fn before(&self) -> HashSet<String> {
        self.record.before.iter().cloned().collect()
    }
    pub(crate) fn validate_before_effect(
        &self,
        owner: &Lease,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.validate(owner, cancel)
    }
    pub(crate) fn publish(
        &self,
        owner: &Lease,
        cancel: &dyn Fn() -> bool,
        stage: &dyn Fn(&'static str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.validate(owner, cancel)?;
        let mut controls = current_controls(owner.root(), cancel)?;
        controls.insert(
            "manifest".into(),
            proof(&owner.root().child("library")?, MANIFEST_NAME, cancel)?,
        );
        let mut expected = self.record.controls.clone();
        if self.dir.exists("queue-swap-proof.json")? {
            expected.insert(
                "queue_cache.json".into(),
                Some(decode::<FileProof>(&bytes(
                    &self.dir,
                    "queue-swap-proof.json",
                    cancel,
                )?)?),
            );
        }
        if controls != expected {
            return Err(CHANGED.into());
        }
        let library = owner.root().child("library")?;
        let quarantine = self.dir.child("sources")?;
        for id in self.record.removed.keys() {
            recovery_fs::check(cancel)?;
            library.rename(id, &quarantine, id, true)?;
            stage("source_moved")?;
        }
        self.validate(owner, cancel)?;
        recovery_fs::check(cancel)?;
        exchange(
            &library,
            MANIFEST_NAME,
            &self.record.controls["manifest"].clone().ok_or(INCOMPLETE)?,
            &self.dir,
            "manifest-swap",
            &self.record.swaps["manifest-swap"],
        )?;
        stage("manifest_published")
    }
    pub(crate) fn persist_queue_cache(&self, owner: &Lease, data: &[u8]) -> Result<(), String> {
        self.validate(owner, &|| false)?;
        let old = self
            .record
            .controls
            .get("queue_cache.json")
            .and_then(Option::as_ref)
            .ok_or(INCOMPLETE)?;
        self.dir.write_new("queue-swap", data)?;
        let next = proof(&self.dir, "queue-swap", &|| false)?.ok_or(INCOMPLETE)?;
        self.dir
            .write_new("queue-swap-proof.json", &encode(&next)?)?;
        exchange(
            owner.root(),
            "queue_cache.json",
            old,
            &self.dir,
            "queue-swap",
            &next,
        )
    }
    pub(crate) fn commit(
        &self,
        owner: &Lease,
        cancel: &dyn Fn() -> bool,
        validate: &Validator<'_>,
        stage: &dyn Fn(&'static str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.validate(owner, cancel)?;
        let root = owner.root();
        let library = root.child("library")?;
        if proof(&library, MANIFEST_NAME, cancel)?.as_ref()
            != Some(&self.record.swaps["manifest-swap"])
        {
            return Err(CHANGED.into());
        }
        let db = bytes(root, "mpd-v2.db", cancel)?;
        if validate(&db, cancel)? != self.after() {
            return Err("LOCAL_SOURCE_RETIREMENT_CATALOG_MISMATCH".into());
        }
        let db_proof = proof(root, "mpd-v2.db", cancel)?.ok_or(INCOMPLETE)?;
        if recovery_fs::hash(&db) != db_proof.hash {
            return Err(CHANGED.into());
        }
        self.dir
            .write_new("confirmed-db.json", &encode(&db_proof)?)?;
        stage("catalog_confirmed")?;
        self.validate(owner, cancel)?;
        if proof(root, "mpd-v2.db", cancel)?.as_ref() != Some(&db_proof) {
            return Err(CHANGED.into());
        }
        exchange(
            root,
            "config.json",
            &self.record.controls["config.json"]
                .clone()
                .ok_or(INCOMPLETE)?,
            &self.dir,
            "config-swap",
            &self.record.swaps["config-swap"],
        )?;
        stage("config_committed")?;
        self.finish(owner, false, cancel)?;
        let _ = stage("finalized");
        Ok(())
    }
    fn finish(
        &self,
        owner: &Lease,
        restored: bool,
        cancel: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        self.validate(owner, cancel)?;
        let root = owner.root();
        let library = root.child("library")?;
        let manifest = if restored {
            self.record.controls["manifest"]
                .as_ref()
                .ok_or(INCOMPLETE)?
        } else {
            &self.record.swaps["manifest-swap"]
        };
        if proof(&library, MANIFEST_NAME, cancel)?.as_ref() != Some(manifest) {
            return Err(CHANGED.into());
        }
        for id in self.record.removed.keys() {
            if library.exists(id)? != restored {
                return Err(CHANGED.into());
            }
        }
        let controls = current_controls(root, cancel)?;
        let config = if restored {
            self.record.controls["config.json"]
                .as_ref()
                .ok_or(INCOMPLETE)?
        } else {
            &self.record.swaps["config-swap"]
        };
        let database = if restored {
            self.record.swaps["db-restore"].clone()
        } else {
            decode::<FileProof>(&bytes(&self.dir, "confirmed-db.json", cancel)?)?
        };
        let queue = if !restored && self.dir.exists("queue-swap-proof.json")? {
            Some(decode::<FileProof>(&bytes(
                &self.dir,
                "queue-swap-proof.json",
                cancel,
            )?)?)
        } else {
            self.record.controls["queue_cache.json"].clone()
        };
        if controls["config.json"].as_ref() != Some(config)
            || controls["mpd-v2.db"].as_ref() != Some(&database)
            || controls["queue_cache.json"] != queue
            || controls["mpd.conf"] != self.record.controls["mpd.conf"]
        {
            return Err(CHANGED.into());
        }
        let completion = Completion {
            plan: self.record.id.clone(),
            restored,
            controls,
        };
        let completion_name = if restored {
            "restore-complete.json"
        } else {
            "complete.json"
        };
        if self.dir.exists(completion_name)? {
            if decode::<Completion>(&bytes(&self.dir, completion_name, cancel)?)? != completion {
                return Err(CHANGED.into());
            }
        } else {
            self.dir.write_new(completion_name, &encode(&completion)?)?;
        }
        if root.exists(LAST)? {
            let old = root.child(LAST)?;
            let old_record: Record = decode(&bytes(&old, "record.json", cancel)?)?;
            validate_record(&old_record)?;
            let history = if root.exists(HISTORY)? {
                root.child(HISTORY)?
            } else {
                root.create(HISTORY)?
            };
            root.rename(LAST, &history, &old_record.id, true)?;
        }
        owner.validate()?;
        root.rename(PENDING, root, LAST, true)
    }
    // Caller must have stopped its owned runtime. Startup recovery additionally
    // binds an offer and the legacy process fence before reaching this method.
    fn restore(
        &self,
        owner: &Lease,
        cancel: &dyn Fn() -> bool,
        validate: &Validator<'_>,
        stage: &dyn Fn(&'static str) -> Result<(), String>,
    ) -> Result<(), String> {
        self.validate(owner, cancel)?;
        let root = owner.root();
        let library = root.child("library")?;
        let old_db = bytes(&self.dir, "db-old", cancel)?;
        if validate(&old_db, cancel)? != self.record.before.iter().cloned().collect() {
            return Err("LOCAL_SOURCE_RETIREMENT_PREVIOUS_DB_UNCONFIRMED".into());
        }
        stage("previous_database_validated")?;
        // Revalidate before each independently resumable boundary.
        for (dir, name, swap, want) in [
            (
                &library,
                MANIFEST_NAME,
                "manifest-swap",
                self.record.controls["manifest"]
                    .as_ref()
                    .ok_or(INCOMPLETE)?,
            ),
            (
                root,
                "config.json",
                "config-swap",
                self.record.controls["config.json"]
                    .as_ref()
                    .ok_or(INCOMPLETE)?,
            ),
        ] {
            self.validate(owner, cancel)?;
            let current = proof(dir, name, cancel)?.ok_or(INCOMPLETE)?;
            if current != *want {
                let new = &self.record.swaps[swap];
                if current != *new {
                    return Err(CHANGED.into());
                }
                exchange(dir, name, new, &self.dir, swap, want)?;
            }
            stage("control_restored")?;
        }
        self.validate(owner, cancel)?;
        let quarantine = self.dir.child("sources")?;
        for id in self.record.removed.keys() {
            if quarantine.exists(id)? {
                quarantine.rename(id, &library, id, true)?;
            }
            stage("source_restored")?;
        }
        self.validate(owner, cancel)?;
        let current = proof(root, "mpd-v2.db", cancel)?.ok_or(INCOMPLETE)?;
        let restore = &self.record.swaps["db-restore"];
        if current != *restore {
            exchange(
                root,
                "mpd-v2.db",
                &current,
                &self.dir,
                "db-restore",
                restore,
            )?;
        }
        stage("database_restored")?;
        if self.dir.exists("queue-swap-proof.json")? {
            let new: FileProof = decode(&bytes(&self.dir, "queue-swap-proof.json", cancel)?)?;
            let old = self.record.controls["queue_cache.json"]
                .as_ref()
                .ok_or(INCOMPLETE)?;
            let current = proof(root, "queue_cache.json", cancel)?.ok_or(INCOMPLETE)?;
            if current != *old {
                if current != new {
                    return Err(CHANGED.into());
                }
                exchange(root, "queue_cache.json", &new, &self.dir, "queue-swap", old)?;
            }
        }
        stage("queue_restored")?;
        self.finish(owner, true, cancel)?;
        let _ = stage("finalized");
        Ok(())
    }
}

pub(crate) fn prepare(
    owner: &Lease,
    previous: &AppConfig,
    next: &AppConfig,
    before: HashSet<String>,
    cancel: &dyn Fn() -> bool,
    validate: &Validator<'_>,
) -> Result<Transaction, String> {
    owner.validate()?;
    let root = owner.root();
    crate::managed_save_intent::check_startup(&root.path)?;
    if root.exists(PENDING)? {
        return Err(BLOCKED.into());
    }
    let library = root.child("library")?;
    let old_bytes = bytes(&library, MANIFEST_NAME, cancel)?;
    let old: MirrorManifest = decode(&old_bytes)?;
    crate::local_media::mirror::validate_manifest(&old)?;
    let namespace = old.registry_id.as_deref().ok_or(INCOMPLETE)?;
    crate::source_identity::check_manifest(&root.path.join("library"), namespace, &old.sources)?;
    crate::source_identity::verify_origins(&root.path.join("library"), &old.sources)?;
    let mut proposed = previous.clone();
    proposed.local_folders = next.local_folders.clone();
    if proposed != *next {
        return Err(UNSAFE.into());
    }
    config_matches_manifest(previous, &old)?;
    let consent = crate::local_media::mirror::source_removal_consent(
        &old.sources,
        &next
            .local_folders
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
    )?
    .ok_or(UNSAFE)?;
    let manifest_proof = proof(&library, MANIFEST_NAME, cancel)?.ok_or(INCOMPLETE)?;
    if manifest_proof.hash != recovery_fs::hash(&old_bytes) {
        return Err(CHANGED.into());
    }
    let roots = source_roots(&library, &old)?;
    let mut removed = BTreeMap::new();
    for id in &consent.removed_ids {
        removed.insert(
            id.clone(),
            tree_hash(&library.child(id)?, &old.sources[id], cancel)?,
        );
    }
    let mut random = [0u8; 16];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut random))
        .map_err(|_| INCOMPLETE)?;
    let mut next_manifest = old.clone();
    next_manifest.generation_id = format!("retirement-{}", recovery_fs::hash(&random));
    next_manifest
        .sources
        .retain(|id, _| !removed.contains_key(id));
    let raw = current_controls(root, cancel)?;
    let old_cfg = bytes(root, "config.json", cancel)?;
    if decode::<AppConfig>(&old_cfg)? != *previous {
        return Err(CHANGED.into());
    }
    let old_db = bytes(root, "mpd-v2.db", cancel)?;
    if validate(&old_db, cancel)? != before {
        return Err("LOCAL_SOURCE_RETIREMENT_PREVIOUS_DB_UNCONFIRMED".into());
    }
    if current_controls(root, cancel)? != raw {
        return Err(CHANGED.into());
    }
    if raw["mpd-v2.db"]
        .as_ref()
        .is_none_or(|p| p.hash != recovery_fs::hash(&old_db))
    {
        return Err(CHANGED.into());
    }
    let mut controls = raw;
    controls.insert("manifest".into(), Some(manifest_proof));
    let mut record = Record {
        version: 1,
        id: String::new(),
        profile: root.id(),
        library: library.id(),
        creator: std::process::id(),
        start: crate::profile_lease::creator_start()?,
        scope: crate::profile_lease::process_scope()?,
        old: old.clone(),
        next: next_manifest.clone(),
        roots,
        removed,
        controls,
        backups: BTreeMap::new(),
        swaps: BTreeMap::new(),
        before: before.iter().cloned().collect(),
        after: catalog_after(&before, &old, &next_manifest)?,
        ledger: recovery_fs::hash(&bytes(root, "local-source-identities.json", cancel)?),
        anchor: recovery_fs::hash(&bytes(root, "local-source-identities.lock", cancel)?),
    };
    let preparing = format!("{PREPARING}-{}", recovery_fs::hash(&random));
    let dir = root.create(&preparing)?;
    dir.create("sources")?;
    for (name, data) in [
        ("config-old", old_cfg),
        ("manifest-old", old_bytes),
        ("db-old", old_db.clone()),
        ("config-proposed", encode(next)?),
        ("manifest-proposed", encode(&next_manifest)?),
    ] {
        dir.write_new(name, &data)?;
        record
            .backups
            .insert(name.into(), proof(&dir, name, cancel)?.ok_or(INCOMPLETE)?);
    }
    for (name, data) in [
        ("config-swap", encode(next)?),
        ("manifest-swap", encode(&next_manifest)?),
        ("db-restore", old_db),
    ] {
        dir.write_new(name, &data)?;
        record
            .swaps
            .insert(name.into(), proof(&dir, name, cancel)?.ok_or(INCOMPLETE)?);
    }
    if let Some(queue) = root.read("queue_cache.json", LIMIT, cancel)? {
        dir.write_new("queue-old", &queue)?;
        record.backups.insert(
            "queue-old".into(),
            proof(&dir, "queue-old", cancel)?.ok_or(INCOMPLETE)?,
        );
        if record.controls["queue_cache.json"]
            .as_ref()
            .is_none_or(|old| old.hash != recovery_fs::hash(&queue))
        {
            return Err(CHANGED.into());
        }
    }
    record.id = record_id(&record)?;
    validate_record(&record)?;
    dir.write_new("record.json", &encode(&record)?)?;
    dir.sync()?;
    if current_controls(root, cancel)?
        != record
            .controls
            .iter()
            .filter(|(n, _)| n.as_str() != "manifest")
            .map(|(n, p)| (n.clone(), p.clone()))
            .collect()
    {
        return Err(CHANGED.into());
    }
    verify_backups(&dir, &record, cancel)?;
    topology(root, &dir, &record, cancel)?;
    if proof(&library, MANIFEST_NAME, cancel)? != record.controls["manifest"] {
        return Err(CHANGED.into());
    }
    root.rename(&preparing, root, PENDING, true)?;
    Transaction::reopen(root, cancel)
}

pub(crate) struct Offer {
    profile: Identity,
    journal: Identity,
    id: String,
    view: String,
    plan: crate::recovery_set::Plan,
    analysis_epoch: u64,
    config_match: crate::library_recovery::ConfigMatch,
}
fn view(
    root: &Dir,
    dir: &Dir,
    record: &Record,
    cancel: &dyn Fn() -> bool,
) -> Result<String, String> {
    topology(root, dir, record, cancel)?;
    verify_backups(dir, record, cancel)?;
    let library = root.child("library")?;
    let mut files = current_controls(root, cancel)?;
    files.insert("manifest".into(), proof(&library, MANIFEST_NAME, cancel)?);
    if files["mpd.conf"] != record.controls["mpd.conf"] {
        return Err(CHANGED.into());
    }
    // Neither an old nor a proposed config can be silently replaced by a third.
    for (name, old, new) in [
        (
            "config.json",
            record.controls["config.json"].as_ref().ok_or(INCOMPLETE)?,
            &record.swaps["config-swap"],
        ),
        (
            "manifest",
            record.controls["manifest"].as_ref().ok_or(INCOMPLETE)?,
            &record.swaps["manifest-swap"],
        ),
    ] {
        if files[name].as_ref() != Some(old) && files[name].as_ref() != Some(new) {
            return Err(CHANGED.into());
        }
    }
    let mut staged = BTreeMap::new();
    for name in [
        "config-swap",
        "manifest-swap",
        "db-restore",
        "queue-swap",
        "queue-swap-proof.json",
        "complete.json",
        "restore-complete.json",
    ] {
        staged.insert(name, proof(dir, name, cancel)?);
    }
    let held = dir.child("sources")?;
    let held = names(&held)?.into_iter().collect::<BTreeSet<_>>();
    Ok(recovery_fs::hash(&encode(&(
        proof(dir, "record.json", cancel)?,
        files,
        staged,
        held,
    ))?))
}
impl Offer {
    pub(crate) fn plan(&self) -> crate::recovery_set::Plan {
        self.plan.clone()
    }
    pub(crate) fn config_match(&self) -> crate::library_recovery::ConfigMatch {
        self.config_match
    }
    pub(crate) fn matches_owner(&self, owner: &Lease) -> bool {
        owner.validate().is_ok() && owner.root().id() == self.profile
    }
    pub(crate) fn matches_offer(&self, analysis: u64, plan: &crate::recovery_set::Plan) -> bool {
        self.analysis_epoch == analysis
            && self.id == plan.id
            && self.plan.choice == plan.choice
            && self.plan.previous_tracks == plan.previous_tracks
            && self.plan.backup_bytes == plan.backup_bytes
            && self.plan.preserves_current_references == plan.preserves_current_references
            && self.plan.resumes_interrupted == plan.resumes_interrupted
    }
}
pub(crate) fn analyze(
    owner: &Lease,
    analysis_epoch: u64,
    runtime: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<Offer, String> {
    let mut offer = analyze_with_fence(owner, cancel, &|record| {
        crate::profile_lease::legacy_fence(
            owner.root(),
            &record.scope,
            record.creator,
            &record.start,
            runtime,
        )
    })?;
    offer.analysis_epoch = analysis_epoch;
    Ok(offer)
}
fn analyze_with_fence(
    owner: &Lease,
    cancel: &dyn Fn() -> bool,
    fence: &dyn Fn(&Record) -> Result<(), String>,
) -> Result<Offer, String> {
    owner.validate()?;
    let (dir, record) = read_record(owner.root(), cancel)?;
    fence(&record)?;
    let view = view(owner.root(), &dir, &record, cancel)?;
    let plan = crate::recovery_set::Plan {
        id: record.id.clone(),
        choice: crate::recovery_set::Choice::RestorePrevious,
        previous_tracks: record
            .before
            .iter()
            .filter(|r| r.starts_with("file: "))
            .count(),
        backup_bytes: record.backups.values().map(|p| p.bytes).sum(),
        preserves_current_references: true,
        resumes_interrupted: true,
    };
    let config_match =
        if proof(owner.root(), "config.json", cancel)? == record.controls["config.json"] {
            crate::library_recovery::ConfigMatch::Previous
        } else {
            crate::library_recovery::ConfigMatch::Next
        };
    owner.validate()?;
    Ok(Offer {
        profile: record.profile,
        journal: dir.id(),
        id: record.id,
        view,
        plan,
        analysis_epoch: 0,
        config_match,
    })
}
pub(crate) fn execute(
    owner: &Lease,
    offer: &Offer,
    runtime: &Path,
    cancel: &dyn Fn() -> bool,
    validate: &Validator<'_>,
    stage: &dyn Fn(&'static str) -> Result<(), String>,
) -> Result<crate::recovery_set::ResultDto, String> {
    execute_with_fence(
        owner,
        offer,
        cancel,
        validate,
        &|record| {
            crate::profile_lease::legacy_fence(
                owner.root(),
                &record.scope,
                record.creator,
                &record.start,
                runtime,
            )
        },
        stage,
    )
}
fn execute_with_fence(
    owner: &Lease,
    offer: &Offer,
    cancel: &dyn Fn() -> bool,
    validate: &Validator<'_>,
    fence: &dyn Fn(&Record) -> Result<(), String>,
    stage: &dyn Fn(&'static str) -> Result<(), String>,
) -> Result<crate::recovery_set::ResultDto, String> {
    if !offer.matches_owner(owner) {
        return Err(CHANGED.into());
    }
    let transaction = Transaction::reopen(owner.root(), cancel)?;
    if transaction.dir.id() != offer.journal
        || transaction.record.id != offer.id
        || view(owner.root(), &transaction.dir, &transaction.record, cancel)? != offer.view
    {
        return Err(CHANGED.into());
    }
    fence(&transaction.record)?;
    recovery_fs::check(cancel)?;
    transaction.restore(owner, cancel, validate, &|name| {
        fence(&transaction.record)?;
        stage(name)
    })?;
    Ok(crate::recovery_set::ResultDto {
        completed: true,
        backup_retained: true,
        requires_reopen: true,
    })
}
#[cfg(test)]
pub(crate) fn validate_reopen(owner: &Lease, id: &str) -> Result<(), String> {
    owner.validate()?;
    validate_reopen_dir(owner.root(), id)
}
pub(crate) fn validate_reopen_dir(root: &Dir, id: &str) -> Result<(), String> {
    root.validate()?;
    if root.exists(PENDING)? {
        return Err(BLOCKED.into());
    }
    let dir = root.child(LAST)?;
    let record: Record = decode(&bytes(&dir, "record.json", &|| false)?)?;
    validate_record(&record)?;
    let complete_name = if dir.exists("restore-complete.json")? {
        "restore-complete.json"
    } else {
        "complete.json"
    };
    let complete: Completion = decode(&bytes(&dir, complete_name, &|| false)?)?;
    if complete.plan != id
        || record.id != id
        || root.id() != record.profile
        || current_controls(root, &|| false)? != complete.controls
    {
        return Err(CHANGED.into());
    }
    let library = root.child("library")?;
    let manifest = if complete.restored {
        record.controls["manifest"].as_ref().ok_or(INCOMPLETE)?
    } else {
        &record.swaps["manifest-swap"]
    };
    if proof(&library, MANIFEST_NAME, &|| false)?.as_ref() != Some(manifest) {
        return Err(CHANGED.into());
    }
    for source in record.removed.keys() {
        if library.exists(source)? != complete.restored {
            return Err(CHANGED.into());
        }
    }
    topology(root, &dir, &record, &|| false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc,
        },
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        path: PathBuf,
        owner: Arc<Lease>,
        config: AppConfig,
        next: AppConfig,
        before: HashSet<String>,
        after: HashSet<String>,
        keep: String,
        remove: String,
    }
    impl Fixture {
        fn new() -> Self { Self::with_single_source(false) }
        fn with_single_source(single: bool) -> Self {
            let path = std::env::temp_dir().join(format!(
                "sonante-retirement-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            let profile = path.join("profile");
            let owner = Arc::new(Lease::acquire(&profile).unwrap());
            let keep = path.join("Keep");
            let remove = path.join("Remove");
            for root in [&keep, &remove] {
                fs::DirBuilder::new().mode(0o700).create(root).unwrap();
                fs::write(root.join("song.flac"), b"fixture").unwrap();
            }
            let prepared = crate::local_media::mirror::build_filtered_staging(
                &profile.join("library"),
                &profile.join("missing"),
                &if single {vec![remove.clone()]} else {vec![keep.clone(), remove.clone()]},
                "initial",
            )
            .unwrap();
            let keep_id = prepared
                .manifest
                .sources
                .iter()
                .find(|(_, root)| **root == keep)
                .map(|(id,_)| id.clone()).unwrap_or_default();
            let remove_id = prepared
                .manifest
                .sources
                .iter()
                .find(|(_, root)| **root == remove)
                .unwrap()
                .0
                .clone();
            let mut config = AppConfig::default();
            config.first_run = false;
            config.local_folders = if single {vec![remove.display().to_string()]} else {vec![keep.display().to_string(), remove.display().to_string()]};
            let mut next = config.clone();
            next.local_folders = if single {vec![]} else {vec![keep.display().to_string()]};
            owner
                .root()
                .write_new("config.json", &encode(&config).unwrap())
                .unwrap();
            owner.root().write_new("mpd-v2.db", b"old").unwrap();
            owner.root().write_new("mpd.conf", b"fixture").unwrap();
            owner
                .root()
                .write_new("queue_cache.json", br#"{"version":2,"tracks":[]}"#)
                .unwrap();
            let mut before = HashSet::from([
                format!("file: {keep_id}/song.flac"),
                format!("directory: {keep_id}"),
                format!("file: {remove_id}/song.flac"),
                format!("directory: {remove_id}"),
            ]);
            if single { before.retain(|r| r.split_once(": ").unwrap().1.split('/').next()==Some(remove_id.as_str())); }
            let after = before
                .iter()
                .filter(|r| {
                    !r.split_once(": ")
                        .unwrap()
                        .1
                        .starts_with(&format!("{remove_id}/"))
                        && *r != &format!("directory: {remove_id}")
                })
                .cloned()
                .collect();
            Self {
                path,
                owner,
                config,
                next,
                before,
                after,
                keep: keep_id,
                remove: remove_id,
            }
        }
        fn validate(
            &self,
            data: &[u8],
            cancel: &dyn Fn() -> bool,
        ) -> Result<HashSet<String>, String> {
            recovery_fs::check(cancel)?;
            match data {
                b"old" => Ok(self.before.clone()),
                b"new" => Ok(self.after.clone()),
                _ => Err("fixture_invalid_db".into()),
            }
        }
        fn prepare(&self) -> Transaction {
            prepare(
                &self.owner,
                &self.config,
                &self.next,
                self.before.clone(),
                &|| false,
                &|b, c| self.validate(b, c),
            )
            .unwrap()
        }
        fn set_db(&self, data: &[u8]) {
            let p = self.owner.root().path.join("mpd-v2.db");
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(p)
                .unwrap();
            use std::io::Write;
            file.write_all(data).unwrap();
            file.sync_all().unwrap();
        }
        fn recover(&self) -> Result<(), String> {
            let offer = analyze_with_fence(&self.owner, &|| false, &|_| Ok(()))?;
            execute_with_fence(
                &self.owner,
                &offer,
                &|| false,
                &|b, c| self.validate(b, c),
                &|_| Ok(()),
                &|_| Ok(()),
            )?;
            validate_reopen(&self.owner, &offer.id)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }
    #[test]
    fn source_retirement_last_source_succeeds_after_supervisor_repairs_664_config() {
        let f=Fixture::with_single_source(true);
        assert_eq!(f.config.local_folders.len(),1);assert!(f.next.local_folders.is_empty());assert!(f.after.is_empty());
        let path=f.owner.root().path.join("mpd.conf");
        fs::set_permissions(&path,fs::Permissions::from_mode(0o664)).unwrap();
        let id=f.owner.root().file_identity("mpd.conf").unwrap();
        let old_config=bytes(f.owner.root(),"config.json",&||false).unwrap();
        // Characterize the actual reported guard refusal before any journal or source move.
        assert!(matches!(prepare(&f.owner,&f.config,&f.next,f.before.clone(),&||false,&|b,c|f.validate(b,c)),Err(e) if e==UNSAFE));
        assert!(!f.owner.root().exists(PENDING).unwrap());
        assert_eq!(bytes(f.owner.root(),"config.json",&||false).unwrap(),old_config);
        crate::supervisor::write_mpd_config_file(&f.owner.root().path,b"fixture").unwrap();
        assert_eq!(f.owner.root().file_identity("mpd.conf").unwrap(),id);
        let tx=f.prepare();tx.publish(&f.owner,&||false,&|_|Ok(())).unwrap();f.set_db(b"new");
        tx.commit(&f.owner,&||false,&|b,c|f.validate(b,c),&|_|Ok(())).unwrap();
        validate_reopen(&f.owner,&tx.record.id).unwrap();
        assert!(decode::<AppConfig>(&bytes(f.owner.root(),"config.json",&||false).unwrap()).unwrap().local_folders.is_empty());
        assert!(decode::<MirrorManifest>(&bytes(&f.owner.root().child("library").unwrap(),MANIFEST_NAME,&||false).unwrap()).unwrap().sources.is_empty());
        assert_eq!(fs::read(f.path.join("Remove/song.flac")).unwrap(),b"fixture");
    }

    #[test]
    fn source_retirement_moves_only_selected_root_and_preserves_original_music() {
        let f = Fixture::new();
        let library = f.owner.root().child("library").unwrap();
        let keep = library.child(&f.keep).unwrap().id();
        let removed = library.child(&f.remove).unwrap().id();
        let tx = f.prepare();
        tx.publish(&f.owner, &|| false, &|_| Ok(())).unwrap();
        assert_eq!(library.child(&f.keep).unwrap().id(), keep);
        assert!(!library.exists(&f.remove).unwrap());
        assert_eq!(
            tx.dir
                .child("sources")
                .unwrap()
                .child(&f.remove)
                .unwrap()
                .id(),
            removed
        );
        f.set_db(b"new");
        tx.commit(&f.owner, &|| false, &|b, c| f.validate(b, c), &|_| Ok(()))
            .unwrap();
        assert!(!f.owner.root().exists(PENDING).unwrap());
        assert_eq!(
            fs::read(f.path.join("Remove/song.flac")).unwrap(),
            b"fixture"
        );
        assert_eq!(
            decode::<AppConfig>(&bytes(f.owner.root(), "config.json", &|| false).unwrap()).unwrap(),
            f.next
        );
        validate_reopen(&f.owner, &tx.record.id).unwrap();
    }
    #[test]
    fn source_retirement_recovers_every_publication_and_commit_boundary() {
        for fault in [
            "source_moved",
            "manifest_published",
            "catalog_confirmed",
            "config_committed",
        ] {
            let f = Fixture::new();
            let library = f.owner.root().child("library").unwrap();
            let survivor = library.child(&f.keep).unwrap().id();
            let tx = f.prepare();
            let stage = |name| {
                if name == fault {
                    Err("injected boundary".into())
                } else {
                    Ok(())
                }
            };
            let published = tx.publish(&f.owner, &|| false, &stage);
            if published.is_ok() {
                f.set_db(b"new");
                assert!(tx
                    .commit(&f.owner, &|| false, &|b, c| f.validate(b, c), &stage)
                    .is_err());
            }
            assert!(f.owner.root().exists(PENDING).unwrap());
            f.recover().unwrap();
            assert_eq!(library.child(&f.keep).unwrap().id(), survivor);
            assert!(library.exists(&f.remove).unwrap());
            assert_eq!(
                decode::<AppConfig>(&bytes(f.owner.root(), "config.json", &|| false).unwrap())
                    .unwrap(),
                f.config
            );
            assert_eq!(
                bytes(f.owner.root(), "mpd-v2.db", &|| false).unwrap(),
                b"old"
            );
        }
    }
    #[test]
    fn source_retirement_recovery_can_resume_each_restoration_boundary() {
        for fault in [
            "previous_database_validated",
            "control_restored",
            "source_restored",
            "database_restored",
            "queue_restored",
        ] {
            let f = Fixture::new();
            let tx = f.prepare();
            tx.publish(&f.owner, &|| false, &|_| Ok(())).unwrap();
            f.set_db(b"new");
            let _ = tx.commit(&f.owner, &|| false, &|b, c| f.validate(b, c), &|stage| {
                if stage == "config_committed" {
                    Err("fixture".into())
                } else {
                    Ok(())
                }
            });
            let offer = analyze_with_fence(&f.owner, &|| false, &|_| Ok(())).unwrap();
            assert!(execute_with_fence(
                &f.owner,
                &offer,
                &|| false,
                &|b, c| f.validate(b, c),
                &|_| Ok(()),
                &|stage| if stage == fault {
                    Err("fixture".into())
                } else {
                    Ok(())
                }
            )
            .is_err());
            f.recover().unwrap();
            assert_eq!(
                bytes(f.owner.root(), "mpd-v2.db", &|| false).unwrap(),
                b"old"
            );
        }
    }
    #[test]
    fn source_retirement_rejects_stale_or_foreign_offer_before_writes() {
        let f = Fixture::new();
        let _tx = f.prepare();
        let offer = analyze_with_fence(&f.owner, &|| false, &|_| Ok(())).unwrap();
        let other = Fixture::new();
        assert!(execute_with_fence(
            &other.owner,
            &offer,
            &|| false,
            &|b, c| other.validate(b, c),
            &|_| Ok(()),
            &|_| Ok(())
        )
        .is_err());
        f.set_db(b"new");
        assert!(execute_with_fence(
            &f.owner,
            &offer,
            &|| false,
            &|b, c| f.validate(b, c),
            &|_| Ok(()),
            &|_| Ok(())
        )
        .is_err());
        assert!(f
            .owner
            .root()
            .child("library")
            .unwrap()
            .exists(&f.remove)
            .unwrap());
    }
    #[test]
    fn source_retirement_cancellation_preserves_bound_pending_record() {
        let f = Fixture::new();
        let tx = f.prepare();
        assert!(tx.publish(&f.owner, &|| true, &|_| Ok(())).is_err());
        assert!(f.owner.root().exists(PENDING).unwrap());
        f.recover().unwrap();
    }
    #[test]
    fn source_retirement_queue_cache_is_recovered_with_its_exact_backup() {
        let f = Fixture::new();
        let tx = f.prepare();
        tx.persist_queue_cache(&f.owner, br#"{"version":2,"tracks":[],"fixture":true}"#)
            .unwrap();
        tx.publish(&f.owner, &|| false, &|_| Ok(())).unwrap();
        f.recover().unwrap();
        assert_eq!(
            bytes(f.owner.root(), "queue_cache.json", &|| false).unwrap(),
            br#"{"version":2,"tracks":[]}"#
        );
    }
    #[test]
    fn source_retirement_startup_gate_and_old_journal_coexistence_are_fail_closed() {
        let f = Fixture::new();
        assert!(crate::managed_save_intent::check_startup(&f.owner.root().path).is_ok());
        let _tx = f.prepare();
        assert!(
            crate::managed_save_intent::check_startup(&f.owner.root().path)
                .unwrap_err()
                .starts_with(BLOCKED)
        );
        f.owner
            .root()
            .write_new(crate::managed_save_intent::NAME, b"foreign marker")
            .unwrap();
        assert!(analyze_with_fence(&f.owner, &|| false, &|_| Ok(())).is_err());
        assert!(f
            .owner
            .root()
            .child("library")
            .unwrap()
            .exists(&f.remove)
            .unwrap());
    }
    #[test]
    fn source_retirement_state_admits_only_bound_current_epoch_and_cancel_revokes() {
        let f = Fixture::new();
        let _tx = f.prepare();
        let state = crate::library_recovery::State::new(true);
        let observation = state
            .unit_retirement_analysis(&f.owner.root().path, &f.owner, "own-fixture", |id| {
                let mut offer = analyze_with_fence(&f.owner, &|| false, &|_| Ok(())).unwrap();
                offer.analysis_epoch = id;
                offer
            })
            .unwrap();
        let plan = observation.plan.as_ref().unwrap();
        assert!(state.admission(observation.analysis_id, &plan.id).is_ok());
        assert!(state
            .admission(observation.analysis_id + 1, &plan.id)
            .is_err());
        assert!(state
            .admission(observation.analysis_id, "foreign-plan")
            .is_err());
        state.cancel("own-fixture");
        assert!(state.admission(observation.analysis_id, &plan.id).is_err());
        // A binding from a different epoch is revoked during publication.
        let stale = state
            .unit_retirement_analysis(&f.owner.root().path, &f.owner, "stale-fixture", |id| {
                let mut offer = analyze_with_fence(&f.owner, &|| false, &|_| Ok(())).unwrap();
                offer.analysis_epoch = id + 1;
                offer
            })
            .unwrap();
        assert!(!stale.recovery_available);
        assert!(stale.plan.is_none());
        assert!(f
            .owner
            .root()
            .child("library")
            .unwrap()
            .exists(&f.remove)
            .unwrap());
    }

    #[test]
    fn source_retirement_native_request_is_only_for_the_retired_id() {
        let f = Fixture::new();
        let tx = f.prepare();
        tx.publish(&f.owner, &|| false, &|_| Ok(())).unwrap();
        let mut after = f.after.iter().cloned().collect::<Vec<_>>();
        after.sort();
        let catalog = format!("{}\nOK\n", after.join("\n"));
        let (socket, server) = crate::tests::fake_restorative_rescan_mpd(vec![
            (
                format!("update \"{}\"", f.remove),
                "updating_db: 73\nOK\n".to_string(),
            ),
            ("status".to_string(), "state: stop\nOK\n".to_string()),
            ("listall".to_string(), catalog),
        ]);
        let plan = crate::local_media::mirror::ManagedRefreshPlan::Targeted {
            directories: tx.removed_ids(),
            changed_files: 0,
        };
        crate::request_refresh_and_inspect(&socket, &plan, None, None, || {
            if crate::AudioEngine::read_validated_local_catalog_at(&socket)? != tx.after() {
                return Err("fixture mismatch".into());
            }
            Ok(())
        })
        .unwrap();
        server.join().unwrap();
        f.set_db(b"new");
        tx.commit(&f.owner, &|| false, &|b, c| f.validate(b, c), &|_| Ok(()))
            .unwrap();
        assert!(f
            .owner
            .root()
            .child("library")
            .unwrap()
            .exists(&f.keep)
            .unwrap());
    }
    #[test]
    fn source_retirement_rejects_preexisting_config_manifest_divergence_before_a_journal() {
        let f = Fixture::new();
        let bad = f.next.clone();
        fs::write(
            f.owner.root().path.join("config.json"),
            encode(&bad).unwrap(),
        )
        .unwrap();
        assert!(prepare(
            &f.owner,
            &bad,
            &f.next,
            f.before.clone(),
            &|| false,
            &|b, c| f.validate(b, c)
        )
        .is_err());
        assert!(!f.owner.root().exists(PENDING).unwrap());
        assert!(f
            .owner
            .root()
            .child("library")
            .unwrap()
            .exists(&f.remove)
            .unwrap());
    }
    #[test]
    fn source_retirement_classifies_without_breaking_legacy_addition_or_falling_back_to_rescan() {
        let f = Fixture::new();
        let library = f.owner.root().path.join("library");
        let sources = crate::retirement_consent_for_profile(&library, &f.config, &f.next)
            .unwrap()
            .unwrap();
        assert_eq!(sources.removed_ids, vec![f.remove.clone()]);
        let mut manifest: MirrorManifest = decode(
            &bytes(
                &f.owner.root().child("library").unwrap(),
                MANIFEST_NAME,
                &|| false,
            )
            .unwrap(),
        )
        .unwrap();
        manifest.schema_version = 1;
        manifest.registry_id = None;
        fs::write(library.join(MANIFEST_NAME), encode(&manifest).unwrap()).unwrap();
        assert!(crate::retirement_consent_for_profile(&library, &f.config, &f.next).is_err());
        fs::remove_file(library.join(MANIFEST_NAME)).unwrap();
        assert!(crate::retirement_consent_for_profile(&library, &f.config, &f.next).is_err());
        let mut old = f.config.clone();
        old.local_folders.pop();
        assert!(
            crate::retirement_consent_for_profile(&library, &old, &f.config)
                .unwrap()
                .is_none()
        );
        assert!(!f.owner.root().exists(PENDING).unwrap());
    }
}
