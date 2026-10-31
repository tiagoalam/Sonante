//! Finite durable sets for transactions produced by this app. Historical intents
//! without a complete set remain report-only. No migration/import engine.
use crate::{
    managed_save_intent::{self, RecoveryBinding},
    profile_lease::Lease,
    recovery_fs::{self, Dir, Identity, Tree, TreeEntry, CHANGED, INCOMPLETE, UNSAFE},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    os::fd::AsRawFd,
    path::Path,
};
pub(crate) const PENDING: &str = ".sonante-recovery.pending";
pub(crate) const LAST: &str = ".sonante-recovery.last";
const MANIFEST: &str = "set.json";
pub(crate) const SEAL: &str = ".sonante-recovery.seal.json";
const LIMIT: u64 = 128 * 1024 * 1024;
const PREFLIGHT_UNCONFIRMED: &str = "LIBRARY_RECOVERY_PREFLIGHT_UNCONFIRMED";
pub(crate) const CONTROLS: [&str; 11] = [
    "config.json",
    "mpd-v2.db",
    "queue_cache.json",
    "favorites.json",
    "playlists.json",
    "mpd.conf",
    "local-source-identities.json",
    "local-source-identities.lock",
    "equalizer-shared-v1.json",
    managed_save_intent::NAME,
    SEAL,
];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Artifact {
    pub hash: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub profile: Identity,
    pub files: BTreeMap<String, Option<Artifact>>,
    pub identities: BTreeMap<String, Option<Identity>>,
    pub live: Option<Tree>,
    pub next: Option<Tree>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Set {
    version: u32,
    prior: Snapshot,
    binding: RecoveryBinding,
    catalog: BTreeSet<String>,
    scope: String,
    creator: u32,
    start: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    version: u32,
    profile: Identity,
    manifest_hash: String,
    record_hash: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    value: T,
    checksum: String,
}
fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    let raw = serde_json::to_vec(v).map_err(|_| INCOMPLETE)?;
    serde_json::to_vec(&Envelope {
        value: v,
        checksum: recovery_fs::hash(&raw),
    })
    .map_err(|_| INCOMPLETE.into())
}
fn decode<T: serde::de::DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, String> {
    let e: Envelope<T> = serde_json::from_slice(bytes).map_err(|_| INCOMPLETE)?;
    if recovery_fs::hash(&serde_json::to_vec(&e.value).map_err(|_| INCOMPLETE)?) != e.checksum {
        return Err(INCOMPLETE.into());
    }
    Ok(e.value)
}
fn snapshot(
    root: &Dir,
    cancel: &dyn Fn() -> bool,
) -> Result<(Snapshot, BTreeMap<String, Vec<u8>>), String> {
    let mut files = BTreeMap::new();
    let mut identities = BTreeMap::new();
    let mut raw = BTreeMap::new();
    let mut total = 0;
    for name in CONTROLS {
        recovery_fs::check(cancel)?;
        let bytes = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        identities.insert(name.into(), root.file_identity(name)?);
        files.insert(
            name.into(),
            bytes.as_ref().map(|b| Artifact {
                hash: recovery_fs::hash(b),
                bytes: b.len() as u64,
            }),
        );
        if let Some(bytes) = bytes {
            total += bytes.len();
            if total > 256 * 1024 * 1024 {
                return Err(INCOMPLETE.into());
            }
            raw.insert(name.into(), bytes);
        }
    }
    let live = Tree::capture(root, "library", cancel)?;
    let next = Tree::capture(root, "library.next", cancel)?;
    let result = Snapshot {
        profile: root.id(),
        files,
        identities,
        live,
        next,
    };
    verify_files(root, &result, cancel)?;
    Ok((result, raw))
}
fn verify_files(root: &Dir, expected: &Snapshot, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    if root.id() != expected.profile {
        return Err(CHANGED.into());
    }
    for (name, proof) in &expected.files {
        let bytes = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        if bytes
            .as_ref()
            .map(|b| Artifact {
                hash: recovery_fs::hash(b),
                bytes: b.len() as u64,
            })
            .as_ref()
            != proof.as_ref()
            || root.file_identity(name)?.as_ref()
                != expected.identities.get(name).ok_or(INCOMPLETE)?.as_ref()
        {
            return Err(CHANGED.into());
        }
    }
    root.validate()
}
fn write_snapshot(
    dir: &Dir,
    view: &Snapshot,
    raw: &BTreeMap<String, Vec<u8>>,
    cancel: &dyn Fn() -> bool,
    budget: u64,
    created: &dyn Fn(String, Identity, bool),
) -> Result<(), String> {
    let size = raw.values().map(|b| b.len() as u64).sum::<u64>();
    let mut space = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::fstatvfs(dir.file.as_raw_fd(), space.as_mut_ptr()) } != 0 {
        return Err(INCOMPLETE.into());
    }
    let space = unsafe { space.assume_init() };
    if size.saturating_add(1024 * 1024) > budget.min(space.f_bavail.saturating_mul(space.f_frsize))
    {
        return Err("LIBRARY_RECOVERY_BACKUP_NO_SPACE".into());
    }
    let files = dir.create("files")?;
    created("files".into(), files.id(), true);
    for (name, bytes) in raw {
        recovery_fs::check(cancel)?;
        files.write_new_tracked(name, bytes, &|id| {
            created(format!("files/{name}"), id, false)
        })?;
    }
    recovery_fs::check(cancel)?;
    dir.write_new_tracked("snapshot.json", &encode(view)?, &|id| {
        created("snapshot.json".into(), id, false)
    })?;
    dir.sync()
}
fn read_snapshot(dir: &Dir, cancel: &dyn Fn() -> bool) -> Result<Snapshot, String> {
    let bytes = dir
        .read("snapshot.json", 64 * 1024 * 1024, cancel)?
        .ok_or(INCOMPLETE)?;
    let view: Snapshot = decode(&bytes)?;
    let files = dir.child("files")?;
    for name in owned_names(&files)? {
        if view.files.get(&name).is_none_or(Option::is_none) {
            return Err(UNSAFE.into());
        }
    }
    for (name, proof) in &view.files {
        if !CONTROLS.contains(&name.as_str()) {
            return Err(UNSAFE.into());
        }
        let bytes = files.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        if bytes
            .as_ref()
            .map(|b| Artifact {
                hash: recovery_fs::hash(b),
                bytes: b.len() as u64,
            })
            .as_ref()
            != proof.as_ref()
        {
            return Err(INCOMPLETE.into());
        }
    }
    if view.files.len() != CONTROLS.len() {
        return Err(INCOMPLETE.into());
    }
    Ok(view)
}
fn file_bytes(dir: &Dir, name: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    dir.child("files")?
        .read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?
        .ok_or(INCOMPLETE.into())
}
fn manifest(tree: &Tree) -> Result<crate::local_media::mirror::MirrorManifest, String> {
    let entry = tree
        .entries
        .get(Path::new(crate::local_media::mirror::MANIFEST_NAME))
        .ok_or(INCOMPLETE)?;
    let TreeEntry::Manifest { bytes } = entry else {
        return Err(INCOMPLETE.into());
    };
    let m: crate::local_media::mirror::MirrorManifest =
        serde_json::from_slice(bytes).map_err(|_| INCOMPLETE)?;
    crate::local_media::mirror::validate_manifest(&m)?;
    for (path, entry) in &tree.entries {
        if path == Path::new(crate::local_media::mirror::MANIFEST_NAME) {
            continue;
        }
        let mut components = path.components();
        let Some(std::path::Component::Normal(id)) = components.next() else {
            return Err(UNSAFE.into());
        };
        let root = m.sources.get(id.to_str().ok_or(UNSAFE)?).ok_or(UNSAFE)?;
        let relative = components.as_path();
        if let TreeEntry::Link { target } = entry {
            if target != &root.join(relative) {
                return Err(UNSAFE.into());
            }
        }
    }
    Ok(m)
}
fn catalog_scope(catalog: &HashSet<String>, tree: &Tree) -> Result<(), String> {
    let m = manifest(tree)?;
    crate::index_coverage::Catalog::from_records(catalog.clone())?;
    if catalog.iter().any(|r| {
        r.split_once(": ")
            .and_then(|(_, u)| u.split('/').next())
            .is_none_or(|id| !m.sources.contains_key(id))
    }) {
        return Err(INCOMPLETE.into());
    }
    Ok(())
}
/// The validator is the real DB reopener in production. Tests inject protocol
/// fixtures explicitly; they do not certify a daemon that was never observed.
pub(crate) fn capture_before(
    lease: &Lease,
    expected: Option<&HashSet<String>>,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
) -> Result<(), String> {
    lease.validate()?;
    let root = lease.root();
    if root.exists(PENDING)? || root.exists(SEAL)? {
        return Err(INCOMPLETE.into());
    }
    let (view, raw) = snapshot(root, cancel)?;
    let intent = raw.get(managed_save_intent::NAME).ok_or(INCOMPLETE)?;
    let binding = managed_save_intent::recovery_binding(intent)?;
    let live = view.live.as_ref().ok_or(INCOMPLETE)?;
    let next = view.next.as_ref().ok_or(INCOMPLETE)?;
    if live.identity != binding.previous_identity
        || next.identity != binding.next_identity
        || managed_save_intent::recovery_config_hash(raw.get("config.json").ok_or(INCOMPLETE)?)?
            != binding.previous_config
    {
        return Err(CHANGED.into());
    }
    let db = raw.get("mpd-v2.db").ok_or(INCOMPLETE)?;
    let catalog = validate(db, cancel)?;
    if expected.is_some_and(|e| e != &catalog) {
        return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into());
    }
    catalog_scope(&catalog, live)?;
    // Prove the live controls did not change while validating their immutable DB
    // copy. Queue changes abort capture, without freezing normal transport.
    verify_files(root, &view, cancel)?;
    if Tree::capture(root, "library", cancel)?.as_ref() != Some(live)
        || Tree::capture(root, "library.next", cancel)?.as_ref() != Some(next)
    {
        return Err(CHANGED.into());
    }
    let scope = crate::profile_lease::process_scope()?;
    let start = crate::profile_lease::creator_start()?;
    let set = Set {
        version: 1,
        prior: view.clone(),
        binding,
        catalog: catalog.into_iter().collect(),
        scope,
        creator: std::process::id(),
        start,
    };
    let dir = root.create(PENDING)?;
    let created = std::cell::RefCell::new(Vec::<(String, Identity, bool)>::new());
    let track = |name, id, is_dir| created.borrow_mut().push((name, id, is_dir));
    let seal_created = std::cell::RefCell::new(None::<Identity>);
    let result = (|| {
        write_snapshot(&dir, &view, &raw, cancel, u64::MAX, &track)?;
        verify_files(root, &view, cancel)?;
        lease.validate()?;
        let encoded = encode(&set)?;
        dir.write_new_tracked(MANIFEST, &encoded, &|id| track(MANIFEST.into(), id, false))?;
        let seal = Seal {
            version: 1,
            profile: root.id(),
            manifest_hash: recovery_fs::hash(&encoded),
            record_hash: set.binding.record_hash.clone(),
        };
        root.write_new_tracked(SEAL, &encode(&seal)?, &|id| {
            *seal_created.borrow_mut() = Some(id)
        })?;
        root.sync()
    })();
    if let Err(error) = result {
        if root.exists(SEAL)? {
            if root.file_identity(SEAL)?.as_ref() != seal_created.borrow().as_ref() {
                return Err(format!("{error}; {CHANGED}"));
            }
            fs::remove_file(root.entry(SEAL)?).map_err(|_| CHANGED)?;
            root.sync()?;
        }
        // This live operation has not published a mirror, cleared a queue, requested
        // indexing, or committed config. Remove only files whose creation identities
        // were captured by this call; an unexpected/substituted entry is preserved.
        match remove_own_partial(root, &dir, &created.into_inner()) {
            Ok(()) => return Err(error),
            Err(cleanup) => return Err(format!("{error}; {cleanup}")),
        }
    }
    Ok(())
}
fn remove_own_partial(
    root: &Dir,
    dir: &Dir,
    created: &[(String, Identity, bool)],
) -> Result<(), String> {
    dir.validate()?;
    root.validate()?;
    let roots = created
        .iter()
        .filter_map(|(p, _, _)| p.split('/').next())
        .collect::<std::collections::BTreeSet<_>>();
    if owned_names(dir)?
        .iter()
        .any(|n| !roots.contains(n.as_str()))
    {
        return Err(CHANGED.into());
    }
    if created.iter().any(|(p, _, d)| p == "files" && *d) {
        let files = dir.child("files")?;
        let expected = created
            .iter()
            .filter_map(|(p, _, _)| p.strip_prefix("files/"))
            .collect::<std::collections::BTreeSet<_>>();
        if owned_names(&files)?
            .iter()
            .any(|n| !expected.contains(n.as_str()))
        {
            return Err(CHANGED.into());
        }
    }
    for (path, id, is_dir) in created {
        if *is_dir {
            if dir.child(path)?.id() != *id {
                return Err(CHANGED.into());
            }
        } else {
            let (parent, name) = if let Some(name) = path.strip_prefix("files/") {
                (dir.child("files")?, name)
            } else {
                (Dir::open(&dir.path)?, path.as_str())
            };
            if parent.file_identity(name)?.as_ref() != Some(id) {
                return Err(CHANGED.into());
            }
        }
    }
    for (path, _, is_dir) in created.iter().rev() {
        if *is_dir {
            fs::remove_dir(dir.entry(path)?).map_err(|_| CHANGED)?;
        } else {
            let (parent, name) = if let Some(name) = path.strip_prefix("files/") {
                (dir.child("files")?, name)
            } else {
                (Dir::open(&dir.path)?, path.as_str())
            };
            fs::remove_file(parent.entry(name)?).map_err(|_| CHANGED)?;
            parent.sync()?;
        }
    }
    dir.sync()?;
    dir.validate()?;
    fs::remove_dir(root.entry(PENDING)?).map_err(|_| CHANGED)?;
    root.sync()
}

fn open_set(root: &Dir, cancel: &dyn Fn() -> bool) -> Result<(Dir, Set), String> {
    let dir = root.child(PENDING)?;
    let manifest_bytes = dir
        .read(MANIFEST, 64 * 1024 * 1024, cancel)?
        .ok_or(INCOMPLETE)?;
    let set: Set = decode(&manifest_bytes)?;
    let seal_bytes = if let Some(bytes) = root.read(SEAL, 4096, cancel)? {
        bytes
    } else {
        if !dir.exists("committed.json")? && !dir.exists("completion.json")? {
            return Err(INCOMPLETE.into());
        }
        dir.read("retained-seal", 4096, cancel)?.ok_or(INCOMPLETE)?
    };
    let seal: Seal = decode(&seal_bytes)?;
    if seal.version != 1
        || seal.profile != root.id()
        || seal.manifest_hash != recovery_fs::hash(&manifest_bytes)
        || seal.record_hash != set.binding.record_hash
    {
        return Err(INCOMPLETE.into());
    }
    if set.version != 1
        || set.prior.profile != root.id()
        || read_snapshot(&dir, cancel)? != set.prior
    {
        return Err(INCOMPLETE.into());
    }
    let b = managed_save_intent::recovery_binding(&file_bytes(
        &dir,
        managed_save_intent::NAME,
        cancel,
    )?)?;
    if b != set.binding {
        return Err(INCOMPLETE.into());
    }
    Ok((dir, set))
}
// One in-memory proof for the exact immutable database bytes read before
// config persistence. It is never serialized or accepted for another profile.
pub(crate) struct PreparedDatabase {
    profile: Identity,
    hash: String,
    records: HashSet<String>,
}
impl PreparedDatabase {
    pub(crate) fn validate_bytes(&self, profile: &Identity, bytes: &[u8], cancel: &dyn Fn() -> bool) -> Result<HashSet<String>, String> {
        recovery_fs::check(cancel)?;
        if profile != &self.profile || recovery_fs::hash(bytes) != self.hash { return Err(CHANGED.into()); }
        Ok(self.records.clone())
    }
    pub(crate) fn validate_current(&self, lease: &Lease, cancel: &dyn Fn() -> bool) -> Result<(), String> {
        lease.validate()?;
        let bytes = lease.root().read("mpd-v2.db", LIMIT, cancel)?.ok_or(INCOMPLETE)?;
        self.validate_bytes(&lease.root().id(), &bytes, cancel)?;
        lease.validate()
    }
}
fn prepare_database_bytes(
    profile: Identity, bytes: &[u8], expected: Option<&HashSet<String>>, cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
) -> Result<PreparedDatabase, String> {
    recovery_fs::check(cancel)?;
    let records = validate(bytes, cancel)?;
    recovery_fs::check(cancel)?;
    if expected.is_some_and(|expected| expected != &records) { return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into()); }
    Ok(PreparedDatabase { profile, hash: recovery_fs::hash(bytes), records })
}
pub(crate) fn prepare_database(
    lease: &Lease, expected: Option<&HashSet<String>>, cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
) -> Result<PreparedDatabase, String> {
    lease.validate()?;
    let bytes = lease.root().read("mpd-v2.db", LIMIT, cancel)?.ok_or(INCOMPLETE)?;
    let proof = prepare_database_bytes(lease.root().id(), &bytes, expected, cancel, validate)?;
    proof.validate_current(lease, cancel)?;
    Ok(proof)
}

