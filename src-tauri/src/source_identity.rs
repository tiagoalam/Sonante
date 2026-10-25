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
//! Active IDs follow the configured canonical root and inode; mount device
//! numbers and missing optional metadata do not prove replacement. Available
//! conflicting FSID/birth observations still refuse automatic continuity.
//! Retired IDs require the prior FSID and creation-time proof for reactivation.
//! Unsupported proof gives a fresh ID on readdition. These are filesystem
//! observations, not content hashes or authentication of
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
    // Callers already require an active ID mapped to this canonical root.
    // Device is an auxiliary mount observation, not the configured source ID.
    fn matches_active(&self, other: &Self) -> bool {
        self.inode == other.inode
            && !matches!((self.filesystem, other.filesystem), (Some(a), Some(b)) if a != b)
            && !matches!((self.birth, other.birth), (Some(a), Some(b)) if a != b)
    }
    // Preserve the prior comparison for retired IDs and historical recovery.
    fn matches_reserved_origin(&self, other: &Self) -> bool {
        let same_fs = match (self.filesystem, other.filesystem) {
            (Some(a), Some(b)) => a == b,
            (None, None) => self.device == other.device,
            _ => false,
        };
        same_fs && self.inode == other.inode && self.birth == other.birth
    }
    fn proves_reactivation(&self, other: &Self) -> bool {
        self.filesystem.is_some() && self.birth.is_some() && self.matches_reserved_origin(other)
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

/// Structural validation only: no Parent::open, hardening, locks or physical
/// Origin::capture. The caller supplies bounded bytes from anchored readonly fds.
pub(crate) fn validate_recovery_extension(old:&[u8],anchor:&[u8],current:&[u8],current_anchor:&[u8])->Result<(),String>{
    let a:Registry=serde_json::from_slice(old).map_err(|_|"LIBRARY_RECOVERY_LEDGER_DIVERGENT")?;
    let b:Registry=serde_json::from_slice(current).map_err(|_|"LIBRARY_RECOVERY_LEDGER_DIVERGENT")?;
    a.validate()?;b.validate()?;
    if a.namespace!=b.namespace||anchor!=a.namespace.as_bytes()||current_anchor!=anchor
        || a.ids.iter().any(|(id,reservation)|b.ids.get(id)!=Some(reservation)) {
        return Err("LIBRARY_RECOVERY_LEDGER_DIVERGENT".into());
    }
    Ok(())
}
pub(crate) fn verify_recovery_origins(bytes:&[u8],sources:&BTreeMap<String,PathBuf>)->Result<(),String>{
    let registry:Registry=serde_json::from_slice(bytes).map_err(|_|"LIBRARY_RECOVERY_LEDGER_DIVERGENT")?;
    registry.validate()?;verify_registered_origins_using(&registry,sources,Origin::matches_reserved_origin)
}

pub(crate) fn validate_recovery_snapshot(
    bytes: &[u8], anchor: &[u8], manifests: &[crate::local_media::mirror::MirrorManifest],
) -> Result<(), ()> {
    if bytes.len() as u64 > MAX_BYTES { return Err(()); }
    let registry: Registry = serde_json::from_slice(bytes).map_err(|_| ())?;
    registry.validate().map_err(|_| ())?;
    if anchor != registry.namespace.as_bytes() { return Err(()); }
    for manifest in manifests {
        if let Some(namespace) = &manifest.registry_id {
            if namespace != &registry.namespace || manifest.sources.iter().any(|(id, root)| {
                registry.ids.get(id).is_none_or(|r| r.root.as_ref() != Some(root) || r.origin.is_none())
            }) { return Err(()); }
        }
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
                    "a origem ativa diverge dos sinais registrados; não será reatribuída automaticamente",
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
    verify_registered_origins_using(registry, sources, Origin::matches_active)
}
fn verify_registered_origins_using(
    registry: &Registry,
    sources: &BTreeMap<String, PathBuf>,
    same_origin: fn(&Origin, &Origin) -> bool,
) -> Result<(), String> {
    for (id, root) in sources {
        let observed = Origin::capture(root)?;
        if !registry.ids.get(id).is_some_and(|r| {
            r.root.as_ref() == Some(root)
                && r.origin
                    .as_ref()
                    .is_some_and(|o| same_origin(o, &observed))
        }) {
            return Err(error("origem da fonte diverge dos sinais registrados"));
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

    #[test]
    fn proposed_active_policy_observation_table_and_retired_separation() {
        let weak=Origin {filesystem:None,device:11,inode:22,birth:None};
        let renumbered=Origin {device:12,..weak.clone()};
        assert!(weak.matches_active(&renumbered), "Active source: device alone is not replacement");
        assert!(!weak.proves_reactivation(&renumbered));
        let observed_strong=Origin {filesystem:Some([31,32]),device:12,inode:22,birth:Some((41,42))};
        assert!(weak.matches_active(&observed_strong), "Missing optional hints do not contradict");
        assert!(observed_strong.matches_active(&weak));
        assert!(!weak.proves_reactivation(&observed_strong));
        assert!(!observed_strong.proves_reactivation(&weak));
        assert!(observed_strong.proves_reactivation(&Origin{device:99,..observed_strong.clone()}));
        assert!(!observed_strong.matches_active(&Origin{filesystem:Some([51,52]),..observed_strong.clone()}));
        assert!(!observed_strong.matches_active(&Origin{birth:Some((61,62)),..observed_strong.clone()}));
        assert!(!weak.matches_active(&Origin{inode:23,..renumbered}));
        assert!(!observed_strong.proves_reactivation(&Origin{birth:Some((61,62)),..observed_strong.clone()}));
    }

    #[test]
    fn proposed_active_policy_six_active_ids_survive_without_ledger_rewrite() {
        let f=Fixture::new();
        let retired=(0..3).map(|i|f.root(&format!("Retired{i}"))).collect::<Vec<_>>();
        f.reserve(&retired,&BTreeMap::new());
        let roots=(0..6).map(|i|f.root(&format!("Source{i}"))).collect::<Vec<_>>();
        f.seed(&roots);
        let manifest=mirror::read_manifest(&f.library()).unwrap();
        let mut registry:Registry=serde_json::from_slice(&fs::read(f.0.join(NAME)).unwrap()).unwrap();
        for id in manifest.sources.keys() {
            let o=registry.ids.get_mut(id).unwrap().origin.as_mut().unwrap();
            o.filesystem=None;o.birth=None;o.device=o.device.wrapping_add(1);
        }
        registry.checksum=registry.checksum().unwrap();
        fs::write(f.0.join(NAME),serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
        let ledger=fs::read(f.0.join(NAME)).unwrap();
        let anchor=fs::read(f.0.join(LOCK)).unwrap();
        let marker=fs::read(f.library().join(mirror::MANIFEST_NAME)).unwrap();
        assert_eq!(registry.ids.len(),9);
        verify_if_initialized(&f.library(),&manifest.sources).unwrap();
        verify_origins(&f.library(),&manifest.sources).unwrap();
        // Historical recovery keeps the former strict comparison.
        assert!(verify_recovery_origins(&ledger,&manifest.sources).is_err());
        let folders=roots.iter().map(|p|p.to_string_lossy().into_owned()).collect::<Vec<_>>();
        assert!(crate::local_media::resolve_local_sources(&f.library(),&folders).is_ok());
        let (mapping,namespace)=reserve(&f.library(),&roots,&current(&manifest.sources),manifest.registry_id.as_deref()).unwrap();
        assert_eq!(mapping,manifest.sources);
        assert_eq!(Some(namespace),manifest.registry_id);
        for name in ["favorites.json","playlists.json","queue_cache.json"] {
            fs::write(f.0.join(name),br#"{"reference":"Source0/Album/track.wav"}"#).unwrap();
        }
        let offline=f.0.join("temporarily-offline");
        fs::rename(&roots[0],&offline).unwrap();
        assert!(crate::local_media::resolve_local_sources(&f.library(),&folders).is_err());
        assert_eq!(fs::read(f.0.join(NAME)).unwrap(),ledger);
        fs::rename(&offline,&roots[0]).unwrap();
        assert!(crate::local_media::resolve_local_sources(&f.library(),&folders).is_ok());
        assert_eq!(fs::read(f.0.join(NAME)).unwrap(),ledger);
        assert_eq!(fs::read(f.0.join(LOCK)).unwrap(),anchor);
        assert_eq!(fs::read(f.library().join(mirror::MANIFEST_NAME)).unwrap(),marker);
        for name in ["favorites.json","playlists.json","queue_cache.json"] {
            assert_eq!(fs::read(f.0.join(name)).unwrap(),br#"{"reference":"Source0/Album/track.wav"}"#);
        }
    }

    #[test]
    fn proposed_active_policy_canonical_root_change_and_symlink_retarget_are_not_device_drift() {
        let f=Fixture::new();
        let a=f.root("A");let b=f.root("B");let alias=f.0.join("alias");
        std::os::unix::fs::symlink(&a,&alias).unwrap();
        f.seed(&[alias.clone()]);
        let folders=vec![alias.to_string_lossy().into_owned()];
        assert!(crate::local_media::resolve_local_sources(&f.library(),&folders).is_ok());
        let ledger=fs::read(f.0.join(NAME)).unwrap();
        let manifest=mirror::read_manifest(&f.library()).unwrap();
        fs::remove_file(&alias).unwrap();std::os::unix::fs::symlink(&b,&alias).unwrap();
        assert!(crate::local_media::resolve_local_sources(&f.library(),&folders).is_err());
        let changed=manifest.sources.keys().map(|id|(id.clone(),b.clone())).collect::<BTreeMap<_,_>>();
        assert!(verify_if_initialized(&f.library(),&changed).is_err());
        assert_eq!(fs::read(f.0.join(NAME)).unwrap(),ledger);
    }

    #[test]
    fn proposed_active_policy_deliberate_removal_does_not_reactivate_weak_retired_id() {
        let f=Fixture::new();let root=f.root("Music");f.seed(&[root.clone()]);
        let manifest=mirror::read_manifest(&f.library()).unwrap();
        let old_id=manifest.sources.keys().next().unwrap().clone();
        let mut registry:Registry=serde_json::from_slice(&fs::read(f.0.join(NAME)).unwrap()).unwrap();
        let o=registry.ids.get_mut(&old_id).unwrap().origin.as_mut().unwrap();
        o.filesystem=None;o.birth=None;o.device=o.device.wrapping_add(1);
        registry.checksum=registry.checksum().unwrap();
        fs::write(f.0.join(NAME),serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
        let retired_reservation=registry.ids[&old_id].clone();
        let removed=f.prepare(&[],"explicit-remove");
        mirror::publish_prepared_managed_generation(&f.library(),&removed).unwrap().cleanup_previous().unwrap();
        assert!(mirror::read_manifest(&f.library()).unwrap().sources.is_empty());
        let (again,namespace)=f.reserve(&[root],&BTreeMap::new());
        assert_ne!(again.keys().next().unwrap(),&old_id);
        assert_eq!(Some(namespace),manifest.registry_id);
        let after:Registry=serde_json::from_slice(&fs::read(f.0.join(NAME)).unwrap()).unwrap();
        assert_eq!(after.ids[&old_id],retired_reservation);
        assert_eq!(after.ids.len(),2);
    }

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
    fn readonly_recovery_ledger_validation_never_captures_physical_roots_or_reserves_ids() {
        let f = Fixture::new();
        let root = f.root("Music"); f.seed(&[root.clone()]);
        let manifest = mirror::read_manifest(&f.library()).unwrap();
        let bytes = fs::read(f.0.join(NAME)).unwrap();
        let anchor = fs::read(f.0.join(LOCK)).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert!(validate_recovery_snapshot(&bytes, &anchor, &[manifest.clone()]).is_ok());
        for malformed in [b"truncated".to_vec(), {
            let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            json["checksum"] = "invalid".into(); serde_json::to_vec(&json).unwrap()
        }] { assert!(validate_recovery_snapshot(&malformed, &anchor, &[manifest.clone()]).is_err()); }
        assert!(validate_recovery_snapshot(&bytes, b"", &[manifest.clone()]).is_err());
        assert!(validate_recovery_snapshot(&bytes, &[b'f'; 32], &[manifest.clone()]).is_err());
        let mut changed = manifest.clone(); changed.registry_id = Some("0".repeat(32));
        assert!(validate_recovery_snapshot(&bytes, &anchor, &[changed]).is_err());
        let mut changed = manifest; *changed.sources.values_mut().next().unwrap() = PathBuf::from("/synthetic-unavailable-root");
        assert!(validate_recovery_snapshot(&bytes, &anchor, &[changed]).is_err());
        assert_eq!(fs::read(f.0.join(NAME)).unwrap(), bytes);
        assert_eq!(fs::read(f.0.join(LOCK)).unwrap(), anchor);
        assert!(!root.exists());
        let observed = crate::managed_save_intent::inspect_recovery(&f.0, 99, &||false).unwrap();
        assert!(!observed.reasons.contains(&crate::library_recovery::Reason::LedgerDivergent));
        assert!(!observed.recovery_available);
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
                .contains("diverge")
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

// FF04-I1a additive and disconnected. No existing D03/verifier/writer changed.
// The caller must obtain these bounded bytes from its anchored readonly snapshot.
pub(crate) fn recovery_validate_unique(bytes:&[u8], limit:u64)->Result<(),()> {
    if bytes.len() as u64>limit {return Err(());}
    struct Checked;
    impl<'de> Deserialize<'de> for Checked {
        fn deserialize<D:serde::Deserializer<'de>>(d:D)->Result<Self,D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value=Checked;
                fn expecting(&self,f:&mut std::fmt::Formatter)->std::fmt::Result {f.write_str("unambiguous JSON")}
                fn visit_bool<E:serde::de::Error>(self,_:bool)->Result<Checked,E>{Ok(Checked)}
                fn visit_i64<E:serde::de::Error>(self,_:i64)->Result<Checked,E>{Ok(Checked)}
                fn visit_u64<E:serde::de::Error>(self,_:u64)->Result<Checked,E>{Ok(Checked)}
                fn visit_f64<E:serde::de::Error>(self,_:f64)->Result<Checked,E>{Ok(Checked)}
                fn visit_str<E:serde::de::Error>(self,_:&str)->Result<Checked,E>{Ok(Checked)}
                fn visit_unit<E:serde::de::Error>(self)->Result<Checked,E>{Ok(Checked)}
                fn visit_seq<A:serde::de::SeqAccess<'de>>(self,mut a:A)->Result<Checked,A::Error>{
                    while a.next_element::<Checked>()?.is_some(){} Ok(Checked)
                }
                fn visit_map<A:serde::de::MapAccess<'de>>(self,mut a:A)->Result<Checked,A::Error>{
                    let mut exact=std::collections::BTreeSet::new();
                    let mut paths=std::collections::BTreeSet::new();
                    while let Some(key)=a.next_key::<String>()? {
                        // Also reject keys that collapse when deserialized as PathBuf.
                        // Exact decoded strings already catch escaped duplicate IDs.
                        if !exact.insert(key.clone()) || !paths.insert(PathBuf::from(&key)) {
                            return Err(serde::de::Error::custom("ambiguous key"));
                        }
                        a.next_value::<Checked>()?;
                    }
                    Ok(Checked)
                }
            }
            d.deserialize_any(Visitor)
        }
    }
    let mut deserializer=serde_json::Deserializer::from_slice(bytes);
    Checked::deserialize(&mut deserializer).map_err(|_|())?;
    deserializer.end().map_err(|_|())
}
pub(crate) fn recovery_decode_unique<T:serde::de::DeserializeOwned>(bytes:&[u8],limit:u64)->Result<T,()> {
    recovery_validate_unique(bytes,limit)?;
    serde_json::from_slice(bytes).map_err(|_|())
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum PreliminaryHistoricalReason {
    MatchesAtInspection, UnsupportedLedger, LedgerInvalid, LedgerExtensionUnconfirmed,
    ManifestBindingUnconfirmed, RootNotFound, RootPermissionDenied, RootUnsafe,
    RootAccessUnconfirmed, HistoricalMismatch, HistoricalIncomparable,
    ObservationChanged, Cancelled, Unconfirmed,
}
/// Private registry state; deliberately no Serialize/Debug or authorization token.
pub(crate) struct PreliminaryHistoricalContext { registry:Registry, bytes:Vec<u8> }
impl PreliminaryHistoricalContext {
    pub(crate) fn from_readonly_bytes(old:&[u8],old_anchor:&[u8],current:&[u8],anchor:&[u8])
        ->Result<Self,PreliminaryHistoricalReason> {
        use PreliminaryHistoricalReason::*;
        if old_anchor.len()!=32 || anchor.len()!=32 {return Err(LedgerInvalid);}
        let a:Registry=recovery_decode_unique(old,MAX_BYTES).map_err(|_|LedgerInvalid)?;
        let b:Registry=recovery_decode_unique(current,MAX_BYTES).map_err(|_|LedgerInvalid)?;
        if a.schema_version!=VERSION || b.schema_version!=VERSION {return Err(UnsupportedLedger);}
        a.validate().map_err(|_|LedgerInvalid)?; b.validate().map_err(|_|LedgerInvalid)?;
        if old_anchor!=a.namespace.as_bytes() || anchor!=b.namespace.as_bytes() {return Err(LedgerInvalid);}
        if a.namespace!=b.namespace || a.ids.iter().any(|(id,r)|b.ids.get(id)!=Some(r)) {
            return Err(LedgerExtensionUnconfirmed);
        }
        // Retain the original extension validator as a second authority. Failure
        // not established by the checks above is unknown, never parsed from text.
        validate_recovery_extension(old,old_anchor,current,anchor).map_err(|_|Unconfirmed)?;
        Ok(Self{registry:b,bytes:current.to_vec()})
    }
    fn mismatch(expected:&Origin,observed:&Origin)->PreliminaryHistoricalReason {
        use PreliminaryHistoricalReason::*;
        if expected.inode!=observed.inode
            || matches!((expected.filesystem,observed.filesystem),(Some(a),Some(b)) if a!=b)
            || matches!((expected.birth,observed.birth),(Some(a),Some(b)) if a!=b) {
            HistoricalMismatch
        } else if expected.filesystem.is_some()!=observed.filesystem.is_some()
            || expected.birth.is_some()!=observed.birth.is_some() {HistoricalIncomparable}
        else {HistoricalMismatch}
    }
    /// Preliminary physical observation only; no Lease/operation/legacy fence.
    /// No paths or Origin fields escape through the finite outcome enum.
    pub(crate) fn assess_manifest(&self,manifest:&crate::local_media::mirror::MirrorManifest,
        cancel:&dyn Fn()->bool)->PreliminaryHistoricalReason {
        use PreliminaryHistoricalReason::*;
        if cancel(){return Cancelled;}
        if crate::local_media::mirror::validate_manifest(manifest).is_err()
            || manifest.registry_id.as_deref()!=Some(self.registry.namespace.as_str())
            || manifest.sources.iter().any(|(id,root)|self.registry.ids.get(id)
                .is_none_or(|r|r.root.as_ref()!=Some(root)||r.origin.is_none())) {
            return ManifestBindingUnconfirmed;
        }
        for (id,root) in &manifest.sources {
            if cancel(){return Cancelled;}
            let r=&self.registry.ids[id];let expected=r.origin.as_ref().expect("binding checked");
            fn io_reason(e:std::io::Error)->PreliminaryHistoricalReason {
                match e.kind(){std::io::ErrorKind::NotFound=>RootNotFound,
                    std::io::ErrorKind::PermissionDenied=>RootPermissionDenied,_=>RootAccessUnconfirmed}
            }
            let named=match fs::symlink_metadata(root){Ok(m)=>m,Err(e)=>return io_reason(e)};
            if !named.is_dir(){return RootUnsafe;}
            let held=match OpenOptions::new().read(true)
                .custom_flags(libc::O_NOFOLLOW|libc::O_DIRECTORY|libc::O_CLOEXEC).open(root) {
                Ok(f)=>f,Err(e)=>return io_reason(e),
            };
            let opened=match held.metadata(){Ok(m)=>m,Err(_)=>return RootAccessUnconfirmed};
            if !opened.is_dir() || (opened.dev(),opened.ino())!=(named.dev(),named.ino()) {return ObservationChanged;}
            let observed=match Origin::capture(root){Ok(o)=>o,Err(_)=>return RootAccessUnconfirmed};
            if !expected.matches_reserved_origin(&observed){return Self::mismatch(expected,&observed);}
            if cancel(){return Cancelled;}
            let after=match fs::symlink_metadata(root){Ok(m)=>m,Err(_)=>return ObservationChanged};
            if !after.is_dir() || (after.dev(),after.ino())!=(opened.dev(),opened.ino()){return ObservationChanged;}
            let fresh=match Origin::capture(root){Ok(o)=>o,Err(_)=>return RootAccessUnconfirmed};
            if !expected.matches_reserved_origin(&fresh){return Self::mismatch(expected,&fresh);}
        }
        if cancel(){return Cancelled;}
        // Use the unchanged final verifier too. A discrepancy is not a typed
        // mismatch inferred from its String error and is never accepted.
        if verify_recovery_origins(&self.bytes,&manifest.sources).is_err(){return Unconfirmed;}
        if cancel(){Cancelled}else{MatchesAtInspection}
    }
}

#[cfg(test)]
mod ff04_historical_table {
    use super::*;
    #[test] fn strict_active_reactivation_table_uses_original_predicates() {
        let weak=Origin{filesystem:None,device:11,inode:22,birth:None};
        let same=weak.clone();let moved=Origin{device:12,..weak.clone()};
        let strong=Origin{filesystem:Some([31,32]),device:11,inode:22,birth:Some((41,42))};
        let renumbered=Origin{device:99,..strong.clone()};
        assert!(weak.matches_reserved_origin(&same));
        assert!(weak.matches_active(&moved));assert!(!weak.matches_reserved_origin(&moved));
        assert!(weak.matches_active(&strong));assert!(!weak.matches_reserved_origin(&strong));
        assert!(strong.matches_active(&weak));assert!(!strong.matches_reserved_origin(&weak));
        assert!(strong.matches_reserved_origin(&renumbered));assert!(strong.proves_reactivation(&renumbered));
        assert!(!weak.proves_reactivation(&same));
        assert_eq!(PreliminaryHistoricalContext::mismatch(&weak,&strong),PreliminaryHistoricalReason::HistoricalIncomparable);
        // Pure decision table only, not a forged physical-history positive.
    }
    #[test] fn readonly_json_rejects_duplicates_path_aliases_trailing_and_limits() {
        for bad in [br#"{"ids":{"x":1,"x":2}}"#.as_slice(),br#"{"entries":{"a/b":1,"a/./b":2}}"#,br#"{} {}"#] {
            assert!(recovery_decode_unique::<serde_json::Value>(bad,4096).is_err());
        }
        assert!(recovery_decode_unique::<serde_json::Value>(b"{}",1).is_err());
        assert!(recovery_decode_unique::<serde_json::Value>(br#"{"a":[1,null,true]}"#,4096).is_ok());
    }
}

// FF06-I1a: isolated readonly inspection, never product admission.
// Existing reservation/recovery functions and D03 predicates above are unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadonlyStamp {
    dev: u64, ino: u64, mode: u32, uid: u32, links: u64, len: u64,
    mtime: (i64, i64), ctime: (i64, i64),
}
impl ReadonlyStamp {
    pub(crate) fn of(m: &fs::Metadata) -> Self {
        Self { dev: m.dev(), ino: m.ino(), mode: m.mode(), uid: m.uid(),
            links: m.nlink(), len: m.len(), mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()) }
    }
}

/// Anchored readonly descriptor. Its path is internal and never a DTO.
pub(crate) struct ReadonlyDirectory { file: File, path: PathBuf, stamp: ReadonlyStamp }
impl ReadonlyDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, ()> {
        if !path.is_absolute() { return Err(()); }
        let before = fs::symlink_metadata(path).map_err(|_| ())?;
        if !before.is_dir() { return Err(()); }
        let file = OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path).map_err(|_| ())?;
        let stamp = ReadonlyStamp::of(&file.metadata().map_err(|_| ())?);
        if stamp != ReadonlyStamp::of(&before) { return Err(()); }
        let result = Self { file, path: path.into(), stamp };
        result.revalidate()?;
        Ok(result)
    }
    pub(crate) fn revalidate(&self) -> Result<(), ()> {
        let a = self.file.metadata().map_err(|_| ())?;
        let b = fs::symlink_metadata(&self.path).map_err(|_| ())?;
        if !a.is_dir() || !b.is_dir() || a.uid() != unsafe { libc::geteuid() }
            || a.mode() & 0o022 != 0 || ReadonlyStamp::of(&a) != self.stamp
            || ReadonlyStamp::of(&b) != self.stamp { return Err(()); }
        Ok(())
    }
    pub(crate) fn entry(&self, name: &std::ffi::OsStr) -> Result<PathBuf, ()> {
        if Path::new(name).components().count() != 1
            || !matches!(Path::new(name).components().next(), Some(std::path::Component::Normal(_))) {
            return Err(());
        }
        Ok(PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd())).join(name))
    }
    // Only previously captured relative mirror entries use this helper.
    pub(crate) fn descendant(&self, relative: &Path) -> Result<PathBuf, ()> {
        if relative.as_os_str().is_empty() || relative.components().any(|part|
            !matches!(part, std::path::Component::Normal(_))) { return Err(()); }
        Ok(PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd())).join(relative))
    }
    pub(crate) fn change_watch_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}",self.file.as_raw_fd()))
    }
    pub(crate) fn supports_local_change_events(&self) -> bool {
        let mut info=std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe {libc::fstatfs(self.file.as_raw_fd(),info.as_mut_ptr())}!=0 {return false;}
        // An optimization precondition, never source/read admission. Remote,
        // FUSE and unknown filesystems retain the complete scan validation.
        matches!(unsafe {info.assume_init()}.f_type as u64,
            0xef53 | 0x9123683e | 0x58465342 | 0x01021994)
    }
    pub(crate) fn names(&self, limit: usize) -> Result<Vec<std::ffi::OsString>, ()> {
        self.revalidate()?;
        let mut names = Vec::new();
        for e in fs::read_dir(format!("/proc/self/fd/{}", self.file.as_raw_fd())).map_err(|_| ())? {
            if names.len() >= limit { return Err(()); }
            names.push(e.map_err(|_| ())?.file_name());
        }
        names.sort(); self.revalidate()?; Ok(names)
    }
}

