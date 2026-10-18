//! Monotonic source-ID reservations. These are not a second config transaction:
//! an abandoned attempt may consume an ID, but rollback must never free one.
//!
//! `local-source-identities.json` and the namespace in its stable `.lock` file
//! belong to the private profile, outside library generations. Keep both when
//! backing up/restoring a profile; never restore an older ledger independently
//! of its references. v2 manifests bind this namespace, including when empty.
//! A missing, corrupt or unsupported ledger is preserved and blocks mutation.
//!
//! Bootstrap reserves only IDs visible in legacy/v1 mappings. Removed IDs from
//! before this feature cannot be reconstructed safely from playlists/favorites.
//! Roots are bound by filesystem ID, inode and creation time (device fallback
//! for an active filesystem without FSID). Reusing a retired ID requires FSID
//! and creation-time proof. Unsupported proof gives a fresh ID on readdition.
//! These are filesystem identities, not content hashes or authentication of
//! cloned filesystems, restored older histories, or live MPD file descriptors.
//! No media files, source xattrs, references or configuration are rewritten.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const NAME: &str = "local-source-identities.json";
const LOCK: &str = "local-source-identities.lock";
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const VERSION: u32 = 1;
const ERROR: &str = "LOCAL_SOURCE_IDENTITY_UNCONFIRMED";
fn error(detail: impl std::fmt::Display) -> String {
    format!("{ERROR}: {detail}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Origin {
    filesystem: Option<[i32; 2]>,
    device: u64,
    inode: u64,
    birth: Option<(u64, u32)>,
}
impl Origin {
    fn capture(root: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(root)
            .map_err(|_| error("não foi possível confirmar a origem da pasta"))?;
        let meta = file.metadata().map_err(error)?;
        let current = fs::symlink_metadata(root).map_err(error)?;
        if !meta.is_dir()
            || !current.is_dir()
            || meta.dev() != current.dev()
            || meta.ino() != current.ino()
        {
            return Err(error("a identidade da pasta mudou"));
        }
        let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
        let filesystem = if unsafe { libc::fstatfs(file.as_raw_fd(), info.as_mut_ptr()) } == 0 {
            let info = unsafe { info.assume_init() };
            // Linux fsid_t is two 32-bit words. It survives device renumbering;
            // zero/unsupported FSIDs do not authorize reactivation of retired IDs.
            let words: [i32; 2] = unsafe { std::mem::transmute(info.f_fsid) };
            (words != [0, 0]).then_some(words)
        } else {
            None
        };
        let birth = meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| (t.as_secs(), t.subsec_nanos()));
        Ok(Self {
            filesystem,
            device: meta.dev(),
            inode: meta.ino(),
            birth,
        })
    }
    fn matches_active(&self, other: &Self) -> bool {
        let same_fs = match (self.filesystem, other.filesystem) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.device == other.device,
            _ => false,
        };
        same_fs && self.inode == other.inode && self.birth == other.birth
    }
    fn proves_reactivation(&self, other: &Self) -> bool {
        self.filesystem.is_some() && self.birth.is_some() && self.matches_active(other)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Reservation {
    root: Option<PathBuf>,
    origin: Option<Origin>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    schema_version: u32,
    namespace: String,
    ids: BTreeMap<String, Reservation>,
    checksum: String,
}
impl Registry {
    fn checksum(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(&(self.schema_version, &self.namespace, &self.ids))
            .map_err(error)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != VERSION
            || !valid_namespace(&self.namespace)
            || self.checksum != self.checksum()?
            || self.ids.len() > 65_536
        {
            return Err(error(
                "registro de identidades inválido; arquivo preservado",
            ));
        }
        for (id, reservation) in &self.ids {
            if !valid_id(id)
                || reservation.origin.is_some() && reservation.root.is_none()
                || reservation.root.as_ref().is_some_and(|p| !p.is_absolute())
            {
                return Err(error("reserva de identidade inválida"));
            }
        }
        Ok(())
    }
}
pub(crate) fn valid_namespace(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_id(id: &str) -> bool {
    id.len() <= 255
        && crate::local_media::mirror::valid_source_id(id).is_some()
        && crate::local_media::is_safe_relative_mpd_uri(id)
}

struct Parent {
    requested: PathBuf,
    path: PathBuf,
    directory: File,
}
impl Parent {
    fn open(library: &Path) -> Result<Self, String> {
        let requested = library
            .parent()
            .ok_or_else(|| error("library sem diretório pai"))?
            .to_path_buf();
        let path = fs::canonicalize(&requested).map_err(error)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(error)?;
        let result = Self {
            requested,
            path,
            directory,
        };
        result.validate()?;
        Ok(result)
    }
    fn validate(&self) -> Result<(), String> {
        let opened = self.directory.metadata().map_err(error)?;
        let path = fs::metadata(&self.requested).map_err(error)?;
        let canonical = fs::symlink_metadata(&self.path).map_err(error)?;
        if !opened.is_dir()
            || !canonical.is_dir()
            || opened.uid() != unsafe { libc::geteuid() }
            || opened.mode() & 0o022 != 0
            || opened.dev() != path.dev()
            || opened.ino() != path.ino()
            || opened.dev() != canonical.dev()
            || opened.ino() != canonical.ino()
        {
            return Err(error("diretório de identidades substituído ou inseguro"));
        }
        Ok(())
    }
    fn entry(&self, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd())).join(name)
    }
    fn open_lock(&self) -> Result<File, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.entry(LOCK))
            .map_err(error)?;
        validate_file(&file, &self.entry(LOCK))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(error("registro de identidades ocupado; tente novamente"));
        }
        self.validate()?;
        Ok(file)
    }
    fn read(&self) -> Result<Option<Registry>, String> {
        self.validate()?;
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.entry(NAME))
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(error(e)),
        };
        validate_file(&file, &self.entry(NAME))?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(error("registro de identidades excede o limite"));
        }
        let registry: Registry = serde_json::from_slice(&bytes)
            .map_err(|_| error("registro de identidades corrompido; arquivo preservado"))?;
        registry.validate()?;
        self.validate()?;
        Ok(Some(registry))
    }
    fn write(&self, registry: &Registry) -> Result<(), String> {
        self.validate()?;
        registry.validate()?;
        let bytes = serde_json::to_vec_pretty(registry).map_err(error)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(error("registro de identidades excede o limite"));
        }
        crate::persistence::atomic_write_private_synced_in(&self.path.join(NAME), &self.directory, &bytes, NAME)
            .map_err(|_| error("reserva de identidades não confirmada no disco; tente novamente após verificar o registro"))?;
        self.validate()
    }
}
fn validate_file(file: &File, path: &Path) -> Result<(), String> {
    let meta = file.metadata().map_err(error)?;
    let current = fs::symlink_metadata(path).map_err(error)?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || !current.is_file()
        || meta.dev() != current.dev()
        || meta.ino() != current.ino()
    {
        return Err(error("arquivo de identidades inseguro ou substituído"));
    }
    Ok(())
}