/// Called only after config persistence AND the existing B5 audit succeeded,
/// before the caller may clean the previous generation or stop marker.
pub(crate) fn confirm_next(
    lease: &Lease,
    expected: Option<&HashSet<String>>,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
) -> Result<(), String> {
    lease.validate()?;
    let root = lease.root();
    let (dir, set) = open_set(root, cancel)?;
    let (view, raw) = snapshot(root, cancel)?;
    let live = view.live.as_ref().ok_or(INCOMPLETE)?;
    if live.identity != set.binding.next_identity
        || managed_save_intent::recovery_config_hash(raw.get("config.json").ok_or(INCOMPLETE)?)?
            != set.binding.next_config
    {
        return Err(CHANGED.into());
    }
    let catalog = validate(raw.get("mpd-v2.db").ok_or(INCOMPLETE)?, cancel)?;
    if expected.is_some_and(|e| e != &catalog) {
        return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into());
    }
    catalog_scope(&catalog, live)?;
    verify_files(root, &view, cancel)?;
    let committed = dir.create("confirmed")?;
    write_snapshot(&committed, &view, &raw, cancel, u64::MAX, &|_, _, _| {})?;
    committed.write_new(
        "catalog.json",
        &encode(&catalog.into_iter().collect::<BTreeSet<_>>())?,
    )?;
    lease.validate()?;
    committed.write_new("complete.json", b"1")?;
    dir.sync()
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Choice {
    RestorePrevious,
    FinishConfirmed,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Plan {
    pub id: String,
    pub choice: Choice,
    pub previous_tracks: usize,
    pub backup_bytes: u64,
    pub preserves_current_references: bool,
    pub resumes_interrupted: bool,
}
fn compatible_ledger(
    root: &Dir,
    dir: &Dir,
    set: &Set,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    let old = file_bytes(dir, "local-source-identities.json", cancel)?;
    let anchor = file_bytes(dir, "local-source-identities.lock", cancel)?;
    let current = root
        .read("local-source-identities.json", 16 * 1024 * 1024, cancel)?
        .ok_or(INCOMPLETE)?;
    let current_anchor = root
        .read("local-source-identities.lock", 32, cancel)?
        .ok_or(INCOMPLETE)?;
    crate::source_identity::validate_recovery_extension(&old, &anchor, &current, &current_anchor)?;
    let confirmed;
    let previous = if dir.exists("completion.json")? {
        confirmed = decode::<Snapshot>(
            &dir.read("completion.json", 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?;
        confirmed.live.as_ref().ok_or(INCOMPLETE)?
    } else if dir.exists("confirmed")? {
        confirmed = read_snapshot(&dir.child("confirmed")?, cancel)?;
        confirmed.live.as_ref().ok_or(INCOMPLETE)?
    } else {
        set.prior.live.as_ref().ok_or(INCOMPLETE)?
    };
    let m = manifest(previous)?;
    if m.registry_id.is_none() {
        return Err("LIBRARY_RECOVERY_LEGACY_GENERATION_UNPROVEN".into());
    }
    crate::source_identity::validate_recovery_snapshot(&current, &current_anchor, &[m])
        .map_err(|_| INCOMPLETE.into())
}
fn content_files_match(actual: &Snapshot, expected: &Snapshot) -> bool {
    ["config.json", "mpd-v2.db", "mpd.conf"].iter().all(|n| {
        actual.files.get(*n) == expected.files.get(*n)
            && actual.identities.get(*n) == expected.identities.get(*n)
    }) && actual.live == expected.live
}
pub(crate) fn analyze(root: &Dir, cancel: &dyn Fn() -> bool) -> Result<Plan, String> {
    let (dir, set) = open_set(root, cancel)?;
    compatible_ledger(root, &dir, &set, cancel)?;
    let (current, raw) = snapshot(root, cancel)?;
    let actual = raw
        .get(managed_save_intent::NAME)
        .map(|b| recovery_fs::hash(b));
    if actual.as_deref() != Some(&set.binding.record_hash)
        && !(actual.is_none()
            && (dir.exists("confirmed")?
                || dir.exists("completion.json")?
                || dir.exists("committed.json")?))
    {
        return Err(CHANGED.into());
    }
    if dir.exists("execution.json")? {
        let journal: Journal = decode(
            &dir.read("execution.json", 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?;
        if journal.version != 1 {
            return Err(INCOMPLETE.into());
        }
        verify_intermediate(root, &dir, &journal, cancel)?;
        return Ok(Plan {
            id: journal.id,
            choice: journal.choice,
            previous_tracks: set
                .catalog
                .iter()
                .filter(|r| r.starts_with("file: "))
                .count(),
            backup_bytes: 0,
            preserves_current_references: true,
            resumes_interrupted: true,
        });
    }
    let choice = if dir.exists("completion.json")? {
        let confirmed: Snapshot = decode(
            &dir.read("completion.json", 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?;
        if !content_files_match(&current, &confirmed) {
            return Err(CHANGED.into());
        }
        Choice::FinishConfirmed
    } else if dir.exists("confirmed")? {
        let confirmed = dir.child("confirmed")?;
        if confirmed.read("complete.json", 8, cancel)?.as_deref() != Some(b"1") {
            return Err(INCOMPLETE.into());
        }
        let proof = read_snapshot(&confirmed, cancel)?;
        if !content_files_match(&current, &proof) {
            return Err(CHANGED.into());
        }
        Choice::FinishConfirmed
    } else {
        let previous = set.prior.live.as_ref().ok_or(INCOMPLETE)?;
        if current.live.as_ref() != Some(previous) && current.next.as_ref() != Some(previous) {
            return Err("LIBRARY_RECOVERY_PREVIOUS_GENERATION_INCOMPLETE".into());
        }
        Choice::RestorePrevious
    };
    let id = recovery_fs::hash(
        &serde_json::to_vec(&(choice, &current, recovery_fs::hash(&encode(&set)?)))
            .map_err(|_| INCOMPLETE)?,
    );
    Ok(Plan {
        id,
        choice,
        previous_tracks: set
            .catalog
            .iter()
            .filter(|r| r.starts_with("file: "))
            .count(),
        backup_bytes: current.files.values().flatten().map(|p| p.bytes).sum(),
        preserves_current_references: true,
        resumes_interrupted: false,
    })
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    id: String,
    choice: Choice,
    before: Snapshot,
    final_files: BTreeMap<String, Option<Artifact>>,
    final_live: Tree,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ResultDto {
    pub completed: bool,
    pub backup_retained: bool,
    pub requires_reopen: bool,
}
fn proof(bytes: Option<&[u8]>) -> Option<Artifact> {
    bytes.map(|b| Artifact {
        hash: recovery_fs::hash(b),
        bytes: b.len() as u64,
    })
}
fn stage(dir: &Dir, name: &str) -> Result<(), String> {
    if dir.exists(name)? {
        if dir.read(name, 8, &|| false)?.as_deref() != Some(b"1") {
            return Err(INCOMPLETE.into());
        }
    } else {
        dir.write_new(name, b"1")?
    }
    dir.sync()
}
fn origins(root: &Dir, tree: &Tree, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let bytes = root
        .read("local-source-identities.json", 16 * 1024 * 1024, cancel)?
        .ok_or(INCOMPLETE)?;
    let m = manifest(tree)?;
    crate::source_identity::verify_recovery_origins(&bytes, &m.sources)
}
fn expected_next(journal: &Journal) -> Option<&Tree> {
    if journal.before.live.as_ref() == Some(&journal.final_live) {
        journal.before.next.as_ref()
    } else {
        journal.before.live.as_ref()
    }
}
fn verify_intermediate(
    root: &Dir,
    dir: &Dir,
    j: &Journal,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    for (name, before) in &j.before.files {
        let bytes = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        let actual = proof(bytes.as_deref());
        let after = j.final_files.get(name).ok_or(INCOMPLETE)?;
        if &actual != before && &actual != after {
            return Err(CHANGED.into());
        }
        let identity = root.file_identity(name)?;
        let effect = format!("effect-{name}.json");
        let published = if dir.exists(&effect)? {
            Some(decode::<Identity>(
                &dir.read(&effect, 4096, cancel)?.ok_or(INCOMPLETE)?,
            )?)
        } else {
            None
        };
        let before_id = j.before.identities.get(name).ok_or(INCOMPLETE)?;
        if matches!(name.as_str(), managed_save_intent::NAME | SEAL)
            && actual.is_none()
            && before.is_some()
            && !dir.exists("committed.json")?
        {
            return Err(CHANGED.into());
        }
        if matches!(
            name.as_str(),
            "config.json" | "mpd-v2.db" | "queue_cache.json"
        ) {
            let temp = format!("{name}.writing");
            if root.exists(&temp)? {
                let temporary = root.file_identity(&temp)?;
                let bytes = root.read(&temp, LIMIT, cancel)?;
                let content = proof(bytes.as_deref());
                let prepared = temporary.as_ref() == published.as_ref()
                    && published.is_some()
                    && &content == after;
                let retired = temporary.as_ref() == before_id.as_ref()
                    && temporary.is_some()
                    && &content == before
                    && identity.as_ref() == published.as_ref()
                    && published.is_some();
                if !prepared && !retired {
                    return Err(CHANGED.into());
                }
            }
        }
        if identity.is_some()
            && identity.as_ref() != before_id.as_ref()
            && identity.as_ref() != published.as_ref()
        {
            return Err(CHANGED.into());
        }
    }
    let live = Tree::capture(root, "library", cancel)?;
    if live.as_ref() != j.before.live.as_ref() && live.as_ref() != Some(&j.final_live) {
        return Err(CHANGED.into());
    }
    let next = Tree::capture(root, "library.next", cancel)?;
    if next.as_ref() != j.before.next.as_ref() && next.as_ref() != expected_next(j) {
        if next.is_some()
            || Tree::capture(dir, "retained-next", cancel)?.as_ref() != expected_next(j)
        {
            return Err(CHANGED.into());
        }
    }
    Ok(())
}
// PROPOSED I1d private executor-only diagnostics. No new public transport.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cause { LegacyUnknown, CancellationObserved, PreflightNotMatched,
    PlanContextChanged, OfferedContextUnconfirmed, LocalJournalContextUnconfirmed,
    CatalogMismatchObserved }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Checkpoint {
    EntrySetOpen, OwnerValidation, SetOpen, OwnershipFence, CancelCheckpoint,
    PlanRead, LedgerRead, ReadonlyCore, CoreDecision, CorePlanBinding,
    OfferedCurrentBinding, FreshPlanRead, FreshPlanDecision, JournalPresence,
    InheritedJournalRead, InheritedJournalHeader, InheritedBlockedRead,
    FreshSnapshot, ExistingBlockedRead, BlockedCreate, BlockedSnapshotWrite,
    BlockedVerify, BackupHook, FinalTargetPrepare, JournalWrite, BackupMarker,
    LocalJournalBinding, IntermediateFence, FenceBoundary, OriginsRead,
    CommittedStateRead, CatalogInputRead, CatalogExpectedRead, CatalogBeforeApply,
    ApplyingMarker, ApplyingHook, GenerationRead, GenerationExchange, RootSync,
    ExchangeHook, NextRead, NextRetention, NextRetentionHook, ControlRead,
    ControlPrepare, ControlProofWrite, ControlReplace, ControlHook, ControlRetention,
    ControlProofDecision, ControlBackupRead, ControlTemporaryRead, ControlEffectRead,
    ValidatingMarker, ValidatingHook, PostDatabaseRead, CatalogAfterApply,
    FinalControlsRead, CommittedMarker, CommittedHook, IntentRead,
    IntentRetention, IntentRetentionHook, SealRead, SealRetention,
    FinalizedMarker, FinalizedHook, RetireOpaqueSubprotocol,
    RetireReadyControls, RetireReadyGeneration, Unknown,
}
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum Effect { Protection=1, LiveGenerationNamespace=2, ControlTemporary=4,
    CanonicalControl=8, Retention=16, BackupNamespace=32,
    ValidatorScratchChildUnknown=64, OpaqueCallback=128 }
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum InheritedFact { JournalAndBlockedRead=1, CommittedNameObservedNotCompleted=2 }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Knowledge { Available, Unknown }
#[derive(Clone, Copy, PartialEq, Eq)]
struct ExecutionProgress {
    checkpoint:Checkpoint, cause:Cause, may_have_issued:u16,
    inherited_facts:u8, knowledge:Knowledge,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct ExecutionFault { progress:ExecutionProgress }
// Deliberate isolation: raw legacy text is NOT part of ExecutionFault.
// No Debug/Serialize/Deserialize/Display/Error/source chaining on either carrier.
struct LegacyFailure(String);
struct ExecutionFailure { fault:ExecutionFault, legacy:LegacyFailure }
impl ExecutionFailure {
    #[cfg(test)]
    fn into_legacy(self)->String { self.legacy.0 }
    fn safe_fault(&self)->ExecutionFault { self.fault }
}
struct ProgressCell(std::cell::Cell<ExecutionProgress>);
impl ProgressCell {
    fn new()->Self {Self(std::cell::Cell::new(ExecutionProgress{
        checkpoint:Checkpoint::Unknown,cause:Cause::LegacyUnknown,
        may_have_issued:0,inherited_facts:0,knowledge:Knowledge::Available}))}
    fn at(&self,checkpoint:Checkpoint) {
        let mut p=self.0.get();p.checkpoint=checkpoint;p.cause=Cause::LegacyUnknown;self.0.set(p);
    }
    fn known(&self,cause:Cause) {let mut p=self.0.get();p.cause=cause;self.0.set(p);}
    fn may(&self,effect:Effect) {let mut p=self.0.get();p.may_have_issued|=effect as u16;self.0.set(p);}
    fn inherited(&self,fact:InheritedFact) {let mut p=self.0.get();p.inherited_facts|=fact as u8;self.0.set(p);}
    fn retire_failure(&self,legacy:String)->String {
        self.at(Checkpoint::RetireOpaqueSubprotocol);legacy
    }
    fn failure(&self,legacy:String)->ExecutionFailure {
        ExecutionFailure{fault:ExecutionFault{progress:self.0.get()},legacy:LegacyFailure(legacy)}
    }
}
impl ExecutionFault {
    // Only for a caller with an explicit missing/unavailable diagnostic outcome.
    // Does NOT catch panic, turn abort into return, or assert completion/quiescence.
    #[cfg(test)]
    fn unavailable()->Self {Self{progress:ExecutionProgress{checkpoint:Checkpoint::Unknown,
        cause:Cause::LegacyUnknown,may_have_issued:u16::MAX,inherited_facts:0,
        knowledge:Knowledge::Unknown}}}
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum OwnershipContract { FixedLegacyFenceReadOnly, Opaque }
// PROPOSED finite projection facade. No serde/Debug/Display/Error or payload getters.
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum FailureClass { LegacyUnknown, CancellationObserved, PreflightUnconfirmed,
    PlanContextChanged, OfferedContextUnconfirmed, LocalJournalContextUnconfirmed, CatalogMismatchObserved }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum FailureBoundary { ProtocolCheck, ReadonlyPreflight, ProtectionRecords,
    LocalJournalBinding, CatalogCheck, GenerationNamespace, ControlWrites,
    FinalValidation, CommitRetention, Unknown }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum FailureKnowledge { ExecutorCaptured, UnknownBoundary }
#[derive(Clone,Copy,PartialEq,Eq)]
#[repr(u8)]
pub(crate) enum PossibleEffect { Protection=1, LiveGenerationNamespace=2,
    ControlTemporary=4, CanonicalControl=8, Retention=16, BackupNamespace=32,
    ValidatorOrChildUnknown=64, OpaqueCallback=128 }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) enum ObservedInherited { JournalAndBlockedRead, CommittedNameNotCompleted }
#[derive(Clone,Copy,PartialEq,Eq)]
pub(crate) struct FailureSummary {
    class:FailureClass, boundary:FailureBoundary, knowledge:FailureKnowledge,
    possible:u8, observed:u8,
}
impl FailureSummary {
    // Sole production known-fact constructor: called only on actual detailed Err.
    fn from_fault(f:ExecutionFault)->Self {
        use Checkpoint as C;
        let class=match f.progress.cause {
            Cause::LegacyUnknown=>FailureClass::LegacyUnknown,
            Cause::CancellationObserved=>FailureClass::CancellationObserved,
            Cause::PreflightNotMatched=>FailureClass::PreflightUnconfirmed,
            Cause::PlanContextChanged=>FailureClass::PlanContextChanged,
            Cause::OfferedContextUnconfirmed=>FailureClass::OfferedContextUnconfirmed,
            Cause::LocalJournalContextUnconfirmed=>FailureClass::LocalJournalContextUnconfirmed,
            Cause::CatalogMismatchObserved=>FailureClass::CatalogMismatchObserved,
        };
        let boundary=match f.progress.checkpoint {
            C::EntrySetOpen|C::OwnerValidation|C::SetOpen|C::OwnershipFence|C::CancelCheckpoint|
            C::PlanRead|C::LedgerRead|C::IntermediateFence|C::FenceBoundary|C::OriginsRead|
            C::CommittedStateRead|C::RootSync=>FailureBoundary::ProtocolCheck,
            C::ReadonlyCore|C::CoreDecision|C::CorePlanBinding|C::OfferedCurrentBinding|
            C::FreshPlanRead|C::FreshPlanDecision=>FailureBoundary::ReadonlyPreflight,
            C::JournalPresence|C::InheritedJournalRead|C::InheritedJournalHeader|C::InheritedBlockedRead|
            C::FreshSnapshot|C::ExistingBlockedRead|C::BlockedCreate|C::BlockedSnapshotWrite|
            C::BlockedVerify|C::BackupHook|C::FinalTargetPrepare|C::JournalWrite|C::BackupMarker=>FailureBoundary::ProtectionRecords,
            C::LocalJournalBinding=>FailureBoundary::LocalJournalBinding,
            C::CatalogInputRead|C::CatalogExpectedRead|C::CatalogBeforeApply=>FailureBoundary::CatalogCheck,
            C::ApplyingMarker|C::ApplyingHook|C::GenerationRead|C::GenerationExchange|
            C::ExchangeHook|C::NextRead|C::NextRetention|C::NextRetentionHook=>FailureBoundary::GenerationNamespace,
            C::ControlRead|C::ControlPrepare|C::ControlProofWrite|C::ControlReplace|C::ControlHook|
            C::ControlRetention|C::ControlProofDecision|C::ControlBackupRead|C::ControlTemporaryRead|
            C::ControlEffectRead=>FailureBoundary::ControlWrites,
            C::ValidatingMarker|C::ValidatingHook|C::PostDatabaseRead|C::CatalogAfterApply|
            C::FinalControlsRead=>FailureBoundary::FinalValidation,
            C::CommittedMarker|C::CommittedHook|C::IntentRead|C::IntentRetention|C::IntentRetentionHook|
            C::SealRead|C::SealRetention|C::FinalizedMarker|C::FinalizedHook|C::RetireOpaqueSubprotocol|
            C::RetireReadyControls|C::RetireReadyGeneration=>FailureBoundary::CommitRetention,
            C::Unknown=>FailureBoundary::Unknown,
        };
        // Explicit allowlist, not exporting ExecutionProgress or arbitrary raw bits.
        let mut possible=0u8;
        for (old,new) in [(Effect::Protection,PossibleEffect::Protection),
            (Effect::LiveGenerationNamespace,PossibleEffect::LiveGenerationNamespace),
            (Effect::ControlTemporary,PossibleEffect::ControlTemporary),
            (Effect::CanonicalControl,PossibleEffect::CanonicalControl),
            (Effect::Retention,PossibleEffect::Retention),(Effect::BackupNamespace,PossibleEffect::BackupNamespace),
            (Effect::ValidatorScratchChildUnknown,PossibleEffect::ValidatorOrChildUnknown),
            (Effect::OpaqueCallback,PossibleEffect::OpaqueCallback)] {
            if f.progress.may_have_issued & old as u16 != 0 {possible|=new as u8;}
        }
        let mut observed=0;
        if f.progress.inherited_facts & InheritedFact::JournalAndBlockedRead as u8 != 0 {observed|=1;}
        if f.progress.inherited_facts & InheritedFact::CommittedNameObservedNotCompleted as u8 != 0 {observed|=2;}
        if f.progress.knowledge==Knowledge::Unknown {return Self::unknown();}
        Self{class,boundary,knowledge:FailureKnowledge::ExecutorCaptured,possible,observed}
    }
    pub(crate) fn unknown()->Self {Self{class:FailureClass::LegacyUnknown,boundary:FailureBoundary::Unknown,
        knowledge:FailureKnowledge::UnknownBoundary,possible:u8::MAX,observed:0}}
    pub(crate) fn class(&self)->FailureClass {self.class}
    pub(crate) fn boundary(&self)->FailureBoundary {self.boundary}
    pub(crate) fn knowledge(&self)->FailureKnowledge {self.knowledge}
    pub(crate) fn may_include(&self,e:PossibleEffect)->bool {self.possible & e as u8 != 0}
    // False means NOT OBSERVED, never absence of inherited records/effects.
    pub(crate) fn observed(&self,e:ObservedInherited)->bool {
        self.observed & match e {ObservedInherited::JournalAndBlockedRead=>1,ObservedInherited::CommittedNameNotCompleted=>2} != 0
    }
}
pub(crate) struct ProjectedExecutionFailure { summary:FailureSummary, legacy:LegacyFailure }
impl ProjectedExecutionFailure {
    pub(crate) fn into_parts(self)->(FailureSummary,String) {(self.summary,self.legacy.0)}
}
pub(crate) fn execute_projected(
    lease:&Lease,id:&str,expected:&OfferedRecoveryBinding,runtime:&Path,
    cancel:&dyn Fn()->bool,
    validate:&dyn Fn(&[u8],&dyn Fn()->bool)->Result<HashSet<String>,String>,
    hook:&dyn Fn(&str)->Result<(),String>,
)->Result<ResultDto,ProjectedExecutionFailure> {
    execute_detailed(lease,id,expected,runtime,cancel,validate,hook).map_err(|failure| {
        let summary=FailureSummary::from_fault(failure.safe_fault());
        ProjectedExecutionFailure{summary,legacy:failure.legacy}
    })
}
fn execute_detailed(
    lease: &Lease,
    id: &str,
    expected: &OfferedRecoveryBinding,
    runtime: &Path,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
) -> Result<ResultDto, ExecutionFailure> {
    let progress=ProgressCell::new();
    progress.at(Checkpoint::EntrySetOpen);
    // A generic cancellation predicate is also opaque. This conservative bit
    // forbids global no-effect claims, even before executor profile writes.
    progress.may(Effect::OpaqueCallback);
    let (_, set)=open_set(lease.root(),cancel).map_err(|legacy|progress.failure(legacy))?;
    execute_checked_detailed(lease,id,expected,cancel,validate,hook,&|| {
        crate::profile_lease::legacy_fence(lease.root(),&set.scope,set.creator,&set.start,runtime)
    },&progress,OwnershipContract::FixedLegacyFenceReadOnly)
}
// Existing cfgtest component helper callers retain the mandatory expected input.
#[cfg(test)]
fn execute_checked(
    lease: &Lease,
    id: &str,
    expected: &OfferedRecoveryBinding,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
    ownership: &dyn Fn() -> Result<(), String>,
) -> Result<ResultDto, String> {
    let progress=ProgressCell::new();
    progress.may(Effect::OpaqueCallback); // generic cancel predicate before any use
    execute_checked_detailed(lease,id,expected,cancel,validate,hook,ownership,
        &progress,OwnershipContract::Opaque).map_err(ExecutionFailure::into_legacy)
}
fn execute_checked_detailed(
    lease: &Lease,
    id: &str,
    expected: &OfferedRecoveryBinding,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
    ownership: &dyn Fn() -> Result<(), String>,
    progress:&ProgressCell,
    ownership_contract:OwnershipContract,
) -> Result<ResultDto, ExecutionFailure> {
    let result=(|| -> Result<ResultDto,String> {
    progress.at(Checkpoint::OwnerValidation);
    lease.validate()?;
    let root = lease.root();
    progress.at(Checkpoint::SetOpen);
    let (dir, set) = open_set(root, cancel)?;
    progress.at(Checkpoint::OwnershipFence);
    if ownership_contract == OwnershipContract::Opaque { progress.may(Effect::OpaqueCallback); }
    ownership()?;
    progress.at(Checkpoint::CancelCheckpoint);
    if let Err(legacy) = recovery_fs::check(cancel) {
        progress.known(Cause::CancellationObserved);
        return Err(legacy);
    }
    progress.at(Checkpoint::PlanRead);
    let plan = analyze(root, cancel)?;
    if plan.id != id {
        progress.known(Cause::PlanContextChanged);
        return Err(CHANGED.into());
    }
    progress.at(Checkpoint::LedgerRead);
    compatible_ledger(root, &dir, &set, cancel)?;
    // FF04-I1c readonly preflight; scope releases preliminary trees before journal.
    {
        progress.at(Checkpoint::ReadonlyCore);
        let preflight = assess_final_readonly(root, cancel);
        progress.at(Checkpoint::CancelCheckpoint);
        if let Err(legacy) = recovery_fs::check(cancel) {
            progress.known(Cause::CancellationObserved);
            return Err(legacy);
        }
        progress.at(Checkpoint::OwnerValidation);
        lease.validate()?;
        progress.at(Checkpoint::OwnershipFence);
        if ownership_contract == OwnershipContract::Opaque { progress.may(Effect::OpaqueCallback); }
        ownership()?;
        progress.at(Checkpoint::CoreDecision);
        if preflight.outcome() != PreliminaryRecoveryOutcome::MatchesAtInspection {
            progress.known(Cause::PreflightNotMatched);
            return Err(PREFLIGHT_UNCONFIRMED.into());
        }
        progress.at(Checkpoint::CorePlanBinding);
        if !preflight.matches_plan(&plan) {
            progress.known(Cause::PlanContextChanged);
            return Err(CHANGED.into());
        }
        progress.at(Checkpoint::OfferedCurrentBinding);
        if !expected.matches_current(&preflight, &plan, lease) {
            progress.known(Cause::OfferedContextUnconfirmed);
            return Err(CHANGED.into());
        }
        progress.at(Checkpoint::FreshPlanRead);
        let current_plan = analyze(root, cancel)?;
        progress.at(Checkpoint::FreshPlanDecision);
        if current_plan.id != plan.id || current_plan.choice != plan.choice {
            progress.known(Cause::PlanContextChanged);
            return Err(CHANGED.into());
        }
        progress.at(Checkpoint::LedgerRead);
        compatible_ledger(root, &dir, &set, cancel)?;
        progress.at(Checkpoint::CancelCheckpoint);
        if let Err(legacy) = recovery_fs::check(cancel) {
            progress.known(Cause::CancellationObserved);
            return Err(legacy);
        }
        progress.at(Checkpoint::OwnerValidation);
        lease.validate()?;
        progress.at(Checkpoint::OwnershipFence);
        if ownership_contract == OwnershipContract::Opaque { progress.may(Effect::OpaqueCallback); }
        ownership()?;
        progress.at(Checkpoint::CancelCheckpoint);
        if let Err(legacy) = recovery_fs::check(cancel) {
            progress.known(Cause::CancellationObserved);
            return Err(legacy);
        }
    }
    progress.at(Checkpoint::JournalPresence);
    let journal = if dir.exists("execution.json")? {
        progress.at(Checkpoint::InheritedJournalRead);
        let j: Journal = decode(
            &dir.read("execution.json", 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?;
        progress.at(Checkpoint::InheritedJournalHeader);
        if j.version != 1 || j.id != id {
            return Err(INCOMPLETE.into());
        }
        progress.at(Checkpoint::InheritedBlockedRead);
        read_snapshot(&dir.child("blocked")?, cancel)?;
        progress.inherited(InheritedFact::JournalAndBlockedRead);
        j
    } else {
        progress.at(Checkpoint::FreshSnapshot);
        let (current, raw) = snapshot(root, cancel)?;
        if dir.exists("blocked")? {
            progress.at(Checkpoint::ExistingBlockedRead);
            if read_snapshot(&dir.child("blocked")?, cancel)? != current {
                return Err(CHANGED.into());
            }
        } else {
            progress.at(Checkpoint::BlockedCreate);
            progress.may(Effect::Protection);
            let blocked = dir.create("blocked")?;
            progress.at(Checkpoint::BlockedSnapshotWrite);
            progress.may(Effect::Protection);
            write_snapshot(&blocked, &current, &raw, cancel, u64::MAX, &|_, _, _| {})?;
        }
        // This durable backup is the blocked state, distinct from the pre-Save set.
        progress.at(Checkpoint::BlockedVerify);
        verify_files(root, &current, cancel)?;
        progress.at(Checkpoint::BackupHook);
        progress.may(Effect::OpaqueCallback);
        hook("backup_complete")?;
        progress.at(Checkpoint::CancelCheckpoint);
        if let Err(legacy) = recovery_fs::check(cancel) {
            progress.known(Cause::CancellationObserved);
            return Err(legacy);
        }
        progress.at(Checkpoint::FinalTargetPrepare);
        let mut final_files = current.files.clone();
        let final_live;
        match plan.choice {
            Choice::RestorePrevious => {
                final_live = set.prior.live.clone().ok_or(INCOMPLETE)?;
                for name in ["config.json", "mpd-v2.db"] {
                    final_files.insert(
                        name.into(),
                        set.prior.files.get(name).ok_or(INCOMPLETE)?.clone(),
                    );
                }
                if let Some(cache) = &set.binding.queue_cache {
                    let old = set.prior.files.get("queue_cache.json").ok_or(INCOMPLETE)?;
                    let current_queue = current.files.get("queue_cache.json").ok_or(INCOMPLETE)?;
                    let is_cleared = raw
                        .get("queue_cache.json")
                        .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
                        .is_some_and(|v| {
                            v.get("version").and_then(|v| v.as_u64()) == Some(2)
                                && v.get("tracks")
                                    .and_then(|v| v.as_array())
                                    .is_some_and(Vec::is_empty)
                        });
                    if old == current_queue || is_cleared {
                        final_files.insert("queue_cache.json".into(), proof(Some(cache)));
                    }
                }
            }
            Choice::FinishConfirmed => {
                let confirmed = if dir.exists("completion.json")? {
                    decode(
                        &dir.read("completion.json", 64 * 1024 * 1024, cancel)?
                            .ok_or(INCOMPLETE)?,
                    )?
                } else {
                    read_snapshot(&dir.child("confirmed")?, cancel)?
                };
                if !content_files_match(&current, &confirmed) {
                    return Err(CHANGED.into());
                }
                final_live = confirmed.live.ok_or(INCOMPLETE)?;
            }
        }
        final_files.insert(managed_save_intent::NAME.into(), None);
        final_files.insert(SEAL.into(), None);
        let j = Journal {
            version: 1,
            id: id.into(),
            choice: plan.choice,
            before: current,
            final_files,
            final_live,
        };
        progress.at(Checkpoint::JournalWrite);
        progress.may(Effect::Protection);
        dir.write_new("execution.json", &encode(&j)?)?;
        progress.at(Checkpoint::BackupMarker);
        progress.may(Effect::Protection);
        stage(&dir, "backup-complete.json")?;
        j
    };
    // Bind the immutable local journal actually used below, including resume.
    // Fresh-branch protective writes may already exist if this late check fails.
    progress.at(Checkpoint::LocalJournalBinding);
    if !expected.matches_journal(&journal, lease) {
        progress.known(Cause::LocalJournalContextUnconfirmed);
        return Err(CHANGED.into());
    }
    let fence = || -> Result<(), String> {
        progress.at(Checkpoint::CancelCheckpoint);
        if let Err(legacy) = recovery_fs::check(cancel) {
            progress.known(Cause::CancellationObserved);
            return Err(legacy);
        }
        progress.at(Checkpoint::OwnerValidation);
        lease.validate()?;
        progress.at(Checkpoint::LedgerRead);
        compatible_ledger(root, &dir, &set, cancel)?;
        progress.at(Checkpoint::IntermediateFence);
        verify_intermediate(root, &dir, &journal, cancel)
    };
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::OriginsRead);
    origins(root, &journal.final_live, cancel)?;
    progress.at(Checkpoint::CommittedStateRead);
    if dir.exists("committed.json")? {
        progress.inherited(InheritedFact::CommittedNameObservedNotCompleted);
        progress.at(Checkpoint::CommittedStateRead);
        if Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live)
            || root.exists("library.next")?
        {
            return Err(CHANGED.into());
        }
        for (name, p) in &journal.final_files {
            if name == managed_save_intent::NAME || name == SEAL {
                continue;
            }
            progress.at(Checkpoint::CommittedStateRead);
            let bytes = root.read(
                name,
                if name == "mpd-v2.db" {
                    LIMIT
                } else {
                    16 * 1024 * 1024
                },
                cancel,
            )?;
            if proof(bytes.as_deref()).as_ref() != p.as_ref() {
                return Err(CHANGED.into());
            }
        }
    }
    progress.at(Checkpoint::CatalogInputRead);
    let candidate = if journal.choice == Choice::RestorePrevious {
        file_bytes(&dir, "mpd-v2.db", cancel)?
    } else {
        root.read("mpd-v2.db", LIMIT, cancel)?.ok_or(INCOMPLETE)?
    };
    progress.at(Checkpoint::CatalogExpectedRead);
    let expected: HashSet<String> = if journal.choice == Choice::RestorePrevious {
        set.catalog.iter().cloned().collect()
    } else {
        if dir.exists("completion-catalog.json")? {
            decode::<BTreeSet<String>>(
                &dir.read("completion-catalog.json", 64 * 1024 * 1024, cancel)?
                    .ok_or(INCOMPLETE)?,
            )?
            .into_iter()
            .collect()
        } else {
            decode::<BTreeSet<String>>(
                &dir.child("confirmed")?
                    .read("catalog.json", 64 * 1024 * 1024, cancel)?
                    .ok_or(INCOMPLETE)?,
            )?
            .into_iter()
            .collect()
        }
    };
    progress.at(Checkpoint::CatalogBeforeApply);
    progress.may(Effect::ValidatorScratchChildUnknown);
    if validate(&candidate, cancel)? != expected {
        progress.known(Cause::CatalogMismatchObserved);
        return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into());
    }
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::ApplyingMarker);
    progress.may(Effect::Protection);
    stage(&dir, "applying.json")?;
    progress.at(Checkpoint::ApplyingHook);
    progress.may(Effect::OpaqueCallback);
    hook("applying")?;
    progress.at(Checkpoint::GenerationRead);
    if Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live) {
        fence()?;
        progress.at(Checkpoint::GenerationRead);
        if Tree::capture(root, "library.next", cancel)?.as_ref() != Some(&journal.final_live) {
            return Err(CHANGED.into());
        }
        progress.at(Checkpoint::GenerationExchange);
        progress.may(Effect::LiveGenerationNamespace);
        crate::local_media::mirror::atomic_exchange(
            &root.entry("library")?,
            &root.entry("library.next")?,
        )?;
        progress.at(Checkpoint::RootSync);
        root.sync()?;
        progress.at(Checkpoint::ExchangeHook);
        progress.may(Effect::OpaqueCallback);
        hook("exchanged")?;
    }
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::NextRead);
    if root.exists("library.next")? {
        progress.at(Checkpoint::GenerationRead);
        if Tree::capture(root, "library.next", cancel)?.as_ref() != expected_next(&journal) {
            return Err(CHANGED.into());
        }
        progress.at(Checkpoint::NextRetention);
        progress.may(Effect::LiveGenerationNamespace);
        progress.may(Effect::Retention);
        root.rename("library.next", &dir, "retained-next", true)?;
        progress.at(Checkpoint::NextRetentionHook);
        progress.may(Effect::OpaqueCallback);
        hook("staging_retained")?;
    } else if let Some(expected) = expected_next(&journal) {
        if Tree::capture(&dir, "retained-next", cancel)?.as_ref() != Some(expected) {
            return Err(CHANGED.into());
        }
    }
    progress.at(Checkpoint::ControlRead);
    for name in ["config.json", "mpd-v2.db", "queue_cache.json"] {
        fence()?;
        progress.at(Checkpoint::ControlRead);
        let current = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        progress.at(Checkpoint::ControlProofDecision);
        if proof(current.as_deref()).as_ref()
            != journal.final_files.get(name).ok_or(INCOMPLETE)?.as_ref()
        {
            if journal.choice != Choice::RestorePrevious {
                return Err(CHANGED.into());
            }
            progress.at(Checkpoint::ControlBackupRead);
            let bytes = if name == "queue_cache.json" {
                set.binding.queue_cache.clone().ok_or(INCOMPLETE)?
            } else {
                file_bytes(&dir, name, cancel)?
            };
            progress.at(Checkpoint::ControlProofDecision);
            if proof(Some(&bytes)).as_ref()
                != journal.final_files.get(name).ok_or(INCOMPLETE)?.as_ref()
            {
                return Err(INCOMPLETE.into());
            }
            let temporary = format!("{name}.writing");
            progress.at(Checkpoint::ControlTemporaryRead);
            if !root.exists(&temporary)? {
                progress.at(Checkpoint::ControlPrepare);
                progress.may(Effect::ControlTemporary);
                root.write_new(&temporary, &bytes)?;
            }
            progress.at(Checkpoint::ControlTemporaryRead);
            if root.read(&temporary, LIMIT, cancel)?.as_deref() != Some(bytes.as_slice()) {
                return Err(CHANGED.into());
            }
            progress.at(Checkpoint::ControlTemporaryRead);
            let identity = root.file_identity(&temporary)?.ok_or(INCOMPLETE)?;
            let effect = format!("effect-{name}.json");
            progress.at(Checkpoint::ControlEffectRead);
            if dir.exists(&effect)? {
                let expected: Identity =
                    decode(&dir.read(&effect, 4096, cancel)?.ok_or(INCOMPLETE)?)?;
                if expected != identity {
                    return Err(CHANGED.into());
                }
            } else {
                progress.at(Checkpoint::ControlProofWrite);
                progress.may(Effect::Protection);
                dir.write_new(&effect, &encode(&identity)?)?;
            }
            fence()?;
            progress.at(Checkpoint::ControlReplace);
            progress.may(Effect::CanonicalControl);
            progress.may(Effect::ControlTemporary);
            root.replace(
                name,
                &bytes,
                journal
                    .before
                    .identities
                    .get(name)
                    .ok_or(INCOMPLETE)?
                    .as_ref(),
            )?;
            progress.at(Checkpoint::ControlHook);
            progress.may(Effect::OpaqueCallback);
            hook(name)?;
        }
        let temporary = format!("{name}.writing");
        progress.at(Checkpoint::ControlTemporaryRead);
        if root.exists(&temporary)? {
            fence()?;
            progress.at(Checkpoint::ControlProofDecision);
            let expected = journal
                .before
                .identities
                .get(name)
                .ok_or(INCOMPLETE)?
                .as_ref()
                .ok_or(INCOMPLETE)?;
            progress.at(Checkpoint::ControlTemporaryRead);
            if root.file_identity(&temporary)?.as_ref() != Some(expected) {
                return Err(CHANGED.into());
            }
            progress.at(Checkpoint::ControlRetention);
            progress.may(Effect::ControlTemporary);
            progress.may(Effect::Retention);
            root.rename(&temporary, &dir, &format!("retained-control-{name}"), true)?;
        }
    }
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::ValidatingMarker);
    progress.may(Effect::Protection);
    stage(&dir, "validating.json")?;
    progress.at(Checkpoint::ValidatingHook);
    progress.may(Effect::OpaqueCallback);
    hook("validating")?;
    progress.at(Checkpoint::PostDatabaseRead);
    let db = root.read("mpd-v2.db", LIMIT, cancel)?.ok_or(INCOMPLETE)?;
    progress.at(Checkpoint::CatalogAfterApply);
    progress.may(Effect::ValidatorScratchChildUnknown);
    if validate(&db, cancel)? != expected {
        progress.known(Cause::CatalogMismatchObserved);
        return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into());
    }
    progress.at(Checkpoint::OriginsRead);
    origins(root, &journal.final_live, cancel)?;
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::FinalControlsRead);
    for (name, p) in &journal.final_files {
        if name == managed_save_intent::NAME || name == SEAL {
            continue;
        }
        progress.at(Checkpoint::FinalControlsRead);
        let actual = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        if proof(actual.as_deref()).as_ref() != p.as_ref() {
            return Err(CHANGED.into());
        }
    }
    progress.at(Checkpoint::GenerationRead);
    if Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live)
        || root.exists("library.next")?
    {
        return Err(CHANGED.into());
    }
    progress.at(Checkpoint::RootSync);
    root.sync()?;
    progress.at(Checkpoint::CommittedMarker);
    progress.may(Effect::Protection);
    stage(&dir, "committed.json")?;
    progress.at(Checkpoint::CommittedHook);
    progress.may(Effect::OpaqueCallback);
    hook("committed")?;
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::IntentRead);
    if let Some(intent) = root.read(managed_save_intent::NAME, 262144, cancel)? {
        if recovery_fs::hash(&intent) != set.binding.record_hash {
            return Err(CHANGED.into());
        }
        progress.at(Checkpoint::IntentRetention);
        progress.may(Effect::CanonicalControl);
        progress.may(Effect::Retention);
        root.rename(managed_save_intent::NAME, &dir, "retained-intent", true)?;
        progress.at(Checkpoint::IntentRetentionHook);
        progress.may(Effect::OpaqueCallback);
        hook("intent_retained")?;
    } else if !dir.exists("retained-intent")?
        && journal
            .before
            .files
            .get(managed_save_intent::NAME)
            .is_some_and(Option::is_some)
    {
        return Err(CHANGED.into());
    }
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::SealRead);
    if root.exists(SEAL)? {
        progress.at(Checkpoint::SealRetention);
        progress.may(Effect::CanonicalControl);
        progress.may(Effect::Retention);
        root.rename(SEAL, &dir, "retained-seal", true)?;
    } else if !dir.exists("retained-seal")? {
        return Err(INCOMPLETE.into());
    }
    progress.at(Checkpoint::FenceBoundary);
    fence()?;
    progress.at(Checkpoint::RootSync);
    root.sync()?;
    progress.at(Checkpoint::FinalizedMarker);
    progress.may(Effect::Protection);
    stage(&dir, "finalized.json")?;
    progress.at(Checkpoint::FinalizedHook);
    progress.may(Effect::OpaqueCallback);
    hook("finalized")?;
    progress.at(Checkpoint::RetireOpaqueSubprotocol);
    progress.may(Effect::Protection);
    progress.may(Effect::Retention);
    progress.may(Effect::BackupNamespace);
    retire(lease, &dir, cancel, &|| {
        progress.at(Checkpoint::OwnershipFence);
        if ownership_contract == OwnershipContract::Opaque { progress.may(Effect::OpaqueCallback); }
        ownership()?;
        fence()?;
        progress.at(Checkpoint::RetireReadyControls);
        for (name, expected) in &journal.final_files {
            progress.at(Checkpoint::CommittedStateRead);
            let bytes = root.read(
                name,
                if name == "mpd-v2.db" {
                    LIMIT
                } else {
                    16 * 1024 * 1024
                },
                cancel,
            )?;
            if proof(bytes.as_deref()).as_ref() != expected.as_ref() {
                return Err(CHANGED.into());
            }
        }
        progress.at(Checkpoint::RetireReadyGeneration);
        if root.exists("library.next")?
            || Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live)
        {
            return Err(CHANGED.into());
        }
        Ok(())
    }).map_err(|legacy|progress.retire_failure(legacy))?;
    Ok(ResultDto {
        completed: true,
        backup_retained: true,
        requires_reopen: true,
    })
    })();
    result.map_err(|legacy|progress.failure(legacy))
}
fn owned_names(dir: &Dir) -> Result<Vec<String>, String> {
    let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.file.as_raw_fd()));
    fs::read_dir(path)
        .map_err(|_| UNSAFE)?
        .map(|e| {
            e.map_err(|_| UNSAFE.into())
                .and_then(|e| e.file_name().into_string().map_err(|_| UNSAFE.into()))
        })
        .collect()
}
/// Delete only an identified, finalized app-produced last set after a newer
/// complete set exists. No generic profile/backups/quarantine cleanup.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DiscardEntry {
    Directory {
        identity: Identity,
    },
    File {
        identity: Identity,
        hash: String,
    },
    Link {
        identity: Identity,
        target: std::path::PathBuf,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Discard {
    profile: Identity,
    root: Identity,
    entries: BTreeMap<std::path::PathBuf, DiscardEntry>,
}
const DISCARD: &str = ".sonante-recovery.discard.json";
fn discard_inventory(
    dir: &Dir,
    prefix: &Path,
    out: &mut BTreeMap<std::path::PathBuf, DiscardEntry>,
    cancel: &dyn Fn() -> bool,
    depth: usize,
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if depth > 130 || out.len() > 262144 {
        return Err(INCOMPLETE.into());
    }
    for name in owned_names(dir)? {
        recovery_fs::check(cancel)?;
        let path = dir.entry(&name)?;
        let m = fs::symlink_metadata(&path).map_err(|_| CHANGED)?;
        let id = Identity {
            dev: m.dev(),
            ino: m.ino(),
        };
        let rel = prefix.join(&name);
        let entry = if m.is_dir() {
            let child = dir.child(&name)?;
            discard_inventory(&child, &rel, out, cancel, depth + 1)?;
            DiscardEntry::Directory { identity: id }
        } else if m.file_type().is_symlink() {
            DiscardEntry::Link {
                identity: id,
                target: fs::read_link(path).map_err(|_| CHANGED)?,
            }
        } else {
            let bytes = dir
                .read(&name, 256 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?;
            DiscardEntry::File {
                identity: id,
                hash: recovery_fs::hash(&bytes),
            }
        };
        out.insert(rel, entry);
    }
    dir.validate()
}
fn discard_parent(root: &Dir, path: &Path) -> Result<Option<(Vec<Dir>, String)>, String> {
    let parts = path.components().collect::<Vec<_>>();
    if parts.is_empty() {
        return Err(UNSAFE.into());
    }
    let mut anchors = vec![Dir::open(&root.path)?];
    for part in &parts[..parts.len() - 1] {
        let std::path::Component::Normal(n) = part else {
            return Err(UNSAFE.into());
        };
        let n = n.to_str().ok_or(UNSAFE)?;
        let parent = anchors.last().ok_or(INCOMPLETE)?;
        if !parent.exists(n)? {
            return Ok(None);
        }
        let child = parent.child(n)?;
        anchors.push(child);
    }
    let std::path::Component::Normal(name) = parts[parts.len() - 1] else {
        return Err(UNSAFE.into());
    };
    Ok(Some((anchors, name.to_str().ok_or(UNSAFE)?.into())))
}
fn discard_last(root: &Dir, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let receipt: Discard = if root.exists(DISCARD)? {
        decode(
            &root
                .read(DISCARD, 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?
    } else {
        if !root.exists(LAST)? {
            return Ok(());
        }
        let dir = root.child(LAST)?;
        if dir.read("finalized.json", 8, cancel)?.as_deref() != Some(b"1") {
            return Err(INCOMPLETE.into());
        }
        let set: Set = decode(
            &dir.read(MANIFEST, 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?;
        if set.version != 1
            || set.prior.profile != root.id()
            || read_snapshot(&dir, cancel)? != set.prior
        {
            return Err(INCOMPLETE.into());
        }
        let retained: Seal = decode(&dir.read("retained-seal", 4096, cancel)?.ok_or(INCOMPLETE)?)?;
        if retained.version != 1
            || retained.profile != root.id()
            || retained.manifest_hash
                != recovery_fs::hash(
                    &dir.read(MANIFEST, 64 * 1024 * 1024, cancel)?
                        .ok_or(INCOMPLETE)?,
                )
            || retained.record_hash != set.binding.record_hash
        {
            return Err(INCOMPLETE.into());
        }
        let allowed = [
            "files",
            "snapshot.json",
            MANIFEST,
            "confirmed",
            "blocked",
            "execution.json",
            "retained-next",
            "retained-intent",
            "retained-seal",
            "retained-control-config.json",
            "retained-control-mpd-v2.db",
            "retained-control-queue_cache.json",
            "completion.json",
            "completion-catalog.json",
            "effect-config.json.json",
            "effect-mpd-v2.db.json",
            "effect-queue_cache.json.json",
            "backup-complete.json",
            "applying.json",
            "validating.json",
            "committed.json",
            "finalized.json",
        ];
        if owned_names(&dir)?
            .iter()
            .any(|n| !allowed.contains(&n.as_str()))
        {
            return Err(UNSAFE.into());
        }
        if dir.exists("retained-next")? {
            let j: Journal = decode(
                &dir.read("execution.json", 64 * 1024 * 1024, cancel)?
                    .ok_or(INCOMPLETE)?,
            )?;
            if Tree::capture(&dir, "retained-next", cancel)?.as_ref() != expected_next(&j) {
                return Err(CHANGED.into());
            }
        }
        for name in ["confirmed", "blocked"] {
            if dir.exists(name)? {
                let child = dir.child(name)?;
                let allowed = if name == "confirmed" {
                    vec!["files", "snapshot.json", "catalog.json", "complete.json"]
                } else {
                    vec!["files", "snapshot.json"]
                };
                if owned_names(&child)?
                    .iter()
                    .any(|n| !allowed.contains(&n.as_str()))
                {
                    return Err(UNSAFE.into());
                }
                read_snapshot(&child, cancel)?;
            }
        }
        if dir.exists("retained-intent")? {
            if recovery_fs::hash(
                &dir.read("retained-intent", 262144, cancel)?
                    .ok_or(INCOMPLETE)?,
            ) != set.binding.record_hash
            {
                return Err(INCOMPLETE.into());
            }
        }
        if dir.exists("execution.json")? {
            let j: Journal = decode(
                &dir.read("execution.json", 64 * 1024 * 1024, cancel)?
                    .ok_or(INCOMPLETE)?,
            )?;
            if j.version != 1 || j.before.profile != root.id() {
                return Err(INCOMPLETE.into());
            }
        }
        for name in [
            "backup-complete.json",
            "applying.json",
            "validating.json",
            "committed.json",
            "finalized.json",
        ] {
            if dir.exists(name)? && dir.read(name, 8, cancel)?.as_deref() != Some(b"1") {
                return Err(INCOMPLETE.into());
            }
        }
        let mut entries = BTreeMap::new();
        discard_inventory(&dir, Path::new(""), &mut entries, cancel, 0)?;
        let result = Discard {
            profile: root.id(),
            root: dir.id(),
            entries,
        };
        root.write_new(DISCARD, &encode(&result)?)?;
        result
    };
    if receipt.profile != root.id() {
        return Err(CHANGED.into());
    }
    if root.exists(LAST)? {
        let dir = root.child(LAST)?;
        if dir.id() != receipt.root {
            return Err(CHANGED.into());
        }
        let mut current = BTreeMap::new();
        discard_inventory(&dir, Path::new(""), &mut current, cancel, 0)?;
        // Missing entries can only be an interrupted deletion. Every remaining entry
        // must still have the inode/content proven before deletion first began.
        for (path, entry) in &current {
            if serde_json::to_vec(entry).map_err(|_| INCOMPLETE)?
                != serde_json::to_vec(receipt.entries.get(path).ok_or(CHANGED)?)
                    .map_err(|_| INCOMPLETE)?
            {
                return Err(CHANGED.into());
            }
        }
        let mut paths = receipt.entries.iter().collect::<Vec<_>>();
        paths.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        for (path, entry) in paths {
            recovery_fs::check(cancel)?;
            dir.validate()?;
            let Some((anchors, name)) = discard_parent(&dir, path)? else {
                continue;
            };
            let parent = anchors.last().ok_or(INCOMPLETE)?;
            if !parent.exists(&name)? {
                continue;
            }
            use std::os::unix::fs::MetadataExt;
            let p = parent.entry(&name)?;
            let m = fs::symlink_metadata(&p).map_err(|_| CHANGED)?;
            let identity = Identity {
                dev: m.dev(),
                ino: m.ino(),
            };
            let expected = match entry {
                DiscardEntry::Directory { identity }
                | DiscardEntry::File { identity, .. }
                | DiscardEntry::Link { identity, .. } => identity,
            };
            if &identity != expected {
                return Err(CHANGED.into());
            }
            match entry {
                DiscardEntry::Directory { .. } => {
                    let child = parent.child(&name)?;
                    if !owned_names(&child)?.is_empty() {
                        return Err(CHANGED.into());
                    }
                    fs::remove_dir(p).map_err(|_| CHANGED)?;
                }
                DiscardEntry::File { hash, .. } => {
                    if recovery_fs::hash(
                        &parent
                            .read(&name, 256 * 1024 * 1024, cancel)?
                            .ok_or(INCOMPLETE)?,
                    ) != *hash
                    {
                        return Err(CHANGED.into());
                    }
                    fs::remove_file(p).map_err(|_| CHANGED)?;
                }
                DiscardEntry::Link { target, .. } => {
                    if fs::read_link(&p).map_err(|_| CHANGED)? != *target {
                        return Err(CHANGED.into());
                    }
                    fs::remove_file(p).map_err(|_| CHANGED)?;
                }
            }
            parent.sync()?;
        }
        dir.sync()?;
        dir.validate()?;
        fs::remove_dir(root.entry(LAST)?).map_err(|_| CHANGED)?;
        root.sync()?;
    }
    root.read(DISCARD, 64 * 1024 * 1024, cancel)?
        .ok_or(INCOMPLETE)?;
    fs::remove_file(root.entry(DISCARD)?).map_err(|_| CHANGED)?;
    root.sync()
}

fn retire(
    lease: &Lease,
    dir: &Dir,
    cancel: &dyn Fn() -> bool,
    ready: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    recovery_fs::check(cancel)?;
    lease.validate()?;
    if dir.read("finalized.json", 8, cancel)?.as_deref() != Some(b"1") {
        return Err(INCOMPLETE.into());
    }
    discard_last(lease.root(), cancel).map_err(|e| format!("{e}: retention"))?;
    ready().map_err(|e| format!("{e}: ready"))?;
    recovery_fs::check(cancel)?;
    lease.validate()?;
    dir.validate()?;
    lease.root().rename(PENDING, lease.root(), LAST, true)
}
pub(crate) fn finish_save(
    lease: &Lease,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
) -> Result<(), String> {
    lease.validate()?;
    let root = lease.root();
    let (dir, set) = open_set(root, cancel)?;
    if root.exists(managed_save_intent::NAME)? || root.exists("library.next")? {
        return Err(INCOMPLETE.into());
    }
    let (view, raw) = snapshot(root, cancel)?;
    let catalog = validate(raw.get("mpd-v2.db").ok_or(INCOMPLETE)?, cancel)?;
    catalog_scope(&catalog, view.live.as_ref().ok_or(INCOMPLETE)?)?;
    if dir.exists("confirmed")? {
        let proof = read_snapshot(&dir.child("confirmed")?, cancel)?;
        if !content_files_match(&view, &proof) {
            return Err(CHANGED.into());
        }
    } else {
        if view.live != set.prior.live
            || managed_save_intent::recovery_config_hash(raw.get("config.json").ok_or(INCOMPLETE)?)?
                != set.binding.previous_config
            || catalog != set.catalog.iter().cloned().collect::<HashSet<_>>()
        {
            return Err(CHANGED.into());
        }
    }
    verify_files(root, &view, cancel)?;
    dir.write_new("completion.json", &encode(&view)?)?;
    dir.write_new(
        "completion-catalog.json",
        &encode(&catalog.into_iter().collect::<BTreeSet<_>>())?,
    )?;
    root.sync()?;
    if root.exists(SEAL)? {
        root.rename(SEAL, &dir, "retained-seal", true)?;
    } else {
        return Err(INCOMPLETE.into());
    }
    let mut final_view = view.clone();
    final_view.files.insert(SEAL.into(), None);
    final_view.identities.insert(SEAL.into(), None);
    stage(&dir, "finalized.json")?;
    retire(lease, &dir, cancel, &|| {
        verify_files(root, &final_view, cancel)?;
        if Tree::capture(root, "library", cancel)? != view.live || root.exists("library.next")? {
            return Err(CHANGED.into());
        }
        Ok(())
    })
}

pub(crate) fn plan_for_profile(
    root: &Dir,
    runtime: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<Plan, String> {
    let (_, set) = open_set(root, cancel)?;
    crate::profile_lease::legacy_fence(root, &set.scope, set.creator, &set.start, runtime)?;
    analyze(root, cancel)
}
pub(crate) fn validate_queue_capture(
    bytes: Option<&[u8]>,
    tracks: &[crate::audio::TrackMetadata],
) -> Result<(), String> {
    let Some(bytes) = bytes else {
        return if tracks.is_empty() {
            Ok(())
        } else {
            Err("LIBRARY_RECOVERY_QUEUE_UNPROVEN".into())
        };
    };
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| INCOMPLETE)?;
    let list = if value.is_array() {
        value.clone()
    } else {
        if value.get("version").and_then(|v| v.as_u64()) != Some(2) {
            return Err(INCOMPLETE.into());
        }
        value.get("tracks").cloned().ok_or(INCOMPLETE)?
    };
    let cached: Vec<crate::audio::TrackMetadata> =
        serde_json::from_value(list).map_err(|_| INCOMPLETE)?;
    let identity =
        |track: &crate::audio::TrackMetadata| -> Result<crate::audio::MediaLocator, String> {
            let locator =
                track
                    .media_locator
                    .clone()
                    .unwrap_or_else(|| crate::audio::MediaLocator::Local {
                        uri: track.uri.clone(),
                    });
            match &locator {
                crate::audio::MediaLocator::Upnp { .. } | crate::audio::MediaLocator::Radio { .. } => return Err("LIBRARY_RECOVERY_QUEUE_INVALID".into()),
                crate::audio::MediaLocator::Local { uri } => {
                    if !crate::local_media::is_safe_relative_mpd_uri(uri) {
                        return Err(INCOMPLETE.into());
                    }
                }
                crate::audio::MediaLocator::Plex {
                    server_id,
                    part_key,
                    ..
                } => {
                    if server_id.trim().is_empty()
                        || part_key.trim().is_empty()
                        || server_id.contains("://")
                        || part_key.contains("://")
                        || crate::plex::contains_plex_token(part_key)
                    {
                        return Err(INCOMPLETE.into());
                    }
                }
            }
            Ok(locator)
        };
    if cached.len() != tracks.len() {
        return Err("LIBRARY_RECOVERY_QUEUE_CHANGED".into());
    }
    for (a, b) in cached.iter().zip(tracks) {
        if identity(a)? != identity(b)? {
            return Err("LIBRARY_RECOVERY_QUEUE_CHANGED".into());
        }
    }
    if let Some(resume) = value.get("resume") {
        let cursor: crate::audio::PlaybackResumeSnapshot =
            serde_json::from_value(resume.clone()).map_err(|_| INCOMPLETE)?;
        if cursor.queue_index >= tracks.len()
            || !cursor.elapsed.is_finite()
            || cursor.elapsed < 0.0
            || cursor.state == crate::audio::PlaybackState::Stopped
        {
            return Err(INCOMPLETE.into());
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn execute_fixture(
    lease: &Lease,
    id: &str,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
) -> Result<ResultDto, String> {
    // Legacy COMPONENT helper only: fresh context cannot prove offer continuity.
    // No fabricated historical checksum or bypass of the production guard.
    let plan = analyze(lease.root(), cancel)?;
    if plan.id != id { return Err(CHANGED.into()); }
    let core = assess_final_readonly(lease.root(), cancel);
    let expected = core.into_offer_binding(0, &plan, lease)?;
    execute_bound_fixture(lease, id, &expected, cancel, validate, hook)
}
#[cfg(test)]
fn execute_bound_fixture(
    lease: &Lease, id: &str, expected: &OfferedRecoveryBinding,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
) -> Result<ResultDto, String> {
    execute_checked(lease, id, expected, cancel, validate, hook, &|| Ok(()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn insufficient_space_does_not_publish_a_complete_snapshot() {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("sonante-space-{}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        let dir = Dir::open(&path).unwrap();
        let view = Snapshot {
            profile: dir.id(),
            files: Default::default(),
            identities: Default::default(),
            live: None,
            next: None,
        };
        assert_eq!(
            write_snapshot(&dir, &view, &BTreeMap::new(), &|| false, 0, &|_, _, _| {}).unwrap_err(),
            "LIBRARY_RECOVERY_BACKUP_NO_SPACE"
        );
        assert!(!dir.exists("snapshot.json").unwrap());
        assert!(!dir.exists("files").unwrap());
        fs::remove_dir(path).unwrap();
    }
}

pub(crate) fn validate_reopen_state(path: &Path) -> Result<(), String> {
    let root = Dir::open(path)?;
    let dir = root.child(LAST)?;
    if dir.read("finalized.json", 8, &|| false)?.as_deref() != Some(b"1") {
        return Err(INCOMPLETE.into());
    }
    let journal: Journal = decode(
        &dir.read("execution.json", 64 * 1024 * 1024, &|| false)?
            .ok_or(INCOMPLETE)?,
    )?;
    verify_intermediate(&root, &dir, &journal, &|| false)?;
    for (name, expected) in &journal.final_files {
        let bytes = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            &|| false,
        )?;
        if proof(bytes.as_deref()).as_ref() != expected.as_ref() {
            return Err(CHANGED.into());
        }
    }
    if root.exists("library.next")?
        || Tree::capture(&root, "library", &|| false)?.as_ref() != Some(&journal.final_live)
    {
        return Err(CHANGED.into());
    }
    Ok(())
}

// FF04-I1a: disconnected readonly core. Existing analyze/execute/origins unchanged.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum PreliminaryFinalRole { Previous, Completion, Confirmed, JournalResume, JournalCommitted }
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum PreliminaryRecoveryReason {
    Historical(crate::source_identity::PreliminaryHistoricalReason),
    SnapshotChanged, ReadUnconfirmed, UnsafeOrAmbiguous, IncompleteSet,
    Cancelled, StructuralUnconfirmed,
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum PreliminaryRecoveryOutcome { MatchesAtInspection, Ineligible, Unconfirmed }
#[cfg(test)]
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum NotExecutionAdmission { PreliminaryOnly }
/// Not serialized, not Debug: selected tree and plan hash stay backend-private.
struct PreliminaryFinalSelection { role:PreliminaryFinalRole, choice:Choice, plan_id:String, tree:Tree, observed_profile:Identity }
/// Private RAM data, not a Lease, admission token, serialized witness or credential.
/// Deliberately no Debug/Serialize/Deserialize or payload getters.
pub(crate) struct OfferedRecoveryBinding {
    analysis_epoch:u64, observed_profile:Identity, plan_id:String,
    choice:Choice, final_live:Tree,
}
impl OfferedRecoveryBinding {
    pub(crate) fn matches_offer(&self,actual_epoch:u64,plan:&Plan)->bool {
        self.analysis_epoch==actual_epoch && self.plan_id==plan.id && self.choice==plan.choice
    }
    pub(crate) fn matches_owner(&self,owner:&Lease)->bool {
        owner.validate().is_ok() && self.observed_profile==owner.root().id()
    }
    fn matches_current(&self,core:&PreliminaryRecoveryAssessment,plan:&Plan,owner:&Lease)->bool {
        core.outcome==PreliminaryRecoveryOutcome::MatchesAtInspection
            && self.matches_owner(owner) && self.plan_id==plan.id && self.choice==plan.choice
            && core.selected.as_ref().is_some_and(|s|s.observed_profile==self.observed_profile
                && s.plan_id==self.plan_id && s.choice==self.choice && s.tree==self.final_live)
    }
    fn matches_journal(&self,journal:&Journal,owner:&Lease)->bool {
        self.matches_owner(owner) && journal.id==self.plan_id && journal.choice==self.choice
            && journal.final_live==self.final_live
    }
}
pub(crate) struct PreliminaryRecoveryAssessment {
    outcome:PreliminaryRecoveryOutcome, reason:Option<PreliminaryRecoveryReason>,
    selected:Option<PreliminaryFinalSelection>,
}
impl PreliminaryRecoveryAssessment {
    pub(crate) fn into_offer_binding(self,actual_epoch:u64,plan:&Plan,owner:&Lease)->Result<OfferedRecoveryBinding,String> {
        if self.outcome!=PreliminaryRecoveryOutcome::MatchesAtInspection {
            return Err(PREFLIGHT_UNCONFIRMED.into());
        }
        if !self.matches_plan(plan) || owner.validate().is_err() {return Err(CHANGED.into());}
        let selected=self.selected.ok_or(CHANGED)?;
        if selected.observed_profile!=owner.root().id() {return Err(CHANGED.into());}
        Ok(OfferedRecoveryBinding{analysis_epoch:actual_epoch,observed_profile:selected.observed_profile,
            plan_id:selected.plan_id,choice:selected.choice,final_live:selected.tree})
    }
    pub(crate) fn outcome(&self)->PreliminaryRecoveryOutcome{self.outcome}
    pub(crate) fn reason(&self)->Option<PreliminaryRecoveryReason>{self.reason}
    pub(crate) fn role(&self)->Option<PreliminaryFinalRole>{self.selected.as_ref().map(|s|s.role)}
    #[cfg(test)]
    pub(crate) fn admission(&self)->NotExecutionAdmission{NotExecutionAdmission::PreliminaryOnly}
    /// Backend plan equality only, never a Lease or execution-admission check.
    pub(crate) fn matches_plan(&self,plan:&Plan)->bool {
        self.selected.as_ref().is_some_and(|s|s.plan_id==plan.id && s.choice==plan.choice)
    }
}
#[derive(Clone,PartialEq,Eq)]
struct PreliminaryReadStamp { dev:u64,ino:u64,uid:u32,mode:u32,links:u64,len:u64,mtime:(i64,i64),ctime:(i64,i64) }
impl PreliminaryReadStamp {
    fn of(m:&fs::Metadata)->Self {
        use std::os::unix::fs::MetadataExt;
        Self{dev:m.dev(),ino:m.ino(),uid:m.uid(),mode:m.mode(),links:m.nlink(),len:m.len(),
            mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())}
    }
}
struct PreliminaryDirectory { dir:Dir, stamp:PreliminaryReadStamp }
struct PreliminaryFile { slot:usize,name:String,limit:u64,stamp:Option<PreliminaryReadStamp>,bytes:Option<Vec<u8>> }
struct PreliminaryTree { slot:usize,name:String,tree:Option<Tree> }
/// All directory descriptors remain alive while nested /proc/self/fd names are used.
struct PreliminaryEvidence<'a> {
    root:&'a Dir,root_stamp:PreliminaryReadStamp,dirs:Vec<PreliminaryDirectory>,
    files:Vec<PreliminaryFile>,trees:Vec<PreliminaryTree>,pending:usize,old_files:usize,
}
impl<'a> PreliminaryEvidence<'a> {
    fn dir(&self,slot:usize)->&Dir{if slot==0{self.root}else{&self.dirs[slot-1].dir}}
    fn check(cancel:&dyn Fn()->bool)->Result<(),PreliminaryRecoveryReason>{
        if cancel(){Err(PreliminaryRecoveryReason::Cancelled)}else{Ok(())}
    }
    fn stamp(dir:&Dir,name:&str)->Result<Option<PreliminaryReadStamp>,PreliminaryRecoveryReason>{
        let path=dir.entry(name).map_err(|_|PreliminaryRecoveryReason::UnsafeOrAmbiguous)?;
        match fs::symlink_metadata(path){Ok(m)=>Ok(Some(PreliminaryReadStamp::of(&m))),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(_)=>Err(PreliminaryRecoveryReason::ReadUnconfirmed)}
    }
    fn directory_stamp(dir:&Dir)->Result<PreliminaryReadStamp,PreliminaryRecoveryReason>{
        dir.validate().map_err(|_|PreliminaryRecoveryReason::SnapshotChanged)?;
        Ok(PreliminaryReadStamp::of(&dir.file.metadata().map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)?))
    }
    fn child(&mut self,parent:usize,name:&str)->Result<Option<usize>,PreliminaryRecoveryReason>{
        if !self.dir(parent).exists(name).map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)? {return Ok(None);}
        let dir=self.dir(parent).child(name).map_err(|_|PreliminaryRecoveryReason::UnsafeOrAmbiguous)?;
        let stamp=Self::directory_stamp(&dir)?;
        self.dirs.push(PreliminaryDirectory{dir,stamp});Ok(Some(self.dirs.len()))
    }
    fn read(&mut self,slot:usize,name:&str,limit:u64,json:bool,marker:bool,cancel:&dyn Fn()->bool)
        ->Result<(),PreliminaryRecoveryReason>{
        Self::check(cancel)?;
        let dir=self.dir(slot);let before=Self::stamp(dir,name)?;
        let bytes=dir.read(name,limit,cancel).map_err(|_|if cancel(){PreliminaryRecoveryReason::Cancelled}else{PreliminaryRecoveryReason::ReadUnconfirmed})?;
        if Self::stamp(dir,name)?!=before || before.is_some()!=bytes.is_some(){return Err(PreliminaryRecoveryReason::SnapshotChanged);}
        if let Some(ref bytes)=bytes {
            if json {crate::source_identity::recovery_validate_unique(bytes,limit)
                .map_err(|_|PreliminaryRecoveryReason::UnsafeOrAmbiguous)?;}
            if marker && bytes.as_slice()!=b"1" {return Err(PreliminaryRecoveryReason::UnsafeOrAmbiguous);}
        }
        self.files.push(PreliminaryFile{slot,name:name.into(),limit,stamp:before,bytes});Ok(())
    }
    fn bytes(&self,slot:usize,name:&str)->Result<&[u8],PreliminaryRecoveryReason>{
        self.files.iter().find(|f|f.slot==slot&&f.name==name).and_then(|f|f.bytes.as_deref()).ok_or(PreliminaryRecoveryReason::IncompleteSet)
    }
    fn present(&self,slot:usize,name:&str)->bool{
        self.files.iter().any(|f|f.slot==slot&&f.name==name&&f.bytes.is_some())
    }
    fn controls(&mut self,slot:usize,cancel:&dyn Fn()->bool)->Result<(),PreliminaryRecoveryReason>{
        let mut total=0u64;
        for name in CONTROLS {
            let cap=if name=="mpd-v2.db"{LIMIT}else if name=="local-source-identities.lock"{32}else if name==SEAL{4096}else{16*1024*1024};
            // Config/stores/queue are raw snapshot data. Corrupt current config
            // can be a legitimate reason to restore; do not parse it as authority.
            self.read(slot,name,cap,name==managed_save_intent::NAME,false,cancel)?;
            total+=self.files.last().and_then(|f|f.bytes.as_ref()).map_or(0,|b|b.len() as u64);
            if total>256*1024*1024{return Err(PreliminaryRecoveryReason::ReadUnconfirmed);}
        } Ok(())
    }
    fn snapshot_dir(&mut self,parent:usize,name:&str,cancel:&dyn Fn()->bool)->Result<Option<usize>,PreliminaryRecoveryReason>{
        let Some(slot)=self.child(parent,name)? else{return Ok(None)};
        self.read(slot,"snapshot.json",64*1024*1024,true,false,cancel)?;
        let files=self.child(slot,"files")?.ok_or(PreliminaryRecoveryReason::IncompleteSet)?;
        // Pre-check the finite11-name schema before old read_snapshot's collector.
        for item in fs::read_dir(format!("/proc/self/fd/{}",self.dir(files).file.as_raw_fd())).map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)? {
            let name=item.map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)?.file_name();
            if !CONTROLS.iter().any(|n|name==std::ffi::OsStr::new(n)){return Err(PreliminaryRecoveryReason::UnsafeOrAmbiguous);}
            Self::check(cancel)?;
        }
        self.controls(files,cancel)?;
        if name=="confirmed" {
            self.read(slot,"complete.json",8,false,true,cancel)?;
            self.read(slot,"catalog.json",64*1024*1024,true,false,cancel)?;
        }
        Ok(Some(slot))
    }
    fn tree(&mut self,slot:usize,name:&str,cancel:&dyn Fn()->bool)->Result<(),PreliminaryRecoveryReason>{
        let tree=Tree::capture(self.dir(slot),name,cancel).map_err(|_|if cancel(){PreliminaryRecoveryReason::Cancelled}else{PreliminaryRecoveryReason::ReadUnconfirmed})?;
        if let Some(ref t)=tree{preliminary_tree_paths(t)?;}
        self.trees.push(PreliminaryTree{slot,name:name.into(),tree});Ok(())
    }
    fn capture(root:&'a Dir,cancel:&dyn Fn()->bool)->Result<Self,PreliminaryRecoveryReason>{
        Self::check(cancel)?;let root_stamp=Self::directory_stamp(root)?;
        let mut e=Self{root,root_stamp,dirs:Vec::new(),files:Vec::new(),trees:Vec::new(),pending:0,old_files:0};
        e.controls(0,cancel)?;
        for name in ["config.json.writing","mpd-v2.db.writing","queue_cache.json.writing"]{
            e.read(0,name,if name=="mpd-v2.db.writing"{LIMIT}else{16*1024*1024},false,false,cancel)?;
        }
        e.tree(0,"library",cancel)?;e.tree(0,"library.next",cancel)?;
        let pending=e.child(0,PENDING)?.ok_or(PreliminaryRecoveryReason::IncompleteSet)?;e.pending=pending;
        e.read(pending,MANIFEST,64*1024*1024,true,false,cancel)?;
        e.read(pending,"snapshot.json",64*1024*1024,true,false,cancel)?;
        let old_files=e.child(pending,"files")?.ok_or(PreliminaryRecoveryReason::IncompleteSet)?;e.old_files=old_files;
        // Same finite file-name guard as saved snapshots, before open_set.
        for item in fs::read_dir(format!("/proc/self/fd/{}",e.dir(old_files).file.as_raw_fd())).map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)? {
            let name=item.map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)?.file_name();
            if !CONTROLS.iter().any(|n|name==std::ffi::OsStr::new(n)){return Err(PreliminaryRecoveryReason::UnsafeOrAmbiguous);}
            Self::check(cancel)?;
        }
        e.controls(old_files,cancel)?;
        for name in ["execution.json","completion.json","completion-catalog.json"] {e.read(pending,name,64*1024*1024,true,false,cancel)?;}
        for name in ["backup-complete.json","applying.json","validating.json","committed.json","finalized.json"] {e.read(pending,name,8,false,true,cancel)?;}
        e.read(pending,"retained-seal",4096,true,false,cancel)?;
        e.read(pending,"retained-intent",16*1024*1024,true,false,cancel)?;
        for name in CONTROLS {e.read(pending,&format!("effect-{name}.json"),4096,true,false,cancel)?;}
        for name in ["config.json","mpd-v2.db","queue_cache.json"] {
            e.read(pending,&format!("retained-control-{name}"),if name=="mpd-v2.db"{LIMIT}else{16*1024*1024},false,false,cancel)?;
        }
        e.snapshot_dir(pending,"blocked",cancel)?;
        e.snapshot_dir(pending,"confirmed",cancel)?;
        e.tree(pending,"retained-next",cancel)?;
        e.validate_embedded_trees()?;
        e.revalidate(cancel)?;Ok(e)
    }
    fn validate_embedded_trees(&self)->Result<(),PreliminaryRecoveryReason>{
        fn snapshot_trees(s:&Snapshot)->Result<(),PreliminaryRecoveryReason>{
            for t in [&s.live,&s.next].into_iter().flatten(){preliminary_tree_paths(t)?;} Ok(())
        }
        for file in &self.files {
            let Some(ref bytes)=file.bytes else{continue};
            if file.name==MANIFEST {
                let set:Set=decode(bytes).map_err(|_|PreliminaryRecoveryReason::StructuralUnconfirmed)?;
                snapshot_trees(&set.prior)?;
            } else if matches!(file.name.as_str(),"snapshot.json"|"completion.json") {
                let s:Snapshot=decode(bytes).map_err(|_|PreliminaryRecoveryReason::StructuralUnconfirmed)?;snapshot_trees(&s)?;
            } else if file.name=="execution.json" {
                let j:Journal=decode(bytes).map_err(|_|PreliminaryRecoveryReason::StructuralUnconfirmed)?;
                snapshot_trees(&j.before)?;preliminary_tree_paths(&j.final_live)?;
            }
        } Ok(())
    }
    fn revalidate(&self,cancel:&dyn Fn()->bool)->Result<(),PreliminaryRecoveryReason>{
        Self::check(cancel)?;
        if Self::directory_stamp(self.root)?!=self.root_stamp{return Err(PreliminaryRecoveryReason::SnapshotChanged);}
        for d in &self.dirs {if Self::directory_stamp(&d.dir)?!=d.stamp{return Err(PreliminaryRecoveryReason::SnapshotChanged);}}
        for f in &self.files {
            Self::check(cancel)?;
            let dir=self.dir(f.slot);
            if Self::stamp(dir,&f.name)?!=f.stamp{return Err(PreliminaryRecoveryReason::SnapshotChanged);}
            let bytes=dir.read(&f.name,f.limit,cancel).map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)?;
            if bytes!=f.bytes||Self::stamp(dir,&f.name)?!=f.stamp{return Err(PreliminaryRecoveryReason::SnapshotChanged);}
        }
        for t in &self.trees {
            Self::check(cancel)?;
            if Tree::capture(self.dir(t.slot),&t.name,cancel).map_err(|_|PreliminaryRecoveryReason::ReadUnconfirmed)?!=t.tree{return Err(PreliminaryRecoveryReason::SnapshotChanged);}
        }
        Self::check(cancel)
    }
}
fn preliminary_tree_paths(tree:&Tree)->Result<(),PreliminaryRecoveryReason>{
    for path in tree.entries.keys() {
        if path.as_os_str().len()>4096 || !crate::local_media::is_safe_relative_mpd_uri(path.to_str().ok_or(PreliminaryRecoveryReason::UnsafeOrAmbiguous)?) {
            return Err(PreliminaryRecoveryReason::UnsafeOrAmbiguous);
        }
    }
    for entry in tree.entries.values() {
        if matches!(entry,TreeEntry::Manifest{bytes} if bytes.len()>262144){return Err(PreliminaryRecoveryReason::UnsafeOrAmbiguous);}
    }
    Ok(())
}
fn preliminary_manifest(tree:&Tree)->Result<crate::local_media::mirror::MirrorManifest,PreliminaryRecoveryReason>{
    let bytes=match tree.entries.get(Path::new(crate::local_media::mirror::MANIFEST_NAME)) {
        Some(TreeEntry::Manifest{bytes})=>bytes,_=>return Err(PreliminaryRecoveryReason::IncompleteSet),
    };
    let _:crate::local_media::mirror::MirrorManifest=crate::source_identity::recovery_decode_unique(bytes,262144)
        .map_err(|_|PreliminaryRecoveryReason::UnsafeOrAmbiguous)?;
    preliminary_tree_paths(tree)?;
    // Existing Tree entry/depth guard remains coarse; no new hard-cap claim.
    manifest(tree).map_err(|_|PreliminaryRecoveryReason::StructuralUnconfirmed)
}
fn preliminary_select(e:&PreliminaryEvidence<'_>,cancel:&dyn Fn()->bool)->Result<PreliminaryFinalSelection,PreliminaryRecoveryReason>{
    use PreliminaryRecoveryReason::*;
    let root=e.root;let p=e.pending;
    let (dir,set)=open_set(root,cancel).map_err(|_|StructuralUnconfirmed)?;
    // Validate the exact additional manifest consumed by compatible_ledger, not
    // every incidental current tree: a damaged non-final tree may be restorable.
    let ledger_tree=if e.present(p,"completion.json") {
        decode::<Snapshot>(e.bytes(p,"completion.json")?).map_err(|_|StructuralUnconfirmed)?.live.ok_or(IncompleteSet)?
    }else if dir.exists("confirmed").map_err(|_|ReadUnconfirmed)? {
        read_snapshot(&dir.child("confirmed").map_err(|_|IncompleteSet)?,cancel).map_err(|_|StructuralUnconfirmed)?.live.ok_or(IncompleteSet)?
    }else{set.prior.live.clone().ok_or(IncompleteSet)?};
    preliminary_manifest(&ledger_tree)?;
    compatible_ledger(root,&dir,&set,cancel).map_err(|_|StructuralUnconfirmed)?;
    let plan=analyze(root,cancel).map_err(|_|if cancel(){Cancelled}else{StructuralUnconfirmed})?;
    let (role,tree)=if e.present(p,"execution.json") {
        let j:Journal=decode(e.bytes(p,"execution.json")?).map_err(|_|StructuralUnconfirmed)?;
        if j.version!=1 || j.id!=plan.id || j.choice!=plan.choice {return Err(StructuralUnconfirmed);}
        verify_intermediate(root,&dir,&j,cancel).map_err(|_|StructuralUnconfirmed)?;
        read_snapshot(&dir.child("blocked").map_err(|_|IncompleteSet)?,cancel).map_err(|_|StructuralUnconfirmed)?;
        let role=if e.present(p,"committed.json") {
            if Tree::capture(root,"library",cancel).map_err(|_|ReadUnconfirmed)?.as_ref()!=Some(&j.final_live)
                || root.exists("library.next").map_err(|_|ReadUnconfirmed)? {return Err(SnapshotChanged);}
            for (name,proof) in &j.final_files {
                if name==managed_save_intent::NAME || name==SEAL {continue;}
                let bytes=root.read(name,if name=="mpd-v2.db"{LIMIT}else{16*1024*1024},cancel).map_err(|_|ReadUnconfirmed)?;
                if self::proof(bytes.as_deref()).as_ref()!=proof.as_ref(){return Err(SnapshotChanged);}
            }
            PreliminaryFinalRole::JournalCommitted
        }else{PreliminaryFinalRole::JournalResume};
        (role,j.final_live)
    }else if e.present(p,"completion.json") {
        let s:Snapshot=decode(e.bytes(p,"completion.json")?).map_err(|_|StructuralUnconfirmed)?;
        (PreliminaryFinalRole::Completion,s.live.ok_or(IncompleteSet)?)
    }else if dir.exists("confirmed").map_err(|_|ReadUnconfirmed)? {
        let s=read_snapshot(&dir.child("confirmed").map_err(|_|IncompleteSet)?,cancel).map_err(|_|StructuralUnconfirmed)?;
        (PreliminaryFinalRole::Confirmed,s.live.ok_or(IncompleteSet)?)
    }else{(PreliminaryFinalRole::Previous,set.prior.live.ok_or(IncompleteSet)?)};
    preliminary_manifest(&tree)?;
    Ok(PreliminaryFinalSelection{role,choice:plan.choice,plan_id:plan.id,tree,observed_profile:e.root.id()})
}
/// No production caller. Reads backend-owned Dir only; never calls execute,
/// plan_for_profile/legacy_fence, catalog validator, writer, State or IPC.
pub(crate) fn assess_final_readonly(root:&Dir,cancel:&dyn Fn()->bool)->PreliminaryRecoveryAssessment {
    use PreliminaryRecoveryReason::*;
    use crate::source_identity::{PreliminaryHistoricalContext as H,PreliminaryHistoricalReason as R};
    let mut selected=None;
    let result=(||->Result<R,PreliminaryRecoveryReason>{
        let e=PreliminaryEvidence::capture(root,cancel)?;
        // Check history extension using the actual current pair, never restoring
        // it from old pending/files bytes and never dropping newer reservations.
        let history=H::from_readonly_bytes(e.bytes(e.old_files,"local-source-identities.json")?,
            e.bytes(e.old_files,"local-source-identities.lock")?,e.bytes(0,"local-source-identities.json")?,
            e.bytes(0,"local-source-identities.lock")?).map_err(Historical)?;
        let selection=preliminary_select(&e,cancel)?;
        let manifest=preliminary_manifest(&selection.tree)?;
        let reason=history.assess_manifest(&manifest,cancel);
        selected=Some(selection);
        e.revalidate(cancel)?;
        Ok(reason)
    })();
    let (outcome,reason)=match result {
        Ok(R::MatchesAtInspection)=>(PreliminaryRecoveryOutcome::MatchesAtInspection,None),
        Ok(r)=>{let outcome=match r {R::HistoricalMismatch|R::HistoricalIncomparable|R::RootNotFound|R::LedgerInvalid|R::UnsupportedLedger|R::LedgerExtensionUnconfirmed|R::ManifestBindingUnconfirmed=>PreliminaryRecoveryOutcome::Ineligible,_=>PreliminaryRecoveryOutcome::Unconfirmed};(outcome,Some(Historical(r)))},
        Err(r)=>(PreliminaryRecoveryOutcome::Unconfirmed,Some(r)),
    };
    PreliminaryRecoveryAssessment{outcome,reason,selected}
}

#[cfg(test)]
mod ff04_readonly_tests {
    use super::*;
    use crate::{config::AppConfig,local_media::mirror,managed_save_intent::{SaveIntent,Completed}};
    use std::{cell::Cell,os::unix::fs::{DirBuilderExt,MetadataExt,PermissionsExt},sync::atomic::{AtomicU64,Ordering},path::PathBuf};
    static NEXT:AtomicU64=AtomicU64::new(0);
    // Retain every scene; no Drop deletion. A future separately admitted runner
    // must provision this own parent, collect completion and verify ownership.
    struct Own {
        _parent:Dir, _root:Dir, path:PathBuf, profile:PathBuf, old:PathBuf,
        owner:Lease, previous:AppConfig,next:AppConfig,prepared:mirror::MirrorBuildResult,
        old_catalog:HashSet<String>,next_catalog:HashSet<String>,
    }
    impl Own {
        fn new()->Self {
            let parent=PathBuf::from(std::env::var_os("FF04_I1A_OWN_FIXTURES").expect("separate future own-FS admission required"));
            assert!(parent.is_absolute());assert_eq!(fs::canonicalize(&parent).unwrap(),parent);
            let m=fs::symlink_metadata(&parent).unwrap();assert!(m.is_dir());
            assert_eq!(m.uid(),unsafe{libc::geteuid()});assert_eq!(m.mode()&0o7777,0o700);
            let parent_dir=Dir::open(&parent).unwrap();
            let name=format!("ff04-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::SeqCst));
            fs::DirBuilder::new().mode(0o700).create(parent_dir.entry(&name).unwrap()).unwrap();
            let path=parent.join(&name);let own=Dir::open(&path).unwrap();parent_dir.validate().unwrap();
            let named=fs::symlink_metadata(&path).unwrap();let held=own.file.metadata().unwrap();
            assert_eq!((named.dev(),named.ino()),(held.dev(),held.ino()));
            let profile=path.join("profile");fs::DirBuilder::new().mode(0o700).create(&profile).unwrap();
            let old=path.join("Old");let added=path.join("Added");
            for p in [&old,&added]{fs::create_dir(p).unwrap();fs::write(p.join("track.flac"),b"synthetic metadata only").unwrap();}
            let owner=Lease::acquire(&profile).unwrap();
            let library=profile.join("library");let staging=profile.join("library.next");
            let mirror::ManagedGenerationPreparation::Prepared(initial)=mirror::prepare_managed_generation(&library,&staging,&[old.clone()],"ff04-old").unwrap() else{panic!("initial")};
            mirror::publish_prepared_managed_generation(&library,&initial).unwrap();
            let mirror::ManagedGenerationPreparation::Prepared(prepared)=mirror::prepare_managed_generation(&library,&staging,&[old.clone(),added.clone()],"ff04-next").unwrap() else{panic!("next")};
            let old_catalog=initial.manifest.sources.keys().map(|id|format!("file: {id}/track.flac")).collect();
            let next_catalog=prepared.manifest.sources.keys().map(|id|format!("file: {id}/track.flac")).collect();
            let previous=AppConfig{first_run:false,local_folders:vec![old.to_str().unwrap().into()],..AppConfig::default()};
            let mut next=previous.clone();next.local_folders.push(added.to_str().unwrap().into());
            let f=Self{_parent:parent_dir,_root:own,path,profile,old,owner,previous,next,prepared,old_catalog,next_catalog};
            f.persist(false);fs::write(f.profile.join("mpd-v2.db"),b"own old catalog").unwrap();f
        }
        fn persist(&self,next:bool){crate::persistence::atomic_write_private_synced(&self.profile.join("config.json"),&serde_json::to_vec(if next{&self.next}else{&self.previous}).unwrap(),"config.json").unwrap();}
        fn validator(&self,bytes:&[u8],cancel:&dyn Fn()->bool)->Result<HashSet<String>,String>{
            recovery_fs::check(cancel)?;
            match bytes{b"own old catalog"=>Ok(self.old_catalog.clone()),b"own next catalog"=>Ok(self.next_catalog.clone()),_=>Err("own component DB fixture unconfirmed".into())}
        }
        fn capture(&self)->SaveIntent {
            let intent=SaveIntent::create(&self.profile,&self.previous,&self.next,&self.prepared).unwrap();
            // Actual writer/Lease/set protocol; fake catalog callback is explicitly
            // COMPONENT ONLY. No native validator/production legacy fence proof.
            capture_before(&self.owner,Some(&self.old_catalog),&||false,&|b,c|self.validator(b,c)).unwrap();intent
        }
        fn publish(&self,intent:&SaveIntent)->mirror::ManagedMirrorPublication {
            let p=intent.publish(&self.prepared,None).unwrap();self.persist(true);
            fs::write(self.profile.join("mpd-v2.db"),b"own next catalog").unwrap();p
        }
        fn state(&self)->BTreeMap<PathBuf,(u64,u64,u32,u64,Vec<u8>)>{
            fn walk(root:&Path,p:&Path,out:&mut BTreeMap<PathBuf,(u64,u64,u32,u64,Vec<u8>)>){
                use std::os::unix::ffi::OsStrExt;
                let m=fs::symlink_metadata(p).unwrap();let bytes=if m.file_type().is_symlink(){fs::read_link(p).unwrap().as_os_str().as_bytes().to_vec()}else if m.is_file(){fs::read(p).unwrap()}else{Vec::new()};
                out.insert(p.strip_prefix(root).unwrap().into(),(m.dev(),m.ino(),m.mode(),m.nlink(),bytes));
                if m.is_dir(){for e in fs::read_dir(p).unwrap(){walk(root,&e.unwrap().path(),out);}}
            }
            self._parent.validate().unwrap();self._root.validate().unwrap();
            let mut out=BTreeMap::new();walk(&self.path,&self.path,&mut out);out
        }
        fn readonly<T>(&self,f:impl FnOnce()->T)->T{let before=self.state();let result=f();assert_eq!(before,self.state());result}
    }
    fn assert_parity(f:&Own,role:PreliminaryFinalRole){
        f.readonly(||{
            let e=PreliminaryEvidence::capture(f.owner.root(),&||false).unwrap();
            let selected=preliminary_select(&e,&||false).unwrap();let plan=analyze(f.owner.root(),&||false).unwrap();
            assert_eq!(selected.role,role);assert_eq!(selected.choice,plan.choice);assert_eq!(selected.plan_id,plan.id);
            let a=assess_final_readonly(f.owner.root(),&||false);assert_eq!(a.role(),Some(role));
            assert_eq!(a.admission(),NotExecutionAdmission::PreliminaryOnly);
            assert_eq!(a.outcome(),PreliminaryRecoveryOutcome::MatchesAtInspection);
        });
    }
    #[test] fn readonly_restore_uses_prior_in_live_and_next_not_generic_current(){
        let f=Own::new();let intent=f.capture();assert_parity(&f,PreliminaryFinalRole::Previous);
        f.publish(&intent);assert_parity(&f,PreliminaryFinalRole::Previous);
        let e=PreliminaryEvidence::capture(f.owner.root(),&||false).unwrap();let s=preliminary_select(&e,&||false).unwrap();
        assert_eq!(s.tree,Tree::capture(f.owner.root(),"library.next",&||false).unwrap().unwrap());
        assert_ne!(s.tree,Tree::capture(f.owner.root(),"library",&||false).unwrap().unwrap());
    }
    #[test] fn readonly_confirmed_and_completion_precedence(){
        let f=Own::new();let intent=f.capture();let publication=f.publish(&intent);
        confirm_next(&f.owner,Some(&f.next_catalog),&||false,&|b,c|f.validator(b,c)).unwrap();
        assert_parity(&f,PreliminaryFinalRole::Confirmed);
        publication.cleanup_previous().unwrap();
        intent.complete_transaction(Ok(()),Some(Completed::Committed)).unwrap();
        // Legitimate interruption after finish_save persisted completion evidence,
        // before retirement; no completion/snapshot/checksum is forged.
        let cancel=||f.profile.join(PENDING).join("completion-catalog.json").exists();
        assert!(finish_save(&f.owner,&cancel,&|b,c|f.validator(b,c)).is_err());
        assert_parity(&f,PreliminaryFinalRole::Completion);
    }
    #[test] fn readonly_journal_precedence_and_committed_resume(){
        for (point,role) in [("applying",PreliminaryFinalRole::JournalResume),("exchanged",PreliminaryFinalRole::JournalResume),("committed",PreliminaryFinalRole::JournalCommitted)]{
            let f=Own::new();let intent=f.capture();f.publish(&intent);let plan=analyze(f.owner.root(),&||false).unwrap();
            assert!(execute_fixture(&f.owner,&plan.id,&||false,&|b,c|f.validator(b,c),&|stage|if stage==point{Err("own injected interruption".into())}else{Ok(())}).is_err());
            assert_parity(&f,role);
            assert!(f.profile.join(PENDING).join("execution.json").is_file());
        }
    }
    #[test] fn readonly_finish_journal_keeps_confirmed_final(){
        let f=Own::new();let intent=f.capture();f.publish(&intent);
        confirm_next(&f.owner,None,&||false,&|b,c|f.validator(b,c)).unwrap();
        let plan=analyze(f.owner.root(),&||false).unwrap();assert_eq!(plan.choice,Choice::FinishConfirmed);
        assert!(execute_fixture(&f.owner,&plan.id,&||false,&|b,c|f.validator(b,c),&|s|if s=="applying"{Err("own interrupted".into())}else{Ok(())}).is_err());
        assert_parity(&f,PreliminaryFinalRole::JournalResume);
    }
    #[test] fn readonly_monotonic_ledger_extension_preserved(){
        let f=Own::new();let _intent=f.capture();let extra=f.path.join("Extra");fs::create_dir(&extra).unwrap();
        let current=mirror::read_manifest(&f.profile.join("library")).unwrap().sources.into_iter().map(|(id,p)|(id,Some(p))).collect();
        crate::source_identity::reserve(&f.profile.join("library"),&[f.old.clone(),extra],&current,None).unwrap();
        assert_parity(&f,PreliminaryFinalRole::Previous);
    }
    #[test] fn readonly_absent_and_replaced_originals_are_finite_negatives(){
        for replace in [false,true]{
            let f=Own::new();let _intent=f.capture();fs::rename(&f.old,f.path.join("Old-away")).unwrap();
            if replace{fs::create_dir(&f.old).unwrap();}
            f.readonly(||{let a=assess_final_readonly(f.owner.root(),&||false);
                assert_eq!(a.outcome(),PreliminaryRecoveryOutcome::Ineligible);
                assert_eq!(a.reason(),Some(PreliminaryRecoveryReason::Historical(if replace{crate::source_identity::PreliminaryHistoricalReason::HistoricalMismatch}else{crate::source_identity::PreliminaryHistoricalReason::RootNotFound})));
                assert!(!f.profile.join(PENDING).join("blocked").exists());assert!(!f.profile.join(PENDING).join("execution.json").exists());});
        }
    }
    #[test] fn readonly_corrupt_duplicate_cancel_and_stale_are_not_success(){
        let f=Own::new();let _intent=f.capture();
        f.readonly(||{let a=assess_final_readonly(f.owner.root(),&||true);assert_eq!(a.reason(),Some(PreliminaryRecoveryReason::Cancelled));});
        let e=PreliminaryEvidence::capture(f.owner.root(),&||false).unwrap();
        let p=f.profile.join("local-source-identities.json");fs::write(&p,br#"{"schema_version":1,"schema_version":1}"#).unwrap();
        f.readonly(||{assert!(e.revalidate(&||false).is_err());let a=assess_final_readonly(f.owner.root(),&||false);assert_ne!(a.outcome(),PreliminaryRecoveryOutcome::MatchesAtInspection);});
    }
    #[test] fn readonly_same_bytes_new_inode_and_changed_mid_assessment_refuse(){
        let f=Own::new();let _intent=f.capture();let e=PreliminaryEvidence::capture(f.owner.root(),&||false).unwrap();
        let p=f.profile.join("local-source-identities.lock");let bytes=fs::read(&p).unwrap();fs::rename(&p,f.path.join("anchor-away")).unwrap();
        fs::write(&p,bytes).unwrap();fs::set_permissions(&p,fs::Permissions::from_mode(0o600)).unwrap();
        f.readonly(||assert!(e.revalidate(&||false).is_err()));
        let called=Cell::new(0);let cancel=||{called.set(called.get()+1);called.get()>3};
        f.readonly(||assert_ne!(assess_final_readonly(f.owner.root(),&cancel).outcome(),PreliminaryRecoveryOutcome::MatchesAtInspection));
    }
    // FF04-I1c prepared COMPONENT tests. These reuse Own's legitimate product
    // history/real Lease, but execute_fixture's ownership stub/fake catalog are
    // NOT production process-fence/native-catalog proof. No runtime in this lot.
    #[test] fn ff04_guard_lost_or_replaced_origin_no_new_recovery_writes(){
        for replaced in [false,true]{
            let f=Own::new();let intent=f.capture();f.publish(&intent);
            let plan=analyze(f.owner.root(),&||false).unwrap();
            fs::rename(&f.old,f.path.join("Old-away")).unwrap();
            if replaced{fs::create_dir(&f.old).unwrap();}
            let hooks=Cell::new(0);let validators=Cell::new(0);
            f.readonly(||{
                let error=execute_fixture(&f.owner,&plan.id,&||false,&|b,c|{validators.set(validators.get()+1);f.validator(b,c)},&|_|{hooks.set(hooks.get()+1);Ok(())}).unwrap_err();
                assert_eq!(error,PREFLIGHT_UNCONFIRMED);assert_eq!(hooks.get(),0);assert_eq!(validators.get(),0);
                assert!(!f.profile.join(PENDING).join("blocked").exists());assert!(!f.profile.join(PENDING).join("execution.json").exists());
            });
        }
    }
    #[test] fn ff04_guard_unknown_ambiguous_journal_keeps_inherited_scene(){
        let f=Own::new();let intent=f.capture();f.publish(&intent);let plan=analyze(f.owner.root(),&||false).unwrap();
        assert!(execute_fixture(&f.owner,&plan.id,&||false,&|b,c|f.validator(b,c),&|s|if s=="applying"{Err("own interrupted setup".into())}else{Ok(())}).is_err());
        let path=f.profile.join(PENDING).join("execution.json");let raw=fs::read(&path).unwrap();
        let value:serde_json::Value=serde_json::from_slice(&raw).unwrap();
        let entries=value["value"]["before"]["live"]["entries"].as_object().unwrap();
        let (key,val)=entries.iter().next().unwrap();
        let pair=format!("{}:{}",serde_json::to_string(key).unwrap(),serde_json::to_string(val).unwrap());
        let normalized=serde_json::to_string(&value).unwrap();let needle=format!("\"entries\":{{{pair}");
        assert!(normalized.contains(&needle));
        let duplicate=normalized.replacen(&needle,&format!("\"entries\":{{{pair},{pair}"),1);
        // Negative ambiguity only: retain the ORIGINAL checksum. No positive
        // history/origin/checksum is fabricated. Old BTreeMap parser loses dup.
        assert!(decode::<Journal>(duplicate.as_bytes()).is_ok());
        fs::write(&path,duplicate).unwrap();
        let resumed=analyze(f.owner.root(),&||false).unwrap();
        f.readonly(||{
            let result=execute_fixture(&f.owner,&resumed.id,&||false,&|_,_|panic!("validator after refused guard"),&|_|panic!("hook after refused guard"));
            assert_eq!(result.unwrap_err(),PREFLIGHT_UNCONFIRMED);
        });
    }
    #[test] fn ff04_guard_existing_blocked_without_journal_not_recreated(){
        let f=Own::new();let intent=f.capture();f.publish(&intent);let plan=analyze(f.owner.root(),&||false).unwrap();
        assert!(execute_fixture(&f.owner,&plan.id,&||false,&|b,c|f.validator(b,c),&|s|if s=="backup_complete"{Err("own protected setup".into())}else{Ok(())}).is_err());
        assert!(f.profile.join(PENDING).join("blocked").is_dir());assert!(!f.profile.join(PENDING).join("execution.json").exists());
        fs::rename(&f.old,f.path.join("Old-away")).unwrap();let plan=analyze(f.owner.root(),&||false).unwrap();
        f.readonly(||assert_eq!(execute_fixture(&f.owner,&plan.id,&||false,&|_,_|panic!("no validator"),&|_|panic!("no hook")).unwrap_err(),PREFLIGHT_UNCONFIRMED));
    }
    #[test] fn ff04_guard_resume_applying_exchanged_committed_preserves_old_effects(){
        for point in ["applying","exchanged","committed"]{
            let f=Own::new();let intent=f.capture();f.publish(&intent);let plan=analyze(f.owner.root(),&||false).unwrap();
            assert!(execute_fixture(&f.owner,&plan.id,&||false,&|b,c|f.validator(b,c),&|s|if s==point{Err("own boundary setup".into())}else{Ok(())}).is_err());
            fs::rename(&f.old,f.path.join("Old-away")).unwrap();let resumed=analyze(f.owner.root(),&||false).unwrap();assert!(resumed.resumes_interrupted);
            f.readonly(||assert_eq!(execute_fixture(&f.owner,&resumed.id,&||false,&|_,_|panic!("no validator"),&|_|panic!("no hook")).unwrap_err(),PREFLIGHT_UNCONFIRMED));
        }
    }
    #[test] fn ff04_guard_confirmed_completion_uses_final_sources(){
        for completion in [false,true]{
            let f=Own::new();let intent=f.capture();let publication=f.publish(&intent);
            confirm_next(&f.owner,Some(&f.next_catalog),&||false,&|b,c|f.validator(b,c)).unwrap();
            if completion{
                publication.cleanup_previous().unwrap();intent.complete_transaction(Ok(()),Some(Completed::Committed)).unwrap();
                let cancel=||f.profile.join(PENDING).join("completion-catalog.json").exists();
                assert!(finish_save(&f.owner,&cancel,&|b,c|f.validator(b,c)).is_err());
            }
            let plan=analyze(f.owner.root(),&||false).unwrap();assert_eq!(plan.choice,Choice::FinishConfirmed);
            // Added is ONLY in confirmed/completion final F, not prior F.
            fs::rename(f.path.join("Added"),f.path.join("Added-away")).unwrap();
            f.readonly(||assert_eq!(execute_fixture(&f.owner,&plan.id,&||false,&|_,_|panic!("no validator"),&|_|panic!("no hook")).unwrap_err(),PREFLIGHT_UNCONFIRMED));
        }
    }
    #[test] fn ff04_guard_cancel_invalid_lease_wrong_id_keep_old_failures(){
        let f=Own::new();let intent=f.capture();f.publish(&intent);let plan=analyze(f.owner.root(),&||false).unwrap();
        f.readonly(||{
            assert!(execute_fixture(&f.owner,&plan.id,&||true,&|_,_|panic!("no validator"),&|_|panic!("no hook")).is_err());
            assert_eq!(execute_fixture(&f.owner,"wrong-id",&||false,&|_,_|panic!("no validator"),&|_|panic!("no hook")).unwrap_err(),CHANGED);
        });
        fs::rename(f.profile.join(".sonante-profile.lock"),f.profile.join("owner-lock-away")).unwrap();
        f.readonly(||assert!(execute_fixture(&f.owner,&plan.id,&||false,&|_,_|panic!("no validator"),&|_|panic!("no hook")).is_err()));
    }
    #[test] fn ff04_guard_plan_equality_checks_choice_and_id_without_new_capability(){
        let f=Own::new();let intent=f.capture();f.publish(&intent);let plan=analyze(f.owner.root(),&||false).unwrap();
        f.readonly(||{
            let core=assess_final_readonly(f.owner.root(),&||false);
            assert_eq!(core.outcome(),PreliminaryRecoveryOutcome::MatchesAtInspection);assert!(core.matches_plan(&plan));
            let mut wrong=plan.clone();wrong.choice=Choice::FinishConfirmed;assert!(!core.matches_plan(&wrong));
            wrong=plan.clone();wrong.id.push_str("-different");assert!(!core.matches_plan(&wrong));
            assert_eq!(core.admission(),NotExecutionAdmission::PreliminaryOnly);
            // Unit comparator, not offeredchoice binding across requests.
        });
    }

    fn fault_component_error(f:&Own,plan:&Plan,expected:&OfferedRecoveryBinding,
        validate:&dyn Fn(&[u8],&dyn Fn()->bool)->Result<HashSet<String>,String>,
        hook:&dyn Fn(&str)->Result<(),String>)->ExecutionFailure {
        let progress=ProgressCell::new();progress.may(Effect::OpaqueCallback);
        match execute_checked_detailed(&f.owner,&plan.id,expected,&||false,validate,hook,
            &||Ok(()),&progress,OwnershipContract::Opaque) {
            Err(failure)=>failure,Ok(_)=>panic!("component expected controlled failure"),
        }
    }
    #[test] fn ff04_fault_component_early_owner_failure_has_no_explicit_writer_issue() {
        let f=Own::new();let intent=f.capture();f.publish(&intent);
        let (plan,expected)=binding_before_fault(&f,1);
        fs::rename(f.profile.join(".sonante-profile.lock"),f.profile.join("own-lock-away")).unwrap();
        f.readonly(||{
            let failure=fault_component_error(&f,&plan,&expected,&|_,_|panic!("no validator"),&|_|panic!("no hook"));
            let safe=failure.safe_fault();assert!(safe.progress.cause==Cause::LegacyUnknown);
            assert!(safe.progress.checkpoint==Checkpoint::OwnerValidation);
            assert!(safe.progress.may_have_issued==Effect::OpaqueCallback as u16);
        }); // Entire own scene measured after intentional fault; inherited not pristine.
    }
    #[test] fn ff04_fault_component_origin_loss_saved_binding_and_readonly_scene() {
        let f=Own::new();let intent=f.capture();f.publish(&intent);
        let (plan,expected)=binding_before_fault(&f,1);
        fs::rename(&f.old,f.path.join("Old-away")).unwrap();
        f.readonly(||{
            let failure=fault_component_error(&f,&plan,&expected,&|_,_|panic!("no validator"),&|_|panic!("no hook"));
            let safe=failure.safe_fault();assert!(safe.progress.cause==Cause::PreflightNotMatched);
            assert!(safe.progress.checkpoint==Checkpoint::CoreDecision);
            assert!(safe.progress.may_have_issued==Effect::OpaqueCallback as u16);
            assert!(!f.profile.join(PENDING).join("blocked").exists());
        });
    }
    #[test] fn ff04_fault_component_opaque_validator_keeps_protection_may_and_legacy() {
        let f=Own::new();let intent=f.capture();f.publish(&intent);
        let (plan,expected)=binding_before_fault(&f,1);
        let protected=["library","library.next","config.json","mpd-v2.db","queue_cache.json","favorites.json","playlists.json"];
        let before=f.state();
        let failure=fault_component_error(&f,&plan,&expected,&|_,_|Err("public arbitrary validator failure".into()),&|_|Ok(()));
        let safe=failure.safe_fault();assert!(safe.progress.cause==Cause::LegacyUnknown);
        assert!(safe.progress.checkpoint==Checkpoint::CatalogBeforeApply);
        assert!(safe.progress.may_have_issued & Effect::Protection as u16 != 0);
        assert!(safe.progress.may_have_issued & Effect::ValidatorScratchChildUnknown as u16 != 0);
        assert!(safe.progress.may_have_issued & Effect::LiveGenerationNamespace as u16 == 0);
        assert!(failure.into_legacy().as_bytes()==b"public arbitrary validator failure");
        let after=f.state();
        for name in protected {
            let prefix=Path::new("profile").join(name);
            let a:Vec<_>=before.iter().filter(|(p,_)|p.starts_with(&prefix)).collect();
            let b:Vec<_>=after.iter().filter(|(p,_)|p.starts_with(&prefix)).collect();
            assert!(a==b); // No Debug dump of payload. Limited measured component scope.
        }
        assert!(f.profile.join(PENDING).join("blocked").is_dir());
        assert!(f.profile.join(PENDING).join("execution.json").is_file());
    }
    #[test] fn ff04_fault_component_existing_hook_boundaries_and_inherited_refusal() {
        for (point,checkpoint) in [("applying",Checkpoint::ApplyingHook),("exchanged",Checkpoint::ExchangeHook),("committed",Checkpoint::CommittedHook)] {
            let f=Own::new();let intent=f.capture();f.publish(&intent);
            let (plan,expected)=binding_before_fault(&f,1);
            let failure=fault_component_error(&f,&plan,&expected,&|b,c|f.validator(b,c),
                &|s|if s==point{Err("public own interruption".into())}else{Ok(())});
            let safe=failure.safe_fault();assert!(safe.progress.checkpoint==checkpoint);
            assert!(safe.progress.cause==Cause::LegacyUnknown);
            assert!(safe.progress.may_have_issued & Effect::Protection as u16 != 0);
            if point!="applying"{assert!(safe.progress.may_have_issued & Effect::LiveGenerationNamespace as u16 != 0);}
            assert!(f.profile.join(PENDING).join("execution.json").is_file());
            fs::rename(f.profile.join(".sonante-profile.lock"),f.profile.join("own-lock-away")).unwrap();
            f.readonly(||{
                let refused=fault_component_error(&f,&plan,&expected,&|_,_|panic!("no validator"),&|_|panic!("no hook"));
                assert!(refused.safe_fault().progress.may_have_issued==Effect::OpaqueCallback as u16);
                assert!(refused.safe_fault().progress.inherited_facts==0); // Too early to observe, not absent.
            });
        }
    }

    fn binding_before_fault(f:&Own,epoch:u64)->(Plan,OfferedRecoveryBinding) {
        let plan=analyze(f.owner.root(),&||false).expect("legitimate component plan required");
        let core=assess_final_readonly(f.owner.root(),&||false);
        let binding=match core.into_offer_binding(epoch,&plan,&f.owner) {
            Ok(b)=>b,Err(_)=>panic!("GAP/INCONCLUSIVE: legitimate strong historical support required; no forged positive"),
        };
        (plan,binding)
    }
    #[test] fn ff04_binding_unit_full_f_choice_epoch_plan_profile_and_local_journal() {
        let f=Own::new();let other=Own::new();
        // Synthetic selected values below test equality only, never historical proof.
        let p=Plan{id:"unit-only".into(),choice:Choice::RestorePrevious,previous_tracks:0,
            backup_bytes:0,preserves_current_references:true,resumes_interrupted:false};
        let b=unit_only_synthetic_offer_binding(7,&p,&f.owner);
        assert!(b.matches_offer(7,&p));assert!(!b.matches_offer(8,&p));
        let mut wrong=p.clone();wrong.choice=Choice::FinishConfirmed;assert!(!b.matches_offer(7,&wrong));
        wrong=p.clone();wrong.id.push('x');assert!(!b.matches_offer(7,&wrong));
        assert!(b.matches_owner(&f.owner));assert!(!b.matches_owner(&other.owner));
        let mut core=PreliminaryRecoveryAssessment{outcome:PreliminaryRecoveryOutcome::MatchesAtInspection,reason:None,
            selected:Some(PreliminaryFinalSelection{role:PreliminaryFinalRole::Previous,choice:p.choice,
                plan_id:p.id.clone(),tree:b.final_live.clone(),observed_profile:f.owner.root().id()})};
        assert!(b.matches_current(&core,&p,&f.owner));
        core.selected.as_mut().unwrap().choice=Choice::FinishConfirmed;
        assert!(!b.matches_current(&core,&p,&f.owner));core.selected.as_mut().unwrap().choice=p.choice;
        core.selected.as_mut().unwrap().tree.entries.insert("changed-F".into(),TreeEntry::Directory);
        assert!(!b.matches_current(&core,&p,&f.owner));
        core.selected.as_mut().unwrap().tree=b.final_live.clone();
        for role in [PreliminaryFinalRole::Completion,PreliminaryFinalRole::Confirmed,PreliminaryFinalRole::JournalResume,PreliminaryFinalRole::JournalCommitted] {
            core.selected.as_mut().unwrap().role=role;assert!(b.matches_current(&core,&p,&f.owner));
        }
        let mut j=Journal{version:1,id:p.id.clone(),choice:p.choice,before:Snapshot{profile:f.owner.root().id(),files:BTreeMap::new(),identities:BTreeMap::new(),live:None,next:None},
            final_files:BTreeMap::new(),final_live:b.final_live.clone()};
        assert!(b.matches_journal(&j,&f.owner));
        j.final_live.entries.insert("local-journal-different-F".into(),TreeEntry::Directory);
        assert!(!b.matches_journal(&j,&f.owner));
        j.final_live=b.final_live.clone();j.choice=Choice::FinishConfirmed;assert!(!b.matches_journal(&j,&f.owner));
        j.choice=p.choice;j.id.push('x');assert!(!b.matches_journal(&j,&f.owner));
        assert!(!b.matches_journal(&j,&other.owner));
    }
    #[test] fn ff04_binding_constructor_refuses_unconfirmed_plan_and_wrong_owner() {
        let f=Own::new();let other=Own::new();let _intent=f.capture();
        let (plan,_expected)=binding_before_fault(&f,1);
        let core=assess_final_readonly(f.owner.root(),&||true);
        assert!(core.into_offer_binding(1,&plan,&f.owner).is_err());
        let mut wrong=plan.clone();wrong.choice=Choice::FinishConfirmed;
        assert!(assess_final_readonly(f.owner.root(),&||false).into_offer_binding(1,&wrong,&f.owner).is_err());
        assert!(assess_final_readonly(f.owner.root(),&||false).into_offer_binding(1,&plan,&other.owner).is_err());
    }
    #[test] fn ff04_binding_saved_before_origin_fault_keeps_scene_readonly() {
        let f=Own::new();let intent=f.capture();f.publish(&intent);
        let (plan,expected)=binding_before_fault(&f,1);
        fs::rename(&f.old,f.path.join("Old-away")).unwrap();
        f.readonly(||assert_eq!(execute_bound_fixture(&f.owner,&plan.id,&expected,&||false,
            &|_,_|panic!("no native validator on rejection"),&|_|panic!("no write stage on rejection")).unwrap_err(),PREFLIGHT_UNCONFIRMED));
    }
    #[test] fn ff04_binding_same_f_survives_legitimate_component_resume_roles() {
        for point in ["applying","exchanged","committed"] {
            let f=Own::new();let intent=f.capture();f.publish(&intent);
            let (plan,expected)=binding_before_fault(&f,1);
            assert!(execute_bound_fixture(&f.owner,&plan.id,&expected,&||false,&|b,c|f.validator(b,c),
                &|s|if s==point{Err("own retained interruption".into())}else{Ok(())}).is_err());
            f.readonly(||{
                let resumed=analyze(f.owner.root(),&||false).unwrap();
                assert_eq!(resumed.id,plan.id);
                let core=assess_final_readonly(f.owner.root(),&||false);
                assert!(expected.matches_current(&core,&resumed,&f.owner));
                let (dir,_)=open_set(f.owner.root(),&||false).unwrap();
                let journal:Journal=decode(&dir.read("execution.json",64*1024*1024,&||false).unwrap().unwrap()).unwrap();
                assert!(expected.matches_journal(&journal,&f.owner));
            });
        }
    }
    #[test] fn ff04_binding_ledger_extension_observed_before_offer_no_whole_ledger_freeze() {
        let f=Own::new();let _intent=f.capture();
        let extra=f.path.join("legitimate-extra");fs::create_dir(&extra).unwrap();
        let current=mirror::read_manifest(&f.profile.join("library")).unwrap().sources.into_iter().map(|(id,p)|(id,Some(p))).collect();
        crate::source_identity::reserve(&f.profile.join("library"),&[f.old.clone(),extra],&current,None).unwrap();
        let (plan,expected)=binding_before_fault(&f,1);
        f.readonly(||assert!(expected.matches_current(&assess_final_readonly(f.owner.root(),&||false),&plan,&f.owner)));
        // Extension after offer may still change the existing Plan.id; not relaxed.
    }

}