pub(crate) struct ReadonlyBoundedFile {
    file: File, name: std::ffi::OsString, stamp: ReadonlyStamp,
    bytes: Vec<u8>, max: u64, private: bool,
}
impl ReadonlyBoundedFile {
    pub(crate) fn open(dir: &ReadonlyDirectory, name: &str, max: u64, private: bool) -> Result<Self, ()> {
        dir.revalidate()?;
        // NONBLOCK prevents a substituted FIFO from hanging before the type check.
        let path = dir.entry(std::ffi::OsStr::new(name))?;
        let file = OpenOptions::new().read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&path).map_err(|_| ())?;
        let stamp = ReadonlyStamp::of(&file.metadata().map_err(|_| ())?);
        let mut result = Self { file, name: name.into(), stamp, bytes: Vec::new(), max, private };
        result.validate_metadata(dir)?;
        result.bytes = result.read_bounded()?;
        result.validate_metadata(dir)?;
        dir.revalidate()?;
        Ok(result)
    }
    fn validate_metadata(&self, dir: &ReadonlyDirectory) -> Result<(), ()> {
        dir.revalidate()?;
        let a = self.file.metadata().map_err(|_| ())?;
        let b = fs::symlink_metadata(dir.entry(&self.name)?).map_err(|_| ())?;
        if !a.is_file() || !b.is_file() || a.uid() != unsafe { libc::geteuid() }
            || a.nlink() != 1 || a.mode() & if self.private { 0o077 } else { 0o022 } != 0
            || a.len() > self.max || ReadonlyStamp::of(&a) != self.stamp
            || ReadonlyStamp::of(&b) != self.stamp { return Err(()); }
        Ok(())
    }
    fn read_bounded(&self) -> Result<Vec<u8>, ()> {
        use std::os::unix::fs::FileExt;
        let mut bytes = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let remaining = (self.max + 1).saturating_sub(bytes.len() as u64) as usize;
            if remaining == 0 { return Err(()); }
            let len = buf.len().min(remaining);
            let n = self.file.read_at(&mut buf[..len], bytes.len() as u64).map_err(|_| ())?;
            if n == 0 { break; }
            bytes.extend_from_slice(&buf[..n]);
            if bytes.len() as u64 > self.max { return Err(()); }
        }
        Ok(bytes)
    }
    pub(crate) fn bytes(&self) -> &[u8] { &self.bytes }
    pub(crate) fn revalidate(&self, dir: &ReadonlyDirectory) -> Result<(), ()> {
        self.validate_metadata(dir)?;
        if self.read_bounded()? != self.bytes { return Err(()); }
        self.validate_metadata(dir)
    }
}

