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
pub(crate) fn execute(
    lease: &Lease,
    id: &str,
    runtime: &Path,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
) -> Result<ResultDto, String> {
    let (_, set) = open_set(lease.root(), cancel)?;
    execute_checked(lease, id, cancel, validate, hook, &|| {
        crate::profile_lease::legacy_fence(
            lease.root(),
            &set.scope,
            set.creator,
            &set.start,
            runtime,
        )
    })
}
fn execute_checked(
    lease: &Lease,
    id: &str,
    cancel: &dyn Fn() -> bool,
    validate: &dyn Fn(&[u8], &dyn Fn() -> bool) -> Result<HashSet<String>, String>,
    hook: &dyn Fn(&str) -> Result<(), String>,
    ownership: &dyn Fn() -> Result<(), String>,
) -> Result<ResultDto, String> {
    lease.validate()?;
    let root = lease.root();
    let (dir, set) = open_set(root, cancel)?;
    ownership()?;
    recovery_fs::check(cancel)?;
    let plan = analyze(root, cancel)?;
    if plan.id != id {
        return Err(CHANGED.into());
    }
    compatible_ledger(root, &dir, &set, cancel)?;
    let journal = if dir.exists("execution.json")? {
        let j: Journal = decode(
            &dir.read("execution.json", 64 * 1024 * 1024, cancel)?
                .ok_or(INCOMPLETE)?,
        )?;
        if j.version != 1 || j.id != id {
            return Err(INCOMPLETE.into());
        }
        read_snapshot(&dir.child("blocked")?, cancel)?;
        j
    } else {
        let (current, raw) = snapshot(root, cancel)?;
        if dir.exists("blocked")? {
            if read_snapshot(&dir.child("blocked")?, cancel)? != current {
                return Err(CHANGED.into());
            }
        } else {
            let blocked = dir.create("blocked")?;
            write_snapshot(&blocked, &current, &raw, cancel, u64::MAX, &|_, _, _| {})?;
        }
        // This durable backup is the blocked state, distinct from the pre-Save set.
        verify_files(root, &current, cancel)?;
        hook("backup_complete")?;
        recovery_fs::check(cancel)?;
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
        dir.write_new("execution.json", &encode(&j)?)?;
        stage(&dir, "backup-complete.json")?;
        j
    };
    let fence = || -> Result<(), String> {
        recovery_fs::check(cancel)?;
        lease.validate()?;
        compatible_ledger(root, &dir, &set, cancel)?;
        verify_intermediate(root, &dir, &journal, cancel)
    };
    fence()?;
    origins(root, &journal.final_live, cancel)?;
    if dir.exists("committed.json")? {
        if Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live)
            || root.exists("library.next")?
        {
            return Err(CHANGED.into());
        }
        for (name, p) in &journal.final_files {
            if name == managed_save_intent::NAME || name == SEAL {
                continue;
            }
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
    let candidate = if journal.choice == Choice::RestorePrevious {
        file_bytes(&dir, "mpd-v2.db", cancel)?
    } else {
        root.read("mpd-v2.db", LIMIT, cancel)?.ok_or(INCOMPLETE)?
    };
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
    if validate(&candidate, cancel)? != expected {
        return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into());
    }
    fence()?;
    stage(&dir, "applying.json")?;
    hook("applying")?;
    if Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live) {
        fence()?;
        if Tree::capture(root, "library.next", cancel)?.as_ref() != Some(&journal.final_live) {
            return Err(CHANGED.into());
        }
        crate::local_media::mirror::atomic_exchange(
            &root.entry("library")?,
            &root.entry("library.next")?,
        )?;
        root.sync()?;
        hook("exchanged")?;
    }
    fence()?;
    if root.exists("library.next")? {
        if Tree::capture(root, "library.next", cancel)?.as_ref() != expected_next(&journal) {
            return Err(CHANGED.into());
        }
        root.rename("library.next", &dir, "retained-next", true)?;
        hook("staging_retained")?;
    } else if let Some(expected) = expected_next(&journal) {
        if Tree::capture(&dir, "retained-next", cancel)?.as_ref() != Some(expected) {
            return Err(CHANGED.into());
        }
    }
    for name in ["config.json", "mpd-v2.db", "queue_cache.json"] {
        fence()?;
        let current = root.read(
            name,
            if name == "mpd-v2.db" {
                LIMIT
            } else {
                16 * 1024 * 1024
            },
            cancel,
        )?;
        if proof(current.as_deref()).as_ref()
            != journal.final_files.get(name).ok_or(INCOMPLETE)?.as_ref()
        {
            if journal.choice != Choice::RestorePrevious {
                return Err(CHANGED.into());
            }
            let bytes = if name == "queue_cache.json" {
                set.binding.queue_cache.clone().ok_or(INCOMPLETE)?
            } else {
                file_bytes(&dir, name, cancel)?
            };
            if proof(Some(&bytes)).as_ref()
                != journal.final_files.get(name).ok_or(INCOMPLETE)?.as_ref()
            {
                return Err(INCOMPLETE.into());
            }
            let temporary = format!("{name}.writing");
            if !root.exists(&temporary)? {
                root.write_new(&temporary, &bytes)?;
            }
            if root.read(&temporary, LIMIT, cancel)?.as_deref() != Some(bytes.as_slice()) {
                return Err(CHANGED.into());
            }
            let identity = root.file_identity(&temporary)?.ok_or(INCOMPLETE)?;
            let effect = format!("effect-{name}.json");
            if dir.exists(&effect)? {
                let expected: Identity =
                    decode(&dir.read(&effect, 4096, cancel)?.ok_or(INCOMPLETE)?)?;
                if expected != identity {
                    return Err(CHANGED.into());
                }
            } else {
                dir.write_new(&effect, &encode(&identity)?)?;
            }
            fence()?;
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
            hook(name)?;
        }
        let temporary = format!("{name}.writing");
        if root.exists(&temporary)? {
            fence()?;
            let expected = journal
                .before
                .identities
                .get(name)
                .ok_or(INCOMPLETE)?
                .as_ref()
                .ok_or(INCOMPLETE)?;
            if root.file_identity(&temporary)?.as_ref() != Some(expected) {
                return Err(CHANGED.into());
            }
            root.rename(&temporary, &dir, &format!("retained-control-{name}"), true)?;
        }
    }
    fence()?;
    stage(&dir, "validating.json")?;
    hook("validating")?;
    let db = root.read("mpd-v2.db", LIMIT, cancel)?.ok_or(INCOMPLETE)?;
    if validate(&db, cancel)? != expected {
        return Err("LIBRARY_RECOVERY_DATABASE_CATALOG_MISMATCH".into());
    }
    origins(root, &journal.final_live, cancel)?;
    fence()?;
    for (name, p) in &journal.final_files {
        if name == managed_save_intent::NAME || name == SEAL {
            continue;
        }
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
    if Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live)
        || root.exists("library.next")?
    {
        return Err(CHANGED.into());
    }
    root.sync()?;
    stage(&dir, "committed.json")?;
    hook("committed")?;
    fence()?;
    if let Some(intent) = root.read(managed_save_intent::NAME, 262144, cancel)? {
        if recovery_fs::hash(&intent) != set.binding.record_hash {
            return Err(CHANGED.into());
        }
        root.rename(managed_save_intent::NAME, &dir, "retained-intent", true)?;
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
    fence()?;
    if root.exists(SEAL)? {
        root.rename(SEAL, &dir, "retained-seal", true)?;
    } else if !dir.exists("retained-seal")? {
        return Err(INCOMPLETE.into());
    }
    fence()?;
    root.sync()?;
    stage(&dir, "finalized.json")?;
    hook("finalized")?;
    retire(lease, &dir, cancel, &|| {
        ownership()?;
        fence()?;
        for (name, expected) in &journal.final_files {
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
        if root.exists("library.next")?
            || Tree::capture(root, "library", cancel)?.as_ref() != Some(&journal.final_live)
        {
            return Err(CHANGED.into());
        }
        Ok(())
    })?;
    Ok(ResultDto {
        completed: true,
        backup_retained: true,
        requires_reopen: true,
    })
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
    execute_checked(lease, id, cancel, validate, hook, &|| Ok(()))
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