// Explicit UNIT data only, no historical/protocol/origin/production-admission proof.
// Available only in cfg(test), with no production fallback or serde path.
#[cfg(test)]
pub(crate) fn unit_only_synthetic_offer_binding(epoch:u64,plan:&Plan,owner:&Lease)->OfferedRecoveryBinding {
    OfferedRecoveryBinding{analysis_epoch:epoch,observed_profile:owner.root().id(),
        plan_id:plan.id.clone(),choice:plan.choice,
        final_live:Tree{identity:owner.root().id(),entries:BTreeMap::from([
            ("unit-private-Tree-sentinel".into(),TreeEntry::Manifest{bytes:b"unit-private-manifest-not-history".to_vec()})])}}
}

#[cfg(test)]
mod ff04_fault_unit_tests {
    use super::*;
    fn has(p:ExecutionProgress,e:Effect)->bool {p.may_have_issued & e as u16 != 0}
    // Pure finite-data UNIT tests; no fixture, Lease or production authority proof.
    #[test] fn ff04_fault_adapter_exact_arbitrary_text_and_safe_finite_copy() {
        let p=ProgressCell::new();p.at(Checkpoint::CatalogBeforeApply);
        p.may(Effect::ValidatorScratchChildUnknown);
        let bytes=b"public-unit-sentinel arbitrary error: ready /not-a-personal-path\ncode=CHANGED";
        let failure=p.failure(String::from_utf8(bytes.to_vec()).unwrap());
        let safe=failure.safe_fault();
        assert!(safe.progress.cause==Cause::LegacyUnknown);
        assert!(safe.progress.checkpoint==Checkpoint::CatalogBeforeApply);
        assert!(has(safe.progress,Effect::ValidatorScratchChildUnknown));
        let legacy=failure.into_legacy();assert!(legacy.as_bytes()==bytes);
    }
    #[test] fn ff04_fault_checkpoint_resets_cause_but_never_accumulated_effects() {
        let p=ProgressCell::new();p.may(Effect::Protection);p.known(Cause::PlanContextChanged);
        p.inherited(InheritedFact::JournalAndBlockedRead);
        p.at(Checkpoint::GenerationExchange);p.may(Effect::LiveGenerationNamespace);
        let before=p.0.get();assert!(before.cause==Cause::LegacyUnknown);
        assert!(has(before,Effect::Protection)&&has(before,Effect::LiveGenerationNamespace));
        p.may(Effect::Protection);p.at(Checkpoint::ControlPrepare);p.may(Effect::ControlTemporary);
        let after=p.0.get();assert!(after.may_have_issued & before.may_have_issued==before.may_have_issued);
        assert!(after.inherited_facts==before.inherited_facts);
        p.known(Cause::OfferedContextUnconfirmed);
        let err=p.failure("public-unit-only".into());assert!(has(err.safe_fault().progress,Effect::ControlTemporary));
        assert!(p.0.get()==err.safe_fault().progress);
    }
    #[test] fn ff04_fault_may_before_first_issue_and_no_clear_on_result() {
        let p=ProgressCell::new();p.at(Checkpoint::BlockedCreate);p.may(Effect::Protection);
        // UNIT issuer returns Err after the annotation; no filesystem call is made.
        let issue=||Err::<(),String>("public first-write failure".into());
        let failure=match issue(){Err(e)=>p.failure(e),Ok(())=>panic!("unit expected failure")};
        assert!(has(failure.safe_fault().progress,Effect::Protection));
        assert!(failure.safe_fault().progress.cause==Cause::LegacyUnknown);
        let _success=Ok::<(),String>(());p.at(Checkpoint::RootSync);
        assert!(has(p.0.get(),Effect::Protection));
    }
    #[test] fn ff04_fault_same_legacy_code_never_selects_cause_or_stage() {
        let early=ProgressCell::new();early.at(Checkpoint::CorePlanBinding);early.known(Cause::PlanContextChanged);
        let late=ProgressCell::new();late.at(Checkpoint::ControlReplace);late.may(Effect::CanonicalControl);
        let a=early.failure(CHANGED.into());let b=late.failure(CHANGED.into());
        assert!(a.safe_fault().progress.cause==Cause::PlanContextChanged);
        assert!(b.safe_fault().progress.cause==Cause::LegacyUnknown);
        assert!(a.safe_fault().progress.checkpoint!=b.safe_fault().progress.checkpoint);
        assert!(a.into_legacy().as_bytes()==b.into_legacy().as_bytes());
    }
    #[test] fn ff04_fault_retire_opaque_error_retains_all_bits_and_original_text() {
        let p=ProgressCell::new();p.may(Effect::Protection);p.may(Effect::Retention);p.may(Effect::BackupNamespace);
        p.inherited(InheritedFact::CommittedNameObservedNotCompleted);
        // Simulate ready returning Ok and a later opaque helper failure. No IO.
        p.at(Checkpoint::RetireReadyGeneration);p.known(Cause::PlanContextChanged);
        let original="public arbitrary retention: ready: unknown".to_string();
        let returned=p.retire_failure(original.clone());let err=p.failure(returned);
        let f=err.safe_fault();assert!(f.progress.checkpoint==Checkpoint::RetireOpaqueSubprotocol);
        assert!(f.progress.cause==Cause::LegacyUnknown);
        assert!(has(f.progress,Effect::Protection)&&has(f.progress,Effect::Retention)&&has(f.progress,Effect::BackupNamespace));
        assert!(f.progress.inherited_facts==InheritedFact::CommittedNameObservedNotCompleted as u8);
        assert!(err.into_legacy().as_bytes()==original.as_bytes());
    }
    #[test] fn ff04_fault_unknown_is_saturated_not_missing_equals_zero() {
        let missing=ExecutionFault::unavailable();
        assert!(missing.progress.knowledge==Knowledge::Unknown);
        assert!(missing.progress.checkpoint==Checkpoint::Unknown);
        assert!(missing.progress.cause==Cause::LegacyUnknown);
        assert!(missing.progress.may_have_issued==u16::MAX&&missing.progress.inherited_facts==0);
    }
    #[test] fn ff04_fault_inherited_facts_do_not_mean_new_effect_or_completed() {
        let p=ProgressCell::new();p.inherited(InheritedFact::JournalAndBlockedRead);
        p.inherited(InheritedFact::CommittedNameObservedNotCompleted);
        let before=p.0.get();assert!(before.may_have_issued==0);
        p.at(Checkpoint::OwnerValidation);let err=p.failure("public inherited refusal".into());
        assert!(err.safe_fault().progress.inherited_facts==before.inherited_facts);
        assert!(err.safe_fault().progress.knowledge==Knowledge::Available);
        // No complete/pristine/reopen field or no-writes decision exists here.
    }
}