/// serde maps normally accept duplicate keys (including duplicate IDs). Reject
/// duplicates recursively before typed deserialization; reject trailing JSON too.
pub(crate) fn readonly_unique_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ()> {
    struct Unique(serde_json::Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> serde::de::Visitor<'de> for V {
                type Value = Unique;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("JSON without duplicate keys") }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique,E> {
                    serde_json::Number::from_f64(v).map(|n| Unique(n.into())).ok_or_else(|| E::custom("nonfinite"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique,E> { Ok(Unique(v.into())) }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Unique,E> { Ok(Unique(serde_json::Value::Null)) }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a:A) -> Result<Unique,A::Error> {
                    let mut v=Vec::new(); while let Some(Unique(x))=a.next_element()? { v.push(x); }
                    Ok(Unique(v.into()))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a:A) -> Result<Unique,A::Error> {
                    let mut v=serde_json::Map::new();
                    while let Some(k)=a.next_key::<String>()? {
                        if v.contains_key(&k) { return Err(serde::de::Error::custom("duplicate key")); }
                        let Unique(x)=a.next_value()?; v.insert(k,x);
                    }
                    Ok(Unique(v.into()))
                }
            }
            d.deserialize_any(V)
        }
    }
    let Unique(v)=serde_json::from_slice(bytes).map_err(|_| ())?;
    serde_json::from_value(v).map_err(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadonlyOriginObservation { Available, NotFound, PermissionDenied, IoError, Conflict }

pub(crate) struct ReadonlyIdentitySnapshot {
    parent: ReadonlyDirectory, ledger: ReadonlyBoundedFile, anchor: ReadonlyBoundedFile,
    registry: Registry,
}
impl ReadonlyIdentitySnapshot {
    pub(crate) fn open(library: &Path) -> Result<Self, ()> {
        let parent=ReadonlyDirectory::open(library.parent().ok_or(())?)?;
        let ledger=ReadonlyBoundedFile::open(&parent, NAME, MAX_BYTES, true)?;
        let anchor=ReadonlyBoundedFile::open(&parent, LOCK, 32, true)?;
        let registry:Registry=readonly_unique_json(ledger.bytes())?;
        registry.validate().map_err(|_| ())?;
        if anchor.bytes()!=registry.namespace.as_bytes() { return Err(()); }
        let s=Self {parent, ledger, anchor, registry}; s.revalidate()?; Ok(s)
    }
    pub(crate) fn revalidate(&self) -> Result<(), ()> {
        self.ledger.revalidate(&self.parent)?; self.anchor.revalidate(&self.parent)
    }
    pub(crate) fn matches_manifest(&self, namespace:&str, sources:&BTreeMap<String,PathBuf>) -> bool {
        namespace==self.registry.namespace && sources.iter().all(|(id,root)|
            self.registry.ids.get(id).is_some_and(|r| r.root.as_ref()==Some(root) && r.origin.is_some()))
    }
    /// No other root is touched, and no error string or raw Origin escapes.
    pub(crate) fn observe(&self, id:&str) -> Result<ReadonlyOriginObservation, ()> {
        use ReadonlyOriginObservation::*;
        self.revalidate()?;
        let r=self.registry.ids.get(id).ok_or(())?;
        let root=r.root.as_ref().ok_or(())?; let expected=r.origin.as_ref().ok_or(())?;
        fn classify(e:std::io::Error)->ReadonlyOriginObservation {
            match e.kind() { std::io::ErrorKind::NotFound=>NotFound,
                std::io::ErrorKind::PermissionDenied=>PermissionDenied, _=>IoError }
        }
        let m=match fs::symlink_metadata(root) { Ok(m)=>m, Err(e)=>return Ok(classify(e)) };
        if !m.is_dir() { return Ok(Conflict); }
        if let Err(e)=fs::read_dir(root) { return Ok(classify(e)); }
        let current=Origin::capture(root).map_err(|_| ())?;
        if !expected.matches_active(&current) { return Ok(Conflict); }
        // Repeat actual D03 capture after access check; do not trust first observation.
        let after=Origin::capture(root).map_err(|_| ())?;
        if !expected.matches_active(&after) || !current.matches_active(&after) { return Ok(Conflict); }
        self.revalidate()?; Ok(Available)
    }
}