/// Read-only v2 manifest validation. History is outside generations, so cleanup
/// or rollback cannot discard reservations. Older v1 manifests remain readable.
pub(crate) fn check_manifest(
    library: &Path,
    namespace: &str,
    sources: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let parent = Parent::open(library)?;
    let registry = parent
        .read()?
        .ok_or_else(|| error("registro obrigatório ausente; não será recriado silenciosamente"))?;
    if registry.namespace != namespace
        || sources.iter().any(|(id, root)| {
            registry
                .ids
                .get(id)
                .is_none_or(|r| r.root.as_ref() != Some(root) || r.origin.is_none())
        })
    {
        return Err(error("manifesto diverge do registro de identidades"));
    }
    Ok(())
}

pub(crate) fn reserve(
    library: &Path,
    roots: &[PathBuf],
    current: &BTreeMap<String, Option<PathBuf>>,
    expected_namespace: Option<&str>,
) -> Result<(BTreeMap<String, PathBuf>, String), String> {
    reserve_with(
        library,
        roots,
        current,
        expected_namespace,
        |parent, registry| parent.write(registry),
    )
}
fn reserve_with(
    library: &Path,
    roots: &[PathBuf],
    current: &BTreeMap<String, Option<PathBuf>>,
    expected_namespace: Option<&str>,
    write: impl FnOnce(&Parent, &Registry) -> Result<(), String>,
) -> Result<(BTreeMap<String, PathBuf>, String), String> {
    let parent = Parent::open(library)?;
    // Same private-directory policy used by the existing persistence writer.
    // Read-only manifest inspection never changes permissions.
    parent
        .directory
        .set_permissions(fs::Permissions::from_mode(0o700))
        .map_err(error)?;
    let mut lock = parent.open_lock()?;
    let mut anchor = String::new();
    (&mut lock)
        .take(34)
        .read_to_string(&mut anchor)
        .map_err(error)?;
    let loaded = parent.read()?;
    if !anchor.is_empty()
        && (!valid_namespace(&anchor) || loaded.as_ref().is_none_or(|r| r.namespace != anchor))
    {
        return Err(error(
            "histórico obrigatório ausente ou divergente; não será reiniciado",
        ));
    }
    if expected_namespace.is_some_and(|n| loaded.as_ref().is_none_or(|r| r.namespace != n)) {
        return Err(error(
            "histórico de identidades obrigatório ausente ou divergente",
        ));
    }
    let original = loaded.as_ref().map(|r| r.checksum.clone());
    let mut registry = match loaded {
        Some(r) => r,
        None => {
            let mut random = [0u8; 16];
            File::open("/dev/urandom")
                .and_then(|mut f| f.read_exact(&mut random))
                .map_err(error)?;
            Registry {
                schema_version: VERSION,
                namespace: random.iter().map(|b| format!("{b:02x}")).collect(),
                ids: BTreeMap::new(),
                checksum: String::new(),
            }
        }
    };
    // Bootstrap only identities currently observable in a v1/legacy generation.
    // No inference from playlists/favorites or irrecoverable earlier removals.
    for (id, root) in current {
        if !valid_id(id) {
            return Err(error("identificador existente inválido"));
        }
        if let Some(known) = registry.ids.get(id) {
            if root
                .as_ref()
                .is_some_and(|p| known.root.as_ref() != Some(p))
            {
                return Err(error("identificador existente pertence a outra origem"));
            }
        } else {
            let origin = root.as_ref().and_then(|p| Origin::capture(p).ok());
            registry.ids.insert(
                id.clone(),
                Reservation {
                    root: root.clone(),
                    origin,
                },
            );
        }
    }
    let mut result = BTreeMap::new();
    for root in roots {
        let observed = Origin::capture(root)?;
        let active = current
            .iter()
            .filter(|(_, p)| p.as_ref() == Some(root))
            .collect::<Vec<_>>();
        if active.len() > 1 {
            return Err(error("origem ativa possui identificadores ambíguos"));
        }
        let id = if let Some((id, _)) = active.first() {
            let known = registry
                .ids
                .get(*id)
                .ok_or_else(|| error("reserva ativa ausente"))?;
            if !known
                .origin
                .as_ref()
                .is_some_and(|o| o.matches_active(&observed))
            {
                return Err(error(
                    "a origem ativa foi substituída; não será reatribuída automaticamente",
                ));
            }
            (*id).clone()
        } else {
            let candidates = registry
                .ids
                .iter()
                .filter(|(_, r)| {
                    r.root.as_ref() == Some(root)
                        && r.origin
                            .as_ref()
                            .is_some_and(|o| o.proves_reactivation(&observed))
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if candidates.len() == 1 {
                candidates[0].clone()
            } else {
                let name = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Musicas");
                let base = if valid_id(name) || name == crate::local_media::mirror::MANIFEST_NAME {
                    name
                } else {
                    "Musicas"
                };
                let mut suffix = 1u64;
                let id = loop {
                    let tail = if suffix == 1 {
                        String::new()
                    } else {
                        format!(" ({suffix})")
                    };
                    let mut end = base.len().min(255 - tail.len());
                    while !base.is_char_boundary(end) {
                        end -= 1;
                    }
                    let candidate = format!("{}{}", &base[..end], tail);
                    if valid_id(&candidate) && !registry.ids.contains_key(&candidate) {
                        break candidate;
                    }
                    suffix = suffix
                        .checked_add(1)
                        .ok_or_else(|| error("namespace de identidades esgotado"))?;
                };
                registry.ids.insert(
                    id.clone(),
                    Reservation {
                        root: Some(root.clone()),
                        origin: Some(observed),
                    },
                );
                id
            }
        };
        result.insert(id, root.clone());
    }
    registry.checksum = registry.checksum()?;
    registry.validate()?;
    validate_file(&lock, &parent.entry(LOCK))?;
    if original.as_deref() != Some(registry.checksum.as_str()) {
        write(&parent, &registry)?;
    }
    // Persist the namespace in the stable lock inode too, so deleting only
    // the ledger cannot reset an empty legacy library on the next startup.
    if anchor.is_empty() {
        lock.rewind().map_err(error)?;
        lock.write_all(registry.namespace.as_bytes())
            .map_err(error)?;
        lock.sync_all().map_err(error)?;
        parent.directory.sync_all().map_err(error)?;
    }
    parent.validate()?;
    Ok((result, registry.namespace))
}

/// Used only at mutation/publication gates that already require online roots.
/// Reading a manifest does not introduce a new policy for disconnected sources.
pub(crate) fn verify_origins(
    library: &Path,
    sources: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let parent = Parent::open(library)?;
    let registry = parent
        .read()?
        .ok_or_else(|| error("registro de identidades ausente"))?;
    verify_registered_origins(&registry, sources)
}

/// Legacy manifests are readable before their first reservation. Once a
/// history exists, resolution must not rebind an old URI to a recreated root.
pub(crate) fn verify_if_initialized(
    library: &Path,
    sources: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let parent = Parent::open(library)?;
    if let Some(registry) = parent.read()? {
        return verify_registered_origins(&registry, sources);
    }
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(parent.entry(LOCK))
    {
        Ok(file) => {
            validate_file(&file, &parent.entry(LOCK))?;
            if file.metadata().map_err(error)?.len() != 0 {
                return Err(error("histórico obrigatório ausente"));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(error(e)),
    }
    Ok(())
}
fn verify_registered_origins(
    registry: &Registry,
    sources: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    for (id, root) in sources {
        let observed = Origin::capture(root)?;
        if !registry.ids.get(id).is_some_and(|r| {
            r.root.as_ref() == Some(root)
                && r.origin
                    .as_ref()
                    .is_some_and(|o| o.matches_active(&observed))
        }) {
            return Err(error("origem substituída desde a reserva de identidade"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_media::mirror;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sonante-source-identity-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn library(&self) -> PathBuf {
            self.0.join("library")
        }
        fn root(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.join("Album")).unwrap();
            fs::write(path.join("Album/track.wav"), relative.as_bytes()).unwrap();
            path
        }
        fn reserve(
            &self,
            roots: &[PathBuf],
            current: &BTreeMap<String, Option<PathBuf>>,
        ) -> (BTreeMap<String, PathBuf>, String) {
            reserve(&self.library(), roots, current, None).unwrap()
        }
        fn prepare(&self, roots: &[PathBuf], id: &str) -> mirror::MirrorBuildResult {
            mirror::build_filtered_staging(&self.0.join("library.next"), &self.library(), roots, id)
                .unwrap()
        }
        fn seed(&self, roots: &[PathBuf]) {
            let p = self.prepare(roots, "seed");
            mirror::publish_prepared_managed_generation(&self.library(), &p)
                .unwrap()
                .cleanup_previous()
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn current(map: &BTreeMap<String, PathBuf>) -> BTreeMap<String, Option<PathBuf>> {
        map.iter()
            .map(|(id, r)| (id.clone(), Some(r.clone())))
            .collect()
    }

    #[test]
    fn retired_ids_survive_empty_generation_restart_and_cleanup_without_rebinding_references() {
        use crate::{
            audio::MediaLocator,
            playlists::{NewPlaylistItem, PlaylistItemMetadata, PlaylistStore},
        };
        let f = Fixture::new();
        let old = f.root("old/Music");
        let new = f.root("new/Music");
        f.seed(&[old.clone()]);
        let uri = "Music/Album/track.wav";
        let store = PlaylistStore::new(f.0.join("playlists.json"));
        let playlist = store
            .create_with_items(
                "Old source",
                vec![NewPlaylistItem {
                    media_locator: MediaLocator::Local { uri: uri.into() },
                    metadata: PlaylistItemMetadata {
                        title: "Old track".into(),
                        artist: "Fixture".into(),
                        album: "Album".into(),
                        duration: None,
                    },
                }],
            )
            .unwrap();
        // Existing persisted references are inputs, never an inferred history.
        for name in ["favorites.json", "queue_cache.json", "config.json"] {
            fs::write(f.0.join(name), format!("{{\"reference\":\"{uri}\"}}")).unwrap();
        }
        let snapshots = [
            "playlists.json",
            "favorites.json",
            "queue_cache.json",
            "config.json",
        ]
        .map(|name| (name, fs::read(f.0.join(name)).unwrap()));
        let removal = f.prepare(&[], "removed-all");
        let plan = mirror::plan_removed_sources_refresh(&f.library(), &removal)
            .unwrap()
            .unwrap();
        mirror::publish_targeted_removed_sources_generation_guarded(
            &f.library(),
            &removal,
            &plan,
            || Ok(()),
        )
        .unwrap()
        .cleanup_previous()
        .unwrap();
        assert!(mirror::read_manifest(&f.library())
            .unwrap()
            .sources
            .is_empty());
        // No in-memory state survives this boundary: every call reloads disk.
        let addition = f.prepare(&[new.clone()], "after-restart");
        assert_eq!(addition.manifest.sources.get("Music (2)"), Some(&new));
        let plan = mirror::plan_added_sources_refresh(&f.library(), &addition)
            .unwrap()
            .unwrap();
        mirror::publish_targeted_added_sources_generation(&f.library(), &addition, &plan)
            .unwrap()
            .cleanup_previous()
            .unwrap();
        let folders = [new.to_string_lossy().into_owned()];
        assert!(crate::local_media::resolve_local_path(&f.library(), uri, &folders).is_err());
        assert_eq!(
            crate::local_media::resolve_local_path(
                &f.library(),
                "Music (2)/Album/track.wav",
                &folders
            )
            .unwrap()
            .physical_path,
            new.join("Album/track.wav")
        );
        assert_eq!(store.get(&playlist.id).unwrap(), playlist);
        for (name, bytes) in snapshots {
            assert_eq!(fs::read(f.0.join(name)).unwrap(), bytes, "{name}");
        }
        let registry = Parent::open(&f.library()).unwrap().read().unwrap().unwrap();
        assert_eq!(registry.ids.len(), 2);
    }

    #[test]
    fn surviving_ids_and_canonical_aliases_are_preserved_under_reordering() {
        let f = Fixture::new();
        let a = f.root("one/Music");
        let b = f.root("two/Music");
        let alias = f.0.join("alias");
        symlink(&a, &alias).unwrap();
        f.seed(&[a.clone(), b.clone()]);
        let before = mirror::read_manifest(&f.library()).unwrap();
        assert_eq!(
            mirror::prepare_managed_generation(
                &f.library(),
                &f.0.join("library.next"),
                &[b, a, alias],
                "reordered"
            )
            .unwrap(),
            mirror::ManagedGenerationPreparation::Unchanged
        );
        assert_eq!(mirror::read_manifest(&f.library()).unwrap(), before);
    }

    #[test]
    fn same_origin_readdition_requires_strong_filesystem_inode_and_birth_proof() {
        let f = Fixture::new();
        let root = f.root("Music");
        let (first, _) = f.reserve(&[root.clone()], &BTreeMap::new());
        f.reserve(&[], &current(&first));
        let (again, _) = f.reserve(&[root.clone()], &BTreeMap::new());
        let origin = Origin::capture(&root).unwrap();
        assert_eq!(
            again.keys().next() == first.keys().next(),
            origin.proves_reactivation(&origin)
        );
        let weak = Origin {
            filesystem: None,
            device: 1,
            inode: 2,
            birth: None,
        };
        assert!(weak.matches_active(&weak));
        assert!(!weak.proves_reactivation(&weak));
        let mut replaced = origin.clone();
        replaced.birth = origin.birth.map(|(s, n)| (s + 1, n));
        assert!(!origin.proves_reactivation(&replaced));
    }

    #[test]
    fn identical_path_recreated_directory_cannot_inherit_active_or_retired_id() {
        let f = Fixture::new();
        let root = f.root("Music");
        let (first, _) = f.reserve(&[root.clone()], &BTreeMap::new());
        fs::rename(&root, f.0.join("old-directory")).unwrap();
        f.root("Music");
        let before = fs::read(f.0.join(NAME)).unwrap();
        assert!(
            reserve(&f.library(), &[root.clone()], &current(&first), None)
                .unwrap_err()
                .contains("substituída")
        );
        assert_eq!(fs::read(f.0.join(NAME)).unwrap(), before);
        f.reserve(&[], &current(&first));
        let (next, _) = f.reserve(&[root.clone()], &BTreeMap::new());
        assert_eq!(next.keys().next().unwrap(), "Music (2)");
    }

    #[test]
    fn changed_root_between_preparation_and_exchange_is_rejected_without_publication() {
        let f = Fixture::new();
        let root = f.root("Music");
        f.seed(&[]);
        let p = f.prepare(&[root.clone()], "next");
        let result = mirror::publish_prepared_managed_generation_guarded(&f.library(), &p, || {
            fs::rename(&root, f.0.join("original")).unwrap();
            f.root("Music");
            Ok(())
        });
        assert!(result.is_err());
        assert!(mirror::read_manifest(&f.library())
            .unwrap()
            .sources
            .is_empty());
        assert!(f.0.join("library.next").is_dir());
    }

    #[test]
    fn v1_and_legacy_bootstrap_reserve_removed_names_before_allocating_new_ones() {
        for managed in [false, true] {
            let f = Fixture::new();
            let old = f.root("old/Music");
            let new = f.root("new/Music");
            fs::create_dir(f.library()).unwrap();
            if managed {
                fs::create_dir_all(f.library().join("Music/Album")).unwrap();
                symlink(
                    old.join("Album/track.wav"),
                    f.library().join("Music/Album/track.wav"),
                )
                .unwrap();
                mirror::write_manifest(
                    &f.library(),
                    &mirror::MirrorManifest {
                        schema_version: 1,
                        generation_id: "legacy-v1".into(),
                        registry_id: None,
                        sources: BTreeMap::from([("Music".into(), old.clone())]),
                    },
                )
                .unwrap();
            } else {
                symlink(&old, f.library().join("Music")).unwrap();
            }
            let p = f.prepare(&[new.clone()], "migration");
            assert_eq!(p.manifest.schema_version, 2);
            assert_eq!(p.manifest.sources.get("Music (2)"), Some(&new));
            if managed {
                assert!(mirror::read_manifest(&f.library()).is_ok());
            }
            mirror::publish_prepared_managed_generation(&f.library(), &p)
                .unwrap()
                .cleanup_previous()
                .unwrap();
            assert!(crate::local_media::resolve_local_path(
                &f.library(),
                "Music/Album/track.wav",
                &[new.to_string_lossy().into_owned()]
            )
            .is_err());
        }
    }

    #[test]
    fn unchanged_v1_is_upgraded_through_normal_preparation_without_renaming_sources() {
        let f = Fixture::new();
        let root = f.root("Music");
        fs::create_dir(f.library()).unwrap();
        fs::create_dir_all(f.library().join("My existing ID/Album")).unwrap();
        symlink(
            root.join("Album/track.wav"),
            f.library().join("My existing ID/Album/track.wav"),
        )
        .unwrap();
        let v1 = mirror::MirrorManifest {
            schema_version: 1,
            generation_id: "old".into(),
            registry_id: None,
            sources: BTreeMap::from([("My existing ID".into(), root.clone())]),
        };
        mirror::write_manifest(&f.library(), &v1).unwrap();
        let mirror::ManagedGenerationPreparation::Prepared(p) = mirror::prepare_managed_generation(
            &f.library(),
            &f.0.join("library.next"),
            &[root],
            "upgrade",
        )
        .unwrap() else {
            panic!("namespace marker must be published")
        };
        assert_eq!(p.manifest.sources, v1.sources);
        assert!(p.manifest.registry_id.is_some());
    }

    #[test]
    fn failed_reservation_never_returns_an_id_and_post_rename_uncertainty_burns_it() {
        for after_write in [false, true] {
            let f = Fixture::new();
            let a = f.root("a/Music");
            let b = f.root("b/Music");
            let failure = reserve_with(&f.library(), &[a], &BTreeMap::new(), None, |parent, r| {
                if after_write {
                    parent.write(r)?;
                }
                Err(error("injected durability failure"))
            });
            assert!(failure.is_err());
            assert!(!f.library().exists());
            let (ids, _) = f.reserve(&[b], &BTreeMap::new());
            assert_eq!(
                ids.keys().next().unwrap(),
                if after_write { "Music (2)" } else { "Music" }
            );
        }
    }

    #[test]
    fn corrupt_truncated_wrong_version_or_checksum_registry_is_preserved_and_blocks_preparation() {
        for mutate in 0..5 {
            let f = Fixture::new();
            let root = f.root("Music");
            f.seed(&[root.clone()]);
            let original = fs::read(f.0.join(NAME)).unwrap();
            let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
            let bytes = match mutate {
                0 => b"{".to_vec(),
                1 => {
                    value["schema_version"] = 2.into();
                    serde_json::to_vec(&value).unwrap()
                }
                2 => {
                    value["ids"]["Music"]["root"] = "/different".into();
                    serde_json::to_vec(&value).unwrap()
                }
                3 => {
                    value["unexpected"] = true.into();
                    serde_json::to_vec(&value).unwrap()
                }
                _ => vec![],
            };
            fs::write(f.0.join(NAME), &bytes).unwrap();
            assert!(mirror::prepare_managed_generation(
                &f.library(),
                &f.0.join("library.next"),
                &[root],
                "blocked"
            )
            .is_err());
            assert_eq!(fs::read(f.0.join(NAME)).unwrap(), bytes);
            assert!(!f.0.join("library.next").exists());
        }
    }

    #[test]
    fn missing_history_never_reinitializes_a_v2_or_empty_legacy_namespace() {
        for managed in [false, true] {
            let f = Fixture::new();
            let a = f.root("a/Music");
            let b = f.root("b/Music");
            if managed {
                f.seed(&[a]);
                let p = f.prepare(&[], "empty");
                mirror::publish_prepared_managed_generation(&f.library(), &p)
                    .unwrap()
                    .cleanup_previous()
                    .unwrap();
            } else {
                f.reserve(&[a], &BTreeMap::new());
            }
            fs::remove_file(f.0.join(NAME)).unwrap();
            assert!(reserve(&f.library(), &[b], &BTreeMap::new(), None).is_err());
            assert!(!f.0.join(NAME).exists());
            if managed {
                assert!(mirror::read_manifest(&f.library()).is_err());
            }
        }
    }

    #[test]
    fn unsafe_files_and_concurrent_writer_fail_without_overwriting_history() {
        for case in 0..4 {
            let f = Fixture::new();
            let a = f.root("Music");
            f.reserve(&[a.clone()], &BTreeMap::new());
            let saved = fs::read(f.0.join(NAME)).unwrap();
            match case {
                0 => {
                    fs::rename(f.0.join(NAME), f.0.join("outside")).unwrap();
                    symlink(f.0.join("outside"), f.0.join(NAME)).unwrap();
                }
                1 => {
                    fs::hard_link(f.0.join(NAME), f.0.join("hardlink")).unwrap();
                }
                2 => {
                    fs::set_permissions(f.0.join(NAME), fs::Permissions::from_mode(0o644)).unwrap()
                }
                _ => {
                    fs::remove_file(f.0.join(LOCK)).unwrap();
                    symlink(f.0.join(NAME), f.0.join(LOCK)).unwrap();
                }
            }
            assert!(reserve(&f.library(), &[a], &BTreeMap::new(), None).is_err());
            assert_eq!(fs::read(f.0.join(NAME)).unwrap(), saved);
        }
        let f = Fixture::new();
        let root = f.root("Music");
        let parent = Parent::open(&f.library()).unwrap();
        let held = parent.open_lock().unwrap();
        assert!(
            reserve(&f.library(), &[root.clone()], &BTreeMap::new(), None)
                .unwrap_err()
                .contains("ocupado")
        );
        drop(held);
        f.reserve(&[root], &BTreeMap::new());
    }

    #[test]
    fn unicode_collision_suffix_stays_within_single_component_limit() {
        let f = Fixture::new();
        let name = "音".repeat(85);
        let a = f.root(&format!("a/{name}"));
        let b = f.root(&format!("b/{name}"));
        let (first, _) = f.reserve(&[a], &BTreeMap::new());
        let (next, _) = f.reserve(&[b], &BTreeMap::new());
        let id = next.keys().next().unwrap();
        assert_ne!(first.keys().next().unwrap(), id);
        assert!(valid_id(id));
        assert!(id.len() <= 255);
    }
    #[test]
    fn resolver_refuses_recreated_root_before_any_new_generation_or_reallocation() {
        let f = Fixture::new();
        let root = f.root("Music");
        f.seed(&[root.clone()]);
        let folders = [root.to_string_lossy().into_owned()];
        assert!(crate::local_media::resolve_local_path(
            &f.library(),
            "Music/Album/track.wav",
            &folders
        )
        .is_ok());
        fs::rename(&root, f.0.join("old-root")).unwrap();
        f.root("Music");
        assert!(crate::local_media::resolve_local_path(
            &f.library(),
            "Music/Album/track.wav",
            &folders
        )
        .is_err());
        // Explicit retirement can still be prepared; it does not adopt the new directory.
        let removed = f.prepare(&[], "retire-replaced-root");
        assert!(removed.manifest.sources.is_empty());
    }
    #[test]
    fn restart_child() {
        let Ok(root) = std::env::var("SONANTE_SOURCE_IDENTITY_TEST_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("sonante-source-identity-"));
        let stage = std::env::var("SONANTE_SOURCE_IDENTITY_TEST_STAGE").unwrap();
        let source = root.join(if stage == "one" {
            "one/Music"
        } else {
            "two/Music"
        });
        let (ids, namespace) =
            reserve(&root.join("library"), &[source], &BTreeMap::new(), None).unwrap();
        if stage == "one" {
            assert_eq!(ids.keys().next().unwrap(), "Music");
            reserve(&root.join("library"), &[], &current(&ids), Some(&namespace)).unwrap();
        } else {
            assert_eq!(ids.keys().next().unwrap(), "Music (2)");
        }
    }

    #[test]
    fn reservations_are_reloaded_in_a_new_process_after_empty_retirement() {
        let f = Fixture::new();
        f.root("one/Music");
        f.root("two/Music");
        for stage in ["one", "two"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "source_identity::tests::restart_child",
                    "--test-threads=1",
                ])
                .env("SONANTE_SOURCE_IDENTITY_TEST_ROOT", &f.0)
                .env("SONANTE_SOURCE_IDENTITY_TEST_STAGE", stage)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        assert_eq!(
            Parent::open(&f.library())
                .unwrap()
                .read()
                .unwrap()
                .unwrap()
                .ids
                .len(),
            2
        );
    }

    #[test]
    fn namespace_mismatch_and_oversized_registry_do_not_reset_or_publish() {
        let f = Fixture::new();
        let root = f.root("Music");
        f.seed(&[root.clone()]);
        let parent = Parent::open(&f.library()).unwrap();
        let mut registry = parent.read().unwrap().unwrap();
        registry.namespace = "abcdef0123456789abcdef0123456789".into();
        registry.checksum = registry.checksum().unwrap();
        parent.write(&registry).unwrap();
        assert!(mirror::read_manifest(&f.library()).is_err());
        assert!(reserve(&f.library(), &[root.clone()], &BTreeMap::new(), None).is_err());
        assert!(!f.0.join("library.next").exists());
        OpenOptions::new()
            .write(true)
            .open(f.0.join(NAME))
            .unwrap()
            .set_len(MAX_BYTES + 1)
            .unwrap();
        assert!(reserve(&f.library(), &[root], &BTreeMap::new(), None).is_err());
        assert_eq!(fs::metadata(f.0.join(NAME)).unwrap().len(), MAX_BYTES + 1);
    }
}