#[cfg(test)]
mod failure_projection_unit_text {
    use super::*;
    #[test] fn projection_unit_unknown_saturates_without_inventing_observed_history() {
        let s=FailureSummary::unknown();assert!(s.knowledge()==FailureKnowledge::UnknownBoundary);
        assert!(s.may_include(PossibleEffect::OpaqueCallback));
        assert!(!s.observed(ObservedInherited::JournalAndBlockedRead));
    }
    #[test] fn projection_unit_legacy_exact_and_finite_summary_only() {
        // Synthetic finite data UNIT, not actual backend provenance or physical proof.
        let progress=ProgressCell::new();progress.at(Checkpoint::RetireOpaqueSubprotocol);
        progress.may(Effect::Retention);let failure=progress.failure("public-unit-legacy-sentinel".into());
        let summary=FailureSummary::from_fault(failure.safe_fault());
        let projected=ProjectedExecutionFailure{summary,legacy:failure.legacy};
        let(s,legacy)=projected.into_parts();assert!(legacy.as_bytes()==b"public-unit-legacy-sentinel");
        assert!(s.class()==FailureClass::LegacyUnknown && s.boundary()==FailureBoundary::CommitRetention);
    }
    #[test] fn projection_unit_known_class_not_from_legacy_text() {
        let progress=ProgressCell::new();progress.at(Checkpoint::CoreDecision);
        progress.known(Cause::PreflightNotMatched);
        let f=progress.failure("public unrelated arbitrary String".into());
        let summary=FailureSummary::from_fault(f.safe_fault());
        assert!(summary.class()==FailureClass::PreflightUnconfirmed);
        assert!(summary.knowledge()==FailureKnowledge::ExecutorCaptured);
        assert!(summary.boundary()==FailureBoundary::ReadonlyPreflight);
        assert!(f.into_legacy().as_bytes()==b"public unrelated arbitrary String");
    }
    #[test] fn projection_unit_shared_checks_after_effects_stay_protocol_category() {
        for checkpoint in [Checkpoint::OwnerValidation,Checkpoint::CancelCheckpoint,
            Checkpoint::LedgerRead,Checkpoint::IntermediateFence,Checkpoint::FenceBoundary,
            Checkpoint::OriginsRead,Checkpoint::RootSync] {
            let p=ProgressCell::new();p.may(Effect::CanonicalControl);p.may(Effect::OpaqueCallback);
            p.at(checkpoint);let f=p.failure(CHANGED.into());let summary=FailureSummary::from_fault(f.safe_fault());
            assert!(summary.boundary()==FailureBoundary::ProtocolCheck);
            assert!(summary.class()==FailureClass::LegacyUnknown);
            assert!(summary.may_include(PossibleEffect::CanonicalControl));
            assert!(summary.may_include(PossibleEffect::OpaqueCallback));
        }
    }
    #[test] fn projection_unit_cumulative_and_inherited_are_independent_observations() {
        let p=ProgressCell::new();p.may(Effect::Protection);p.at(Checkpoint::ControlReplace);
        p.may(Effect::CanonicalControl);p.inherited(InheritedFact::JournalAndBlockedRead);
        let f=p.failure("public unit".into());let summary=FailureSummary::from_fault(f.safe_fault());
        assert!(summary.may_include(PossibleEffect::Protection));
        assert!(summary.may_include(PossibleEffect::CanonicalControl));
        assert!(summary.observed(ObservedInherited::JournalAndBlockedRead));
        assert!(!summary.observed(ObservedInherited::CommittedNameNotCompleted)); // Not observed, not absent.
    }
    #[test] fn projection_unit_unknown_fault_saturates_all_domains_without_history() {
        let summary=FailureSummary::from_fault(ExecutionFault::unavailable());
        assert!(summary.knowledge()==FailureKnowledge::UnknownBoundary);
        for effect in [PossibleEffect::Protection,PossibleEffect::LiveGenerationNamespace,
            PossibleEffect::ControlTemporary,PossibleEffect::CanonicalControl,PossibleEffect::Retention,
            PossibleEffect::BackupNamespace,PossibleEffect::ValidatorOrChildUnknown,PossibleEffect::OpaqueCallback] {
            assert!(summary.may_include(effect));
        }
        assert!(!summary.observed(ObservedInherited::JournalAndBlockedRead));
        assert!(!summary.observed(ObservedInherited::CommittedNameNotCompleted));
    }

}

#[cfg(test)]
mod precommit_database_tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn precommit_database_proof_reuses_only_same_profile_and_bytes_without_count_limit() {
        let profile = Identity { dev: 1, ino: 2 };
        let records = (0..57_223).map(|n|format!("file: Source/Album{}/track{n}.flac", n/30)).collect::<HashSet<_>>();
        let calls = Cell::new(0);
        let proof = prepare_database_bytes(profile.clone(), b"immutable database", Some(&records), &||false,
            &|_,_| {calls.set(calls.get()+1);Ok(records.clone())}).unwrap();
        assert_eq!(proof.validate_bytes(&profile,b"immutable database",&||false).unwrap(),records);
        assert_eq!(calls.get(),1,"confirmation must not reopen another validator");
        assert_eq!(proof.validate_bytes(&Identity {dev:1,ino:3},b"immutable database",&||false).unwrap_err(),CHANGED);
        assert_eq!(proof.validate_bytes(&profile,b"changed database",&||false).unwrap_err(),CHANGED);
        assert_eq!(proof.validate_bytes(&profile,b"immutable database",&||true).unwrap_err(),recovery_fs::CANCELLED);
    }

    #[test]
    fn precommit_database_refuses_validator_error_mismatch_and_late_cancellation() {
        let profile=Identity {dev:1,ino:2};
        assert!(matches!(prepare_database_bytes(profile.clone(),b"db",None,&||false,
            &|_,_|Err("LIBRARY_RECOVERY_DATABASE_INVALID".into())),Err(error) if error=="LIBRARY_RECOVERY_DATABASE_INVALID"));
        let expected=HashSet::from(["file: Source/track.flac".into()]);
        assert!(matches!(prepare_database_bytes(profile.clone(),b"db",Some(&expected),&||false,
            &|_,_|Ok(HashSet::new())),Err(error) if error=="LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH"));
        let cancelled=Cell::new(false);
        assert!(matches!(prepare_database_bytes(profile,b"db",None,&||cancelled.get(),
            &|_,_|{cancelled.set(true);Ok(expected.clone())}),Err(error) if error==recovery_fs::CANCELLED));
    }

    #[test]
    fn precommit_database_integrates_owned_lease_confirmation_and_recovered_database() {
        use crate::{config::AppConfig, local_media::mirror, managed_save_intent::{SaveIntent, Completed}};
        use std::os::unix::fs::DirBuilderExt;
        for recovered in [false, true] {
            let mut entropy=[0u8;16];
            use std::io::Read;
            fs::File::open("/dev/urandom").unwrap().read_exact(&mut entropy).unwrap();
            let base=std::env::temp_dir().join(format!("sonante-precommit-{}",&recovery_fs::hash(&entropy)[..16]));
            fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
            let profile=base.join("profile");let old=base.join("Old");let added=base.join("Added");
            for path in [&profile,&old,&added] {fs::DirBuilder::new().mode(0o700).create(path).unwrap();}
            for path in [&old,&added] {fs::write(path.join("track.flac"),b"metadata fixture only").unwrap();}
            let owner=Lease::acquire(&profile).unwrap();
            let library=profile.join("library");let staging=profile.join("library.next");
            let mirror::ManagedGenerationPreparation::Prepared(initial)=mirror::prepare_managed_generation(&library,&staging,&[old.clone()],"precommit-old").unwrap() else {panic!("initial generation")};
            mirror::publish_prepared_managed_generation(&library,&initial).unwrap();
            let old_catalog=initial.manifest.sources.keys().map(|id|format!("file: {id}/track.flac")).collect::<HashSet<_>>();
            let previous=AppConfig {first_run:false,local_folders:vec![old.to_str().unwrap().into()],..AppConfig::default()};
            crate::persistence::atomic_write_private_synced(&profile.join("config.json"),&serde_json::to_vec(&previous).unwrap(),"config.json").unwrap();
            owner.root().write_new("mpd-v2.db",b"old").unwrap();
            let mirror::ManagedGenerationPreparation::Prepared(prepared)=mirror::prepare_managed_generation(&library,&staging,&[old.clone(),added.clone()],"precommit-new").unwrap() else {panic!("new generation")};
            let new_catalog=prepared.manifest.sources.keys().map(|id|format!("file: {id}/track.flac")).collect::<HashSet<_>>();
            let mut next=previous.clone();next.local_folders.push(added.to_str().unwrap().into());
            let intent=SaveIntent::create(&profile,&previous,&next,&prepared).unwrap();
            let calls=Cell::new(0);
            let validator=|bytes:&[u8],cancel:&dyn Fn()->bool| {
                recovery_fs::check(cancel)?;calls.set(calls.get()+1);
                match bytes {b"old"=>Ok(old_catalog.clone()),b"new"=>Ok(new_catalog.clone()),_=>Err("fixture database refused".into())}
            };
            capture_before(&owner,Some(&old_catalog),&||false,&validator).unwrap();
            let publication=intent.publish(&prepared,None).unwrap();
            fs::write(profile.join("mpd-v2.db"),b"new").unwrap();
            let proof=prepare_database(&owner,Some(&new_catalog),&||false,&validator).unwrap();
            assert_eq!(calls.get(),2,"one old and one new validation");
            proof.validate_current(&owner,&||false).unwrap();
            fs::write(profile.join("mpd-v2.db"),b"changed").unwrap();
            assert_eq!(proof.validate_current(&owner,&||false).unwrap_err(),CHANGED);
            assert_eq!(AppConfig::load_from_path(&profile.join("config.json")).unwrap(),previous,"changed DB cannot commit config");
            fs::write(profile.join("mpd-v2.db"),b"new").unwrap();
            assert_eq!(proof.validate_current(&owner,&||true).unwrap_err(),recovery_fs::CANCELLED);
            let foreign=base.join("foreign");fs::DirBuilder::new().mode(0o700).create(&foreign).unwrap();
            let foreign_owner=Lease::acquire(&foreign).unwrap();foreign_owner.root().write_new("mpd-v2.db",b"new").unwrap();
            assert_eq!(proof.validate_current(&foreign_owner,&||false).unwrap_err(),CHANGED);
            if recovered {
                publication.rollback().unwrap();fs::write(profile.join("mpd-v2.db"),b"old").unwrap();
                assert!(proof.validate_current(&owner,&||false).is_err(),"rejected DB proof cannot validate restored DB");
                publication.cleanup_rejected_after_rollback().unwrap();
                let (_,uncertain)=intent.conclude_transaction(Err("validator refused new database".into()),Some(Completed::Recovered));assert!(!uncertain);
                finish_save(&owner,&||false,&validator).unwrap();assert_eq!(calls.get(),3,"recovery validates the old database");
            } else {
                crate::persistence::atomic_write_private_synced(&profile.join("config.json"),&serde_json::to_vec(&next).unwrap(),"config.json").unwrap();
                let reuse=|bytes:&[u8],cancel:&dyn Fn()->bool|proof.validate_bytes(&owner.root().id(),bytes,cancel);
                confirm_next(&owner,Some(&new_catalog),&||false,&reuse).unwrap();
                publication.cleanup_previous().unwrap();
                intent.complete_transaction(Ok(()),Some(Completed::Committed)).unwrap();
                finish_save(&owner,&||false,&reuse).unwrap();assert_eq!(calls.get(),2,"confirmed DB is not reopened by confirm/finish");
            }
            assert!(!profile.join(managed_save_intent::NAME).exists());assert!(!profile.join(PENDING).exists());assert!(profile.join(LAST).is_dir());
            eprintln!("retained precommit fixture: {}",base.display());
        }
    }

    #[test]
    fn precommit_database_failure_rolls_back_without_config_commit_or_uncertain_persistence() {
        let events=RefCell::new(Vec::new());
        let result=crate::complete_published_mirror_refresh(
            ||prepare_database_bytes(Identity {dev:1,ino:2},b"new db",None,&||false,
                &|_,_|Err("LIBRARY_RECOVERY_DATABASE_INVALID".into())).map(|_|()),
            ||{events.borrow_mut().push("capture");Ok(())},
            ||{events.borrow_mut().push("stop");Ok(())},
            ||{events.borrow_mut().push("rollback");Ok(())},
            |_|{events.borrow_mut().push("restore");Ok(crate::RestorePreviousOutcome::Restored)},
            ||{events.borrow_mut().push("cleanup_rejected");Ok(())},
            ||{events.borrow_mut().push("commit");Ok(())},
            ||{events.borrow_mut().push("cleanup_previous");Ok(())},
        ).unwrap_err();
        assert!(result.contains("LIBRARY_RECOVERY_DATABASE_INVALID"));
        assert!(!result.contains("CONFIG_PERSISTENCE_UNCERTAIN"));
        assert_eq!(*events.borrow(),["capture","stop","rollback","restore","cleanup_rejected"]);
    }
}
