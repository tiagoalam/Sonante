use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalMediaClass {
    PlayableAudio,
    ControlSidecar,
    ArtworkSidecar,
    Ignored,
}

pub(crate) fn classify_path(path: &Path) -> LocalMediaClass {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return LocalMediaClass::Ignored;
    };

    if [
        "flac", "mp3", "wav", "aif", "aiff", "ogg", "oga", "opus", "aac", "m4a", "mp4", "wv",
        "mpc", "dsf", "dff",
    ]
    .iter()
    .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    {
        LocalMediaClass::PlayableAudio
    } else if extension.eq_ignore_ascii_case("cue") {
        LocalMediaClass::ControlSidecar
    } else if ["jpg", "jpeg", "png", "webp"]
        .iter()
        .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    {
        LocalMediaClass::ArtworkSidecar
    } else {
        LocalMediaClass::Ignored
    }
}

pub(crate) fn is_safe_relative_mpd_uri(relative_uri: &str) -> bool {
    let mut components = Path::new(relative_uri).components();
    let lowercase_uri = relative_uri.to_ascii_lowercase();
    !relative_uri.is_empty()
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.all(|component| matches!(component, Component::Normal(_)))
        && !relative_uri.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        && !relative_uri.contains("://")
        && !lowercase_uri.starts_with("http:")
        && !lowercase_uri.starts_with("https:")
        && !lowercase_uri.contains("x-plex-token")
}

pub(crate) fn is_virtual_cue_track_uri(uri: &str) -> bool {
    let path = Path::new(uri);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let Some(track) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(number) = track.strip_prefix("track") else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let Some(cue_directory) = path.parent() else {
        return false;
    };
    let is_cue = cue_directory
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".cue"));
    let has_physical_folder = cue_directory
        .parent()
        .is_some_and(|folder| !folder.as_os_str().is_empty());
    is_cue && has_physical_folder
}

pub(crate) fn is_catalog_track_uri(uri: &str) -> bool {
    is_virtual_cue_track_uri(uri) || classify_path(Path::new(uri)) == LocalMediaClass::PlayableAudio
}

#[allow(dead_code)]
pub(crate) fn is_structurally_valid_local_uri(uri: &str) -> bool {
    validate_local_uri(uri).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalLibraryTopology {
    Legacy,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedLocalSources {
    topology: LocalLibraryTopology,
    sources: BTreeMap<String, PathBuf>,
}

impl ResolvedLocalSources {
    #[allow(dead_code)]
    pub(crate) fn topology(&self) -> LocalLibraryTopology {
        self.topology
    }

    pub(crate) fn sources(&self) -> &BTreeMap<String, PathBuf> {
        &self.sources
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedLocalPath {
    pub(crate) source_id: String,
    pub(crate) root: PathBuf,
    pub(crate) physical_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalPathResolveError {
    InvalidPath,
    SourceNotFound,
    NotFound,
    OutsideRoot,
    Unavailable(String),
}

impl fmt::Display for LocalPathResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => formatter.write_str("Caminho local relativo inválido."),
            Self::SourceNotFound => formatter.write_str("Source local desconhecida."),
            Self::NotFound => formatter.write_str("Caminho local não encontrado."),
            Self::OutsideRoot => {
                formatter.write_str("Caminho fora da pasta local configurada.")
            }
            Self::Unavailable(_) => {
                formatter.write_str("Biblioteca local indisponível ou incompatível.")
            }
        }
    }
}

pub(crate) fn resolve_local_sources(
    library_dir: &Path,
    configured_local_folders: &[String],
) -> Result<ResolvedLocalSources, LocalPathResolveError> {
    let configured_paths = configured_local_folders
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let canonical_roots = crate::diagnostics::startup_stage("roots.validate", mirror::canonical_configured_roots(&configured_paths))
        .map_err(LocalPathResolveError::Unavailable)?;
    let library_metadata = fs::symlink_metadata(library_dir).map_err(|error| {
        LocalPathResolveError::Unavailable(format!(
            "Biblioteca local indisponível ({}): {error}",
            library_dir.display()
        ))
    })?;
    if !library_metadata.file_type().is_dir() || library_metadata.file_type().is_symlink() {
        return Err(LocalPathResolveError::Unavailable(
            "A biblioteca local não é um diretório real.".to_string(),
        ));
    }

    let marker = library_dir.join(mirror::MANIFEST_NAME);
    let (topology, sources) = match fs::symlink_metadata(&marker) {
        Ok(_) => (
            LocalLibraryTopology::Managed,
            crate::diagnostics::startup_stage("managed.mapping", mirror::import_managed_source_mapping(library_dir, &canonical_roots))
                .map_err(LocalPathResolveError::Unavailable)?,
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => (
            LocalLibraryTopology::Legacy,
            mirror::import_legacy_source_mapping(library_dir, &canonical_roots)
                .map_err(LocalPathResolveError::Unavailable)?,
        ),
        Err(error) => {
            return Err(LocalPathResolveError::Unavailable(format!(
                "Falha ao examinar marker da biblioteca local: {error}"
            )))
        }
    };

    let mapped_roots = sources.values().cloned().collect::<HashSet<_>>();
    if mapped_roots.len() != canonical_roots.len()
        || canonical_roots
            .iter()
            .any(|root| !mapped_roots.contains(root))
    {
        return crate::diagnostics::startup_stage("mapping.root_set", Err(LocalPathResolveError::Unavailable(
            "A topologia da biblioteca diverge das pastas locais configuradas.".to_string(),
        )));
    }

    crate::diagnostics::startup_stage("identity.verify", crate::source_identity::verify_if_initialized(library_dir, &sources))
        .map_err(LocalPathResolveError::Unavailable)?;
    Ok(ResolvedLocalSources { topology, sources })
}

pub(crate) fn resolve_local_path(
    library_dir: &Path,
    uri: &str,
    configured_local_folders: &[String],
) -> Result<ResolvedLocalPath, LocalPathResolveError> {
    let path = validate_local_uri(uri)?;
    let mut components = path.components();
    let source_id = match components.next() {
        Some(Component::Normal(value)) => value
            .to_str()
            .ok_or(LocalPathResolveError::InvalidPath)?
            .to_string(),
        _ => return Err(LocalPathResolveError::InvalidPath),
    };
    let sources = resolve_local_sources(library_dir, configured_local_folders)?;
    let root = sources
        .sources
        .get(&source_id)
        .cloned()
        .ok_or(LocalPathResolveError::SourceNotFound)?;
    let remainder = components.collect::<PathBuf>();
    let candidate = if remainder.as_os_str().is_empty() {
        root.clone()
    } else {
        root.join(remainder)
    };
    let physical_path = fs::canonicalize(&candidate).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            LocalPathResolveError::NotFound
        } else {
            LocalPathResolveError::Unavailable(format!(
                "Falha ao resolver caminho local ({}): {error}",
                candidate.display()
            ))
        }
    })?;
    if !physical_path.starts_with(&root) {
        return Err(LocalPathResolveError::OutsideRoot);
    }
    Ok(ResolvedLocalPath {
        source_id,
        root,
        physical_path,
    })
}

fn validate_local_uri(uri: &str) -> Result<PathBuf, LocalPathResolveError> {
    if uri.is_empty()
        || uri.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        || uri
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || uri.contains("://")
    {
        return Err(LocalPathResolveError::InvalidPath);
    }
    let path = Path::new(uri);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(LocalPathResolveError::InvalidPath);
    }
    Ok(path.to_path_buf())
}

// MEDIA-1B infrastructure is deliberately dormant until the production cutover in MEDIA-1C.
#[allow(dead_code)]
pub(crate) mod mirror {
    use super::{classify_path, is_safe_relative_mpd_uri, LocalMediaClass};
    use serde::{Deserialize, Serialize};
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::ffi::{CStr, CString, OsStr, OsString};
    use std::fs;
    use std::io::{ErrorKind, Read};
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{symlink, MetadataExt};
    use std::path::{Component, Path, PathBuf};
    use std::time::{Duration, Instant};

    const MIRROR_SCHEMA_VERSION: u32 = 2;
    pub(crate) const MANIFEST_NAME: &str = ".sonante-mirror.json";

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct MirrorManifest {
        pub schema_version: u32,
        pub generation_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub registry_id: Option<String>,
        pub sources: BTreeMap<String, PathBuf>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct MirrorBuildResult {
        pub staging_path: PathBuf,
        pub manifest: MirrorManifest,
        pub refresh_plan: ManagedRefreshPlan,
        pub coverage: crate::index_coverage::Inventory,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum ManagedRefreshPlan {
        /// Same source IDs and media entries; only the durable namespace marker changes.
        MetadataOnly,
        Full,
        Targeted {
            directories: Vec<String>,
            changed_files: usize,
        },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct PureSourceRemoval {
        pub removed_ids: Vec<String>,
        pub remaining_ids: Vec<String>,
        pub refresh_plan: ManagedRefreshPlan,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum MirrorComparison {
        Equivalent,
        Different,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum ManagedGenerationPreparation {
        Unchanged,
        Prepared(MirrorBuildResult),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum ManagedMirrorPublicationKind {
        ReplacedExisting,
        FirstPublication,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum ManagedMirrorPreviousKind {
        Legacy,
        Managed,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct ManagedMirrorPublication {
        live_path: PathBuf,
        staging_path: PathBuf,
        rollback: PublicationRollback,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PublicationRollback {
        Exchange {
            published: EntryIdentity,
            previous: EntryIdentity,
            previous_kind: ManagedMirrorPreviousKind,
        },
        FirstPublication {
            published: EntryIdentity,
        },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct EntryIdentity {
        device: u64,
        inode: u64,
    }

    // Private to the immediate validate-then-publish call below. This is not a
    // durable authorization to publish after other work or a lock handoff.
    struct ValidatedManagedPublication {
        live_path: PathBuf,
        staging_path: PathBuf,
        published: EntryIdentity,
        previous: Option<(EntryIdentity, ManagedMirrorPreviousKind)>,
    }

    impl ManagedMirrorPublication {
        /// Identity already proven by publication; optional preview readers use
        /// it to reject replacement generations without changing the transaction.
        pub(crate) fn preview_generation_identity(&self) -> (u64, u64) {
            let identity = match self.rollback {
                PublicationRollback::Exchange { published, .. }
                | PublicationRollback::FirstPublication { published } => published,
            };
            (identity.device, identity.inode)
        }

        pub(crate) fn kind(&self) -> ManagedMirrorPublicationKind {
            match self.rollback {
                PublicationRollback::Exchange { .. } => {
                    ManagedMirrorPublicationKind::ReplacedExisting
                }
                PublicationRollback::FirstPublication { .. } => {
                    ManagedMirrorPublicationKind::FirstPublication
                }
            }
        }

        pub(crate) fn previous_kind(&self) -> Option<ManagedMirrorPreviousKind> {
            match self.rollback {
                PublicationRollback::Exchange { previous_kind, .. } => Some(previous_kind),
                PublicationRollback::FirstPublication { .. } => None,
            }
        }

        pub(crate) fn rollback(&self) -> Result<(), String> {
            match self.rollback {
                PublicationRollback::Exchange {
                    published,
                    previous,
                    ..
                } => {
                    require_identity(&self.live_path, published, "library publicada")?;
                    require_identity(&self.staging_path, previous, "geração anterior")?;
                    rollback_exchange(&self.live_path, &self.staging_path)
                }
                PublicationRollback::FirstPublication { published } => {
                    require_identity(&self.live_path, published, "library publicada")?;
                    atomic_rename_noreplace(&self.live_path, &self.staging_path)
                }
            }
        }

        fn validate_parent(&self, anchor: &OpenRoot) -> Result<(), String> {
            let named = open_directory_path(self.live_path.parent().ok_or("Publicação sem parent")?)?;
            let actual = stat_fd(named.as_raw_fd())?;
            let opened = stat_fd(anchor.parent.as_raw_fd())?;
            if actual.st_dev != opened.st_dev || actual.st_ino != opened.st_ino {
                return Err("Parent da publicação foi substituído.".into());
            }
            Ok(())
        }
        pub(crate) fn cleanup_previous_durable(&self) -> Result<(), String> {
            let anchor = OpenRoot::open(&self.live_path)?;
            self.cleanup_previous()?;
            self.validate_parent(&anchor)?;
            sync_parent_fd(anchor.parent.as_raw_fd())?;
            self.validate_parent(&anchor)
        }

        pub(crate) fn cleanup_previous(&self) -> Result<(), String> {
            let PublicationRollback::Exchange { previous, .. } = self.rollback else {
                return Ok(());
            };
            require_identity(&self.staging_path, previous, "geração anterior")?;
            let marker = self.staging_path.join(MANIFEST_NAME);
            match fs::symlink_metadata(&marker) {
                Ok(_) => cleanup_managed_generation(&self.staging_path),
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    cleanup_legacy_generation(&self.staging_path)
                }
                Err(error) => Err(format!(
                    "Falha ao examinar geração anterior antes do cleanup: {error}"
                )),
            }
        }

        pub(crate) fn cleanup_rejected_after_rollback(&self) -> Result<(), String> {
            let published = match self.rollback {
                PublicationRollback::Exchange { published, .. }
                | PublicationRollback::FirstPublication { published } => published,
            };
            require_identity(&self.staging_path, published, "geração rejeitada")?;
            cleanup_managed_generation(&self.staging_path)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum MirrorCrashState {
        LegacyOnly,
        ManagedLiveNormal,
        AbandonedStaging,
        LikelyPostExchangePreConfig,
        PreviousGenerationPendingCleanup,
        Ambiguous,
        Invalid,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum PublishedEntry {
        Directory,
        Symlink(PathBuf),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PublishedTargetValidation {
        Strict,
        AllowMissingFromPreviousGeneration,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct ManagedGenerationSnapshot {
        registry_id: Option<String>,
        sources: BTreeMap<String, PathBuf>,
        entries: BTreeMap<PathBuf, PublishedEntry>,
    }

    const MAX_TARGETED_CHANGED_FILES: usize = 32;
    const MAX_TARGETED_DIRECTORIES: usize = 8;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ExistingTree {
        Missing,
        Legacy { compatible: bool },
        Managed { compatible: bool },
        Unexpected,
        Invalid,
    }

    #[derive(Debug)]
    struct ScannedSource {
        root: PathBuf,
        files: Vec<ScannedFile>,
        playable: HashSet<PathBuf>,
        cues: Vec<PathBuf>,
        excluded: usize,
    }

    #[derive(Debug)]
    struct ScannedFile {
        physical: PathBuf,
        relative: PathBuf,
    }

    pub(crate) fn build_filtered_staging(
        staging_path: &Path,
        live_library_path: &Path,
        configured_roots: &[PathBuf],
        generation_id: &str,
    ) -> Result<MirrorBuildResult, String> {
        build_filtered_staging_with_hook(
            staging_path,
            live_library_path,
            configured_roots,
            generation_id,
            || Ok(()),
        )
    }

    fn build_filtered_staging_with_hook<F>(
        staging_path: &Path,
        live_library_path: &Path,
        configured_roots: &[PathBuf],
        generation_id: &str,
        before_publish: F,
    ) -> Result<MirrorBuildResult, String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        validate_generation_id(generation_id)?;
        validate_sibling_paths(staging_path, live_library_path)?;
        if path_exists_without_following(staging_path)? {
            return Err(format!(
                "A staging da biblioteca já existe: {}",
                staging_path.display()
            ));
        }

        let roots = canonical_configured_roots(configured_roots)?;
        // Preserve the legacy/topology validation before reserving any new ID.
        load_existing_source_mapping(live_library_path, &roots)?;
        let (current, namespace) = identity_sources(live_library_path)?;
        let (sources, registry_id) = crate::source_identity::reserve(
            live_library_path, &roots, &current, namespace.as_deref(),
        )?;
        let mut scanned = Vec::with_capacity(sources.len());
        for (source_id, root) in &sources {
            scanned.push((source_id.clone(), scan_source(root)?));
        }

        fs::create_dir(staging_path).map_err(|error| {
            format!(
                "Falha ao criar staging da biblioteca ({}): {error}",
                staging_path.display()
            )
        })?;

        let build_result = (|| -> Result<MirrorBuildResult, String> {
            before_publish()?;
            crate::source_identity::verify_origins(staging_path, &sources)?;
            let mut coverage = crate::index_coverage::Inventory::new(generation_id, &sources, true)?;
            for (source_id, source) in &scanned {
                publish_source(staging_path, source_id, source, &mut coverage)?;
            }
            let manifest = MirrorManifest {
                schema_version: MIRROR_SCHEMA_VERSION,
                registry_id: Some(registry_id),
                generation_id: generation_id.to_string(),
                sources,
            };
            write_manifest(staging_path, &manifest)?;
            let validated = read_manifest(staging_path)?;
            if validated != manifest {
                return Err("O manifesto validado difere do manifesto gravado.".to_string());
            }
            validate_published_links(staging_path, &scanned)?;
            Ok(MirrorBuildResult {
                staging_path: staging_path.to_path_buf(),
                manifest,
                refresh_plan: ManagedRefreshPlan::Full,
                coverage,
            })
        })();

        match build_result {
            Ok(result) => Ok(result),
            Err(error) => match remove_owned_staging(staging_path) {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(format!(
                    "{error}; cleanup da staging também falhou: {cleanup_error}"
                )),
            },
        }
    }

    pub(crate) fn load_existing_source_mapping(
        library_path: &Path,
        canonical_roots: &[PathBuf],
    ) -> Result<BTreeMap<String, PathBuf>, String> {
        let metadata = match fs::symlink_metadata(library_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => {
                return Err(format!(
                    "Falha ao examinar biblioteca existente ({}): {error}",
                    library_path.display()
                ))
            }
        };
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("A biblioteca existente não é um diretório real.".to_string());
        }

        let marker_path = library_path.join(MANIFEST_NAME);
        match fs::symlink_metadata(&marker_path) {
            Ok(_) => import_managed_source_mapping(library_path, canonical_roots),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                import_legacy_source_mapping(library_path, canonical_roots)
            }
            Err(error) => Err(format!(
                "Falha ao examinar marker da biblioteca existente ({}): {error}",
                marker_path.display()
            )),
        }
    }

    /// All observable IDs, including removed or dangling legacy entries, are
    /// reserved before the next generation is built. Never infer history from
    /// personal references or restrict this list to the proposed roots.
    pub(crate) fn identity_sources(
        library_path: &Path,
    ) -> Result<(BTreeMap<String, Option<PathBuf>>, Option<String>), String> {
        match fs::symlink_metadata(library_path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok((BTreeMap::new(), None)),
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
            _ => return Err("Biblioteca de identidades inválida.".into()),
        }
        match fs::symlink_metadata(library_path.join(MANIFEST_NAME)) {
            Ok(_) => {
                let manifest = read_manifest(library_path)?;
                validate_cleanup_tree(library_path, &manifest)?;
                return Ok((
                    manifest
                        .sources
                        .into_iter()
                        .map(|(id, root)| (id, Some(root)))
                        .collect(),
                    manifest.registry_id,
                ));
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let mut result = BTreeMap::new();
        for entry in sorted_entries(library_path)? {
            let id = entry
                .file_name()
                .to_str()
                .and_then(valid_source_id)
                .ok_or_else(|| "Identificador legado inválido.".to_string())?;
            let root = if fs::symlink_metadata(entry.path())
                .map_err(|e| e.to_string())?
                .file_type()
                .is_symlink()
            {
                match fs::canonicalize(entry.path()) {
                    Ok(root) => Some(root),
                    Err(e) if e.kind() == ErrorKind::NotFound => None,
                    Err(e) => return Err(e.to_string()),
                }
            } else {
                None
            };
            result.insert(id, root);
        }
        Ok((result, None))
    }

    pub(crate) fn import_managed_source_mapping(
        library_path: &Path,
        canonical_roots: &[PathBuf],
    ) -> Result<BTreeMap<String, PathBuf>, String> {
        let manifest = crate::diagnostics::startup_stage("managed.read_manifest", read_manifest(library_path))?;
        crate::diagnostics::startup_stage("managed.topology", validate_cleanup_tree(library_path, &manifest))
            .map_err(|error| format!("Geração gerenciada incompatível: {error}"))?;
        let configured = canonical_roots.iter().cloned().collect::<HashSet<_>>();
        let mut by_target = HashMap::new();
        let mut mapping = BTreeMap::new();
        for (source_id, root) in manifest.sources {
            if !configured.contains(&root) {
                continue;
            }
            if let Some(previous) = by_target.insert(root.clone(), source_id.clone()) {
                return Err(format!(
                    "A root gerenciada possui IDs ambíguos: {previous} e {source_id}"
                ));
            }
            mapping.insert(source_id, root);
        }
        Ok(mapping)
    }

    pub(crate) fn import_legacy_source_mapping(
        library_path: &Path,
        canonical_roots: &[PathBuf],
    ) -> Result<BTreeMap<String, PathBuf>, String> {
        let metadata = fs::symlink_metadata(library_path).map_err(|error| {
            format!(
                "Falha ao examinar biblioteca legada ({}): {error}",
                library_path.display()
            )
        })?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err("A biblioteca legada não é um diretório real.".to_string());
        }
        let configured = canonical_roots.iter().cloned().collect::<HashSet<_>>();
        let mut by_target = HashMap::new();
        let mut mapping = BTreeMap::new();
        for entry in sorted_entries(library_path)? {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("Falha ao examinar entrada legada: {error}"))?;
            if !metadata.file_type().is_symlink() {
                return Err(format!(
                    "A biblioteca legada contém entrada inesperada: {}",
                    path.display()
                ));
            }
            let source_id = entry
                .file_name()
                .to_str()
                .and_then(valid_source_id)
                .ok_or_else(|| format!("Source ID legado inválido: {}", path.display()))?;
            let target = match fs::canonicalize(&path) {
                Ok(target) => target,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "Falha ao resolver source legado ({}): {error}",
                        path.display()
                    ))
                }
            };
            if !configured.contains(&target) {
                continue;
            }
            if let Some(previous) = by_target.insert(target.clone(), source_id.clone()) {
                return Err(format!(
                    "A root legada possui IDs ambíguos: {previous} e {source_id}"
                ));
            }
            mapping.insert(source_id, target);
        }
        Ok(mapping)
    }

    pub(crate) fn write_manifest(
        generation_path: &Path,
        manifest: &MirrorManifest,
    ) -> Result<(), String> {
        validate_manifest(manifest)?;
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| format!("Falha ao serializar manifesto do mirror: {error}"))?;
        fs::write(generation_path.join(MANIFEST_NAME), bytes)
            .map_err(|error| format!("Falha ao gravar manifesto do mirror: {error}"))
    }

    pub(crate) fn read_manifest(generation_path: &Path) -> Result<MirrorManifest, String> {
        let root_metadata = fs::symlink_metadata(generation_path)
            .map_err(|error| format!("Falha ao examinar geração do mirror: {error}"))?;
        if !root_metadata.file_type().is_dir() || root_metadata.file_type().is_symlink() {
            return Err("A geração do mirror não é um diretório real.".to_string());
        }
        let manifest_path = generation_path.join(MANIFEST_NAME);
        let metadata = fs::symlink_metadata(&manifest_path)
            .map_err(|error| format!("Manifesto do mirror indisponível: {error}"))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err("O manifesto do mirror não é um arquivo regular.".to_string());
        }
        let bytes = fs::read(&manifest_path)
            .map_err(|error| format!("Falha ao ler manifesto do mirror: {error}"))?;
        let manifest: MirrorManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Manifesto do mirror inválido: {error}"))?;
        validate_manifest(&manifest)?;
        if let Some(namespace) = &manifest.registry_id {
            crate::source_identity::check_manifest(generation_path, namespace, &manifest.sources)?;
        }
        Ok(manifest)
    }

    pub(crate) fn is_managed_generation(path: &Path) -> bool {
        read_manifest(path)
            .and_then(|manifest| validate_cleanup_tree(path, &manifest))
            .is_ok()
    }

    pub(crate) fn compare_managed_generations(
        left: &Path,
        right: &Path,
    ) -> Result<MirrorComparison, String> {
        let left = managed_generation_snapshot(left)?;
        let right = managed_generation_snapshot(right)?;
        if left == right {
            Ok(MirrorComparison::Equivalent)
        } else {
            Ok(MirrorComparison::Different)
        }
    }

    pub(crate) fn prepare_managed_generation(
        live_library_path: &Path,
        staging_path: &Path,
        configured_roots: &[PathBuf],
        generation_id: &str,
    ) -> Result<ManagedGenerationPreparation, String> {
        prepare_managed_generation_for_refresh(live_library_path, staging_path,
            configured_roots, generation_id, false)
    }

    pub(crate) fn prepare_managed_generation_for_refresh(
        live_library_path: &Path,
        staging_path: &Path,
        configured_roots: &[PathBuf],
        generation_id: &str,
        force_refresh: bool,
    ) -> Result<ManagedGenerationPreparation, String> {
        let live_has_managed_marker = path_exists_without_following(
            &live_library_path.join(MANIFEST_NAME),
        )?;
        let preparation_started = Instant::now();
        let prepared = build_filtered_staging(
            staging_path,
            live_library_path,
            configured_roots,
            generation_id,
        );
        let preparation_duration = preparation_started.elapsed();
        let mut prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                log_preparation_timings(
                    preparation_duration,
                    Duration::ZERO,
                    "error",
                    &ManagedRefreshPlan::Full,
                );
                return Err(error);
            }
        };

        if !live_has_managed_marker {
            log_preparation_timings(
                preparation_duration,
                Duration::ZERO,
                "prepared",
                &prepared.refresh_plan,
            );
            return Ok(ManagedGenerationPreparation::Prepared(prepared));
        }

        let comparison_started = Instant::now();
        let comparison = compare_previous_managed_generation_with_staging(
            live_library_path,
            staging_path,
        );
        let comparison_duration = comparison_started.elapsed();
        match comparison {
            Ok((MirrorComparison::Equivalent, _)) => {
                // A user-requested rescan keeps a publication/previous generation
                // until audit or typed cancellation, even with identical media.
                if force_refresh {
                    return Ok(ManagedGenerationPreparation::Prepared(prepared));
                }

                cleanup_managed_generation(staging_path)?;
                log_preparation_timings(
                    preparation_duration,
                    comparison_duration,
                    "unchanged",
                    &ManagedRefreshPlan::Full,
                );
                Ok(ManagedGenerationPreparation::Unchanged)
            }
            Ok((MirrorComparison::Different, refresh_plan)) => {
                prepared.refresh_plan = refresh_plan;
                log_preparation_timings(
                    preparation_duration,
                    comparison_duration,
                    "prepared",
                    &prepared.refresh_plan,
                );
                Ok(ManagedGenerationPreparation::Prepared(prepared))
            }
            Err(error) => {
                log_preparation_timings(
                    preparation_duration,
                    comparison_duration,
                    "error",
                    &ManagedRefreshPlan::Full,
                );
                match cleanup_managed_generation(staging_path) {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(format!(
                        "{error}; cleanup da staging preparada também falhou: {cleanup_error}"
                    )),
                }
            }
        }
    }

    pub(crate) fn publish_prepared_managed_generation(
        live_library_path: &Path,
        prepared: &MirrorBuildResult,
    ) -> Result<ManagedMirrorPublication, String> {
        publish_prepared_managed_generation_guarded(live_library_path, prepared, || Ok(()))
    }

    pub(crate) fn publish_prepared_managed_generation_guarded(
        live_library_path: &Path,
        prepared: &MirrorBuildResult,
        before_exchange: impl FnOnce() -> Result<(), String>,
    ) -> Result<ManagedMirrorPublication, String> {
        publish_prepared_with_sync(live_library_path, prepared, before_exchange, sync_parent_fd)
    }

    pub(crate) const PUBLICATION_UNCONFIRMED: &str = "LOCAL_MIRROR_PUBLICATION_UNCONFIRMED";
    fn sync_parent_fd(fd: RawFd) -> Result<(), String> {
        if unsafe { libc::fsync(fd) } != 0 {
            return Err("Sincronização do diretório da publicação não confirmada.".into());
        }
        Ok(())
    }
    fn publish_prepared_with_sync(
        live: &Path,
        prepared: &MirrorBuildResult,
        before_exchange: impl FnOnce() -> Result<(), String>,
        sync: impl FnOnce(RawFd) -> Result<(), String>,
    ) -> Result<ManagedMirrorPublication, String> {
        let validated = validate_prepared_managed_publication(live, prepared)?;
        let metadata = prepared.refresh_plan == ManagedRefreshPlan::MetadataOnly;
        let anchor = if metadata {
            if compare_previous_managed_generation_with_staging(live, &prepared.staging_path)?.1
                != ManagedRefreshPlan::MetadataOnly
            {
                return Err("Plano de migração de identidade mudou.".into());
            }
            Some(OpenRoot::open(live)?)
        } else {
            None
        };
        before_exchange()?;
        if metadata
            && compare_previous_managed_generation_with_staging(live, &prepared.staging_path)?.1
                != ManagedRefreshPlan::MetadataOnly
        {
            return Err("Plano de migração de identidade mudou antes da publicação.".into());
        }
        if metadata {
            if read_manifest(&prepared.staging_path)? != prepared.manifest {
                return Err("Manifesto da migração mudou antes da publicação.".into());
            }
            crate::managed_save_intent::sync_mirror(&prepared.staging_path, 0).map_err(|_| {
                "Sincronização da migração não confirmada; staging preservada.".to_string()
            })?;
        }
        let publication = publish_validated_managed_generation(validated)?;
        if let Some(anchor) = anchor {
            publication.validate_parent(&anchor)
                .and_then(|_| sync(anchor.parent.as_raw_fd()))
                .and_then(|_| publication.validate_parent(&anchor))
                .map_err(|_| format!("{PUBLICATION_UNCONFIRMED}: migração publicada sem confirmação durável; ambas as gerações preservadas."))?;
        }
        Ok(publication)
    }

    pub(crate) const TARGETED_ADDITION_PLAN_CHANGED: &str = "LOCAL_SOURCE_TARGETED_PLAN_CHANGED";

    pub(crate) fn publish_targeted_removed_sources_generation_guarded(
        live_library_path: &Path,
        prepared: &MirrorBuildResult,
        expected: &PureSourceRemoval,
        before_exchange: impl FnOnce() -> Result<(), String>,
    ) -> Result<ManagedMirrorPublication, String> {
        let validated = validate_prepared_managed_publication(live_library_path, prepared)?;
        if plan_removed_sources_refresh(live_library_path, prepared)?.as_ref() != Some(expected) {
            return Err("LOCAL_SOURCE_REMOVAL_PLAN_CHANGED".to_string());
        }
        before_exchange()?;
        publish_validated_managed_generation(validated)
    }

    pub(crate) fn publish_targeted_added_sources_generation(
        live_library_path: &Path,
        prepared: &MirrorBuildResult,
        expected_plan: &ManagedRefreshPlan,
    ) -> Result<ManagedMirrorPublication, String> {
        publish_targeted_added_sources_generation_guarded(
            live_library_path,
            prepared,
            expected_plan,
            || Ok(()),
        )
    }

    pub(crate) fn publish_targeted_added_sources_generation_guarded(
        live_library_path: &Path,
        prepared: &MirrorBuildResult,
        expected_plan: &ManagedRefreshPlan,
        before_exchange: impl FnOnce() -> Result<(), String>,
    ) -> Result<ManagedMirrorPublication, String> {
        if !matches!(expected_plan, ManagedRefreshPlan::Targeted { .. }) {
            return Err(TARGETED_ADDITION_PLAN_CHANGED.to_string());
        }
        let validated = validate_prepared_managed_publication(live_library_path, prepared)?;
        if !matches!(
            validated.previous,
            Some((_, ManagedMirrorPreviousKind::Managed))
        ) {
            return Err(TARGETED_ADDITION_PLAN_CHANGED.to_string());
        }
        // Recheck immediately before exchange: an addition must never silently
        // become a full rescan if the live topology or staged files changed.
        let (current_plan, _) = plan_added_sources_refresh_diagnostic(live_library_path, prepared)?;
        if current_plan.as_ref() != Some(expected_plan) {
            return Err(TARGETED_ADDITION_PLAN_CHANGED.to_string());
        }
        before_exchange()?;
        publish_validated_managed_generation(validated)
    }

    fn validate_prepared_managed_publication(
        live_library_path: &Path,
        prepared: &MirrorBuildResult,
    ) -> Result<ValidatedManagedPublication, String> {
        let staging_path = &prepared.staging_path;
        validate_sibling_paths(staging_path, live_library_path)?;

        let staged_manifest = read_manifest(staging_path)?;
        if staged_manifest != prepared.manifest {
            return Err("A staging preparada não corresponde ao resultado do builder.".to_string());
        }
        // Both snapshots validate symlink targets under physical source roots;
        // metadata/canonicalize here can block on NFS even though the mirror is local.
        managed_generation_snapshot(staging_path)?;
        let published = directory_identity(staging_path, "staging preparada")?;

        let live_metadata = match fs::symlink_metadata(live_library_path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "Falha ao examinar library antes da publicação: {error}"
                ))
            }
        };

        let previous = if let Some(metadata) = live_metadata {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err("A library existente não é um diretório real.".to_string());
            }
            let previous_kind = validate_live_for_publication(live_library_path, &staged_manifest)?;
            Some((
                EntryIdentity {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                },
                previous_kind,
            ))
        } else {
            None
        };
        Ok(ValidatedManagedPublication {
            live_path: live_library_path.to_path_buf(),
            staging_path: staging_path.to_path_buf(),
            published,
            previous,
        })
    }

    fn publish_validated_managed_generation(
        validated: ValidatedManagedPublication,
    ) -> Result<ManagedMirrorPublication, String> {
        let ValidatedManagedPublication {
            live_path,
            staging_path,
            published,
            previous,
        } = validated;
        // Check root identities again after the caller's transaction guard.
        // This opens only source directories, never their musical contents.
        let manifest = read_manifest(&staging_path)?;
        if manifest.registry_id.is_some() {
            crate::source_identity::verify_origins(&staging_path, &manifest.sources)?;
        }
        if let Some((previous, previous_kind)) = previous {
            require_identity(&staging_path, published, "staging preparada")?;
            require_identity(&live_path, previous, "library existente")?;
            atomic_exchange(&live_path, &staging_path)?;
            Ok(ManagedMirrorPublication {
                live_path,
                staging_path,
                rollback: PublicationRollback::Exchange {
                    published,
                    previous,
                    previous_kind,
                },
            })
        } else {
            require_identity(&staging_path, published, "staging preparada")?;
            atomic_rename_noreplace(&staging_path, &live_path)?;
            Ok(ManagedMirrorPublication {
                live_path,
                staging_path,
                rollback: PublicationRollback::FirstPublication { published },
            })
        }
    }

    pub(crate) fn classify_mirror_crash_state(
        live_library_path: &Path,
        next_library_path: &Path,
        configured_roots: &[PathBuf],
    ) -> MirrorCrashState {
        let canonical_roots = match canonical_configured_roots(configured_roots) {
            Ok(roots) => roots,
            Err(_) => return MirrorCrashState::Invalid,
        };
        let live = classify_existing_tree(live_library_path, &canonical_roots);
        let next = classify_existing_tree(next_library_path, &canonical_roots);
        use ExistingTree::{Invalid, Legacy, Managed, Missing, Unexpected};
        match (live, next) {
            (Invalid, _) | (_, Invalid) => MirrorCrashState::Invalid,
            (Legacy { compatible: true }, Missing) => MirrorCrashState::LegacyOnly,
            (Managed { compatible: true }, Missing) => MirrorCrashState::ManagedLiveNormal,
            (
                Legacy { compatible: true },
                Managed {
                    compatible: true,
                },
            ) => MirrorCrashState::AbandonedStaging,
            (
                Legacy { compatible: true },
                Managed { compatible: false },
            ) => MirrorCrashState::AbandonedStaging,
            (
                Legacy { compatible: false } | Managed { compatible: false },
                Managed { compatible: true },
            ) => MirrorCrashState::LikelyPostExchangePreConfig,
            (
                Managed { compatible: true },
                Managed { .. },
            ) => MirrorCrashState::Ambiguous,
            (
                Managed { compatible: true },
                Legacy { compatible: true },
            ) => MirrorCrashState::PreviousGenerationPendingCleanup,
            (_, Unexpected) | (Unexpected, _) => MirrorCrashState::Ambiguous,
            _ => MirrorCrashState::Ambiguous,
        }
    }

    fn managed_generation_snapshot(path: &Path) -> Result<ManagedGenerationSnapshot, String> {
        managed_generation_snapshot_with_target_validation(
            path,
            PublishedTargetValidation::Strict,
        )
    }

    fn compare_previous_managed_generation_with_staging(
        previous: &Path,
        staging: &Path,
    ) -> Result<(MirrorComparison, ManagedRefreshPlan), String> {
        let previous = managed_generation_snapshot_with_target_validation(
            previous,
            PublishedTargetValidation::AllowMissingFromPreviousGeneration,
        )?;
        let staging = managed_generation_snapshot(staging)?;
        if previous == staging {
            Ok((MirrorComparison::Equivalent, ManagedRefreshPlan::Full))
        } else {
            let refresh_plan = targeted_refresh_plan(&previous, &staging);
            Ok((MirrorComparison::Different, refresh_plan))
        }
    }

    /// Same confirmed sources; derive all changed subtrees from BOTH mirror
    /// snapshots. CUE is local to its parent subtree by cue_targets' policy.
    /// This never turns an event into an unqualified global rescan.
    pub(crate) fn plan_filesystem_refresh(previous_path: &Path, prepared: &MirrorBuildResult) -> Result<ManagedRefreshPlan, String> {
        let previous = managed_generation_snapshot_with_target_validation(previous_path, PublishedTargetValidation::AllowMissingFromPreviousGeneration)?;
        let staging = managed_generation_snapshot(&prepared.staging_path)?;
        if previous.sources != staging.sources || staging.sources != prepared.manifest.sources { return Err("LOCAL_WATCH_SOURCE_MAP_CHANGED".into()); }
        let mut directories = std::collections::BTreeSet::new();
        let mut changed_files = 0;
        for path in previous.entries.keys().chain(staging.entries.keys()) {
            if previous.entries.get(path) == staging.entries.get(path) { continue; }
            let Some(source) = path.components().next().and_then(|c| c.as_os_str().to_str()) else { return Err("LOCAL_WATCH_UNSAFE_PATH".into()); };
            if !staging.sources.contains_key(source) { return Err("LOCAL_WATCH_SOURCE_MAP_CHANGED".into()); }
            let parent = targeted_parent(path, staging.entries.contains_key(path), &staging).unwrap_or_else(|| Path::new(source));
            let parent = parent.to_str().ok_or("LOCAL_WATCH_UNSAFE_PATH")?;
            if !is_safe_relative_mpd_uri(parent) { return Err("LOCAL_WATCH_UNSAFE_PATH".into()); }
            directories.insert(parent.to_string()); changed_files += 1;
            // Compress a large structural batch to its involved sources. This
            // bounds commands without dropping work or scanning other sources.
            if directories.len() > MAX_TARGETED_DIRECTORIES {
                directories = directories.iter().map(|p| p.split('/').next().unwrap().to_owned()).collect();
            }
        }
        let directories = directories.iter().filter(|path| !Path::new(path).ancestors().skip(1).any(|parent| parent.to_str().is_some_and(|p| directories.contains(p)))).cloned().collect();
        Ok(ManagedRefreshPlan::Targeted { directories, changed_files })
    }

    pub(crate) fn plan_added_sources_refresh(
        previous_path: &Path,
        prepared: &MirrorBuildResult,
    ) -> Result<Option<ManagedRefreshPlan>, String> {
        plan_added_sources_refresh_diagnostic(previous_path, prepared).map(|(plan, _)| plan)
    }

    pub(crate) fn plan_added_sources_refresh_diagnostic(
        previous_path: &Path,
        prepared: &MirrorBuildResult,
    ) -> Result<(Option<ManagedRefreshPlan>, &'static str), String> {
        match fs::symlink_metadata(previous_path.join(MANIFEST_NAME)) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok((None, "legacy")),
            Err(error) => return Err(format!("Falha ao examinar manifesto anterior: {error}")),
            Ok(_) => {}
        }

        let previous = managed_generation_snapshot_with_target_validation(
            previous_path,
            PublishedTargetValidation::AllowMissingFromPreviousGeneration,
        )?;
        let staging = managed_generation_snapshot(&prepared.staging_path)?;
        if staging.sources != prepared.manifest.sources {
            return Err("A staging mudou durante o planejamento do refresh.".to_string());
        }
        if previous.sources.iter().any(|(id, root)| staging.sources.get(id) != Some(root)) {
            return Ok((None, "previous_mapping_changed"));
        }
        if previous.entries.iter().any(|(path, entry)| staging.entries.get(path) != Some(entry)) {
            return Ok((None, "previous_entries_changed"));
        }

        let added_sources = staging
            .sources
            .keys()
            .filter(|id| !previous.sources.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        if added_sources.is_empty() {
            return Ok((None, "no_added_sources"));
        }
        if added_sources.len() > MAX_TARGETED_DIRECTORIES {
            return Ok((None, "too_many_sources"));
        }
        if added_sources.iter().any(|id| !is_safe_relative_mpd_uri(id)) {
            return Ok((None, "unsafe_mpd_uri"));
        }
        let added_ids = added_sources.iter().map(String::as_str).collect::<HashSet<_>>();
        let mut changed_files = 0;
        for (path, entry) in &staging.entries {
            if previous.entries.contains_key(path) {
                continue;
            }
            let source_id = match path.components().next() {
                Some(Component::Normal(id)) => id.to_str(),
                _ => None,
            };
            if !source_id.is_some_and(|id| added_ids.contains(id)) {
                return Ok((None, "entry_outside_added_sources"));
            }
            if matches!(entry, PublishedEntry::Symlink(_)) {
                changed_files += 1;
            }
        }
        if changed_files == 0 {
            return Ok((None, "no_published_files"));
        }
        Ok((
            Some(ManagedRefreshPlan::Targeted {
                directories: added_sources,
                changed_files,
            }),
            "none",
        ))
    }

    /// Only deletion of complete source IDs, with identical surviving mappings
    /// and mirror entries. This is intentionally separate from other refreshes.
    pub(crate) fn plan_removed_sources_refresh(
        previous_path: &Path,
        prepared: &MirrorBuildResult,
    ) -> Result<Option<PureSourceRemoval>, String> {
        match fs::symlink_metadata(previous_path.join(MANIFEST_NAME)) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("Falha ao examinar manifesto anterior: {error}")),
            Ok(_) => {}
        }
        let previous = managed_generation_snapshot_with_target_validation(
            previous_path, PublishedTargetValidation::AllowMissingFromPreviousGeneration,
        )?;
        let staging = managed_generation_snapshot(&prepared.staging_path)?;
        if staging.sources != prepared.manifest.sources {
            return Err("A staging mudou durante o planejamento da remoção.".to_string());
        }
        if staging.sources.iter().any(|(id, root)| previous.sources.get(id) != Some(root)) {
            return Ok(None);
        }
        let removed_ids = previous.sources.keys()
            .filter(|id| !staging.sources.contains_key(*id)).cloned().collect::<Vec<_>>();
        if removed_ids.is_empty() || removed_ids.len() > MAX_TARGETED_DIRECTORIES
            || removed_ids.iter().any(|id| !is_safe_relative_mpd_uri(id))
        {
            return Ok(None);
        }
        let surviving_entries = previous.entries.iter().filter(|(path, _)| {
            path.components().next().and_then(|part| part.as_os_str().to_str())
                .is_some_and(|id| staging.sources.contains_key(id))
        }).map(|(path, entry)| (path.clone(), entry.clone())).collect::<BTreeMap<_, _>>();
        if surviving_entries != staging.entries {
            return Ok(None);
        }
        let changed_files = previous.entries.iter().filter(|(path, entry)| {
            matches!(entry, PublishedEntry::Symlink(_)) && !staging.entries.contains_key(*path)
        }).count();
        Ok(Some(PureSourceRemoval {
            refresh_plan: ManagedRefreshPlan::Targeted { directories: removed_ids.clone(), changed_files },
            removed_ids,
            remaining_ids: staging.sources.keys().cloned().collect(),
        }))
    }

    fn targeted_refresh_plan(
        previous: &ManagedGenerationSnapshot,
        staging: &ManagedGenerationSnapshot,
    ) -> ManagedRefreshPlan {
        if previous.registry_id.is_none() && staging.registry_id.is_some()
            && previous.sources == staging.sources && previous.entries == staging.entries
        {
            return ManagedRefreshPlan::MetadataOnly;
        }
        if previous.sources != staging.sources {
            return ManagedRefreshPlan::Full;
        }

        let mut changed_files = 0;
        let mut directories = HashSet::new();
        for path in previous.entries.keys().chain(staging.entries.keys()) {
            let before = previous.entries.get(path);
            let after = staging.entries.get(path);
            if before == after {
                continue;
            }
            if before.is_some() && after.is_some() {
                return ManagedRefreshPlan::Full;
            }
            let entry = before.or(after);
            if !matches!(entry, Some(PublishedEntry::Symlink(_))) {
                continue;
            }
            if path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("cue"))
            {
                return ManagedRefreshPlan::Full;
            }
            changed_files += 1;
            if changed_files > MAX_TARGETED_CHANGED_FILES {
                return ManagedRefreshPlan::Full;
            }
            let Some(parent) = targeted_parent(path, after.is_some(), staging) else {
                return ManagedRefreshPlan::Full;
            };
            if parent.as_os_str().is_empty()
                || parent
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return ManagedRefreshPlan::Full;
            }
            let Some(parent) = parent.to_str() else {
                return ManagedRefreshPlan::Full;
            };
            if !is_safe_relative_mpd_uri(parent) {
                return ManagedRefreshPlan::Full;
            }
            directories.insert(parent.to_string());
            if directories.len() > MAX_TARGETED_DIRECTORIES {
                return ManagedRefreshPlan::Full;
            }
        }

        if changed_files == 0 || directories.is_empty() {
            return ManagedRefreshPlan::Full;
        }
        let mut directories = directories.into_iter().collect::<Vec<_>>();
        directories.sort();
        ManagedRefreshPlan::Targeted {
            directories,
            changed_files,
        }
    }

    fn targeted_parent<'a>(
        path: &'a Path,
        is_addition: bool,
        staging: &'a ManagedGenerationSnapshot,
    ) -> Option<&'a Path> {
        let mut parent = path.parent()?;
        if is_addition {
            return matches!(staging.entries.get(parent), Some(PublishedEntry::Directory))
                .then_some(parent);
        }
        loop {
            if matches!(staging.entries.get(parent), Some(PublishedEntry::Directory)) {
                return Some(parent);
            }
            parent = parent.parent()?;
            if parent.as_os_str().is_empty() {
                return None;
            }
        }
    }

    fn log_preparation_timings(
        preparation: Duration,
        comparison: Duration,
        outcome: &str,
        refresh_plan: &ManagedRefreshPlan,
    ) {
        let (strategy, changed_files, directories) = match refresh_plan {
            _ if outcome == "unchanged" => ("none", 0, 0),
            ManagedRefreshPlan::MetadataOnly => ("metadata", 0, 0),
            ManagedRefreshPlan::Full => ("full", 0, 0),
            ManagedRefreshPlan::Targeted {
                directories,
                changed_files,
            } => ("targeted", *changed_files, directories.len()),
        };
        eprintln!(
            "[Mirror] preparation_ms={} comparison_ms={} outcome={} refresh={} changed_files={} directories={}",
            preparation.as_millis(),
            comparison.as_millis(),
            outcome,
            strategy,
            changed_files,
            directories
        );
    }

    fn managed_generation_snapshot_with_target_validation(
        path: &Path,
        target_validation: PublishedTargetValidation,
    ) -> Result<ManagedGenerationSnapshot, String> {
        let root = OpenRoot::open(path)?;
        let manifest = read_manifest_at(root.directory.as_raw_fd())?;
        validate_manifest_roots(&manifest)?;
        if let Some(namespace) = &manifest.registry_id {
            crate::source_identity::check_manifest(path, namespace, &manifest.sources)?;
        }
        let allowed_sources = manifest
            .sources
            .keys()
            .map(OsString::from)
            .collect::<HashSet<_>>();
        validate_tree_at(
            root.directory.as_raw_fd(),
            TreeLevel::ManagedRoot(&allowed_sources),
            &mut |_| Ok(()),
        )?;
        let mut entries = BTreeMap::new();
        for (source_id, source_root) in &manifest.sources {
            let name = os_str_c_string(OsStr::new(source_id))?;
            let Some(metadata) = stat_at(root.directory.as_raw_fd(), &name)? else {
                continue;
            };
            if file_kind(&metadata) != libc::S_IFDIR {
                return Err(format!("Source managed inválida: {source_id}"));
            }
            let source = open_directory_at(root.directory.as_raw_fd(), &name)?;
            ensure_same_entry(root.directory.as_raw_fd(), &name, source.as_raw_fd())?;
            entries.insert(PathBuf::from(source_id), PublishedEntry::Directory);
            collect_published_entries(
                source.as_raw_fd(),
                Path::new(source_id),
                source_root,
                &mut entries,
                target_validation,
            )?;
        }
        Ok(ManagedGenerationSnapshot {
            registry_id: manifest.registry_id,
            sources: manifest.sources,
            entries,
        })
    }

    fn validate_manifest_roots(manifest: &MirrorManifest) -> Result<(), String> {
        for root in manifest.sources.values() {
            let canonical = fs::canonicalize(root).map_err(|error| {
                format!("Root física do manifesto indisponível ({}): {error}", root.display())
            })?;
            let metadata = fs::symlink_metadata(root)
                .map_err(|error| format!("Falha ao examinar root do manifesto: {error}"))?;
            if canonical != *root
                || !metadata.file_type().is_dir()
                || metadata.file_type().is_symlink()
            {
                return Err("Manifesto contém root física não canônica ou insegura.".to_string());
            }
        }
        Ok(())
    }

    fn collect_published_entries(
        directory_fd: RawFd,
        relative_directory: &Path,
        source_root: &Path,
        entries: &mut BTreeMap<PathBuf, PublishedEntry>,
        target_validation: PublishedTargetValidation,
    ) -> Result<(), String> {
        for name in directory_names(directory_fd)? {
            let c_name = os_str_c_string(&name)?;
            let Some(metadata) = stat_at(directory_fd, &c_name)? else {
                continue;
            };
            let relative = relative_directory.join(&name);
            match file_kind(&metadata) {
                libc::S_IFDIR => {
                    let child = open_directory_at(directory_fd, &c_name)?;
                    ensure_same_entry(directory_fd, &c_name, child.as_raw_fd())?;
                    entries.insert(relative.clone(), PublishedEntry::Directory);
                    collect_published_entries(
                        child.as_raw_fd(),
                        &relative,
                        source_root,
                        entries,
                        target_validation,
                    )?;
                }
                libc::S_IFLNK => {
                    let target = read_link_at(directory_fd, &c_name)?;
                    validate_published_target(&target, source_root, target_validation)?;
                    entries.insert(relative, PublishedEntry::Symlink(target));
                }
                _ => {
                    return Err(format!(
                        "Entrada inesperada em geração managed: {}",
                        relative.display()
                    ))
                }
            }
        }
        Ok(())
    }

    fn validate_published_target(
        target: &Path,
        source_root: &Path,
        target_validation: PublishedTargetValidation,
    ) -> Result<(), String> {
        if !target.is_absolute() || !target.starts_with(source_root) {
            return Err("Symlink publicado aponta para fora da root do source.".to_string());
        }
        let relative = target
            .strip_prefix(source_root)
            .map_err(|_| "Symlink publicado aponta para fora da root do source.".to_string())?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err("Target publicado contém path relativo inseguro.".to_string());
        }
        if classify_path(target) == LocalMediaClass::Ignored {
            return Err("Target publicado não pertence à política local.".to_string());
        }
        let metadata = match fs::symlink_metadata(target) {
            Ok(metadata) => metadata,
            Err(error)
                if target_validation
                    == PublishedTargetValidation::AllowMissingFromPreviousGeneration
                    && error.raw_os_error() == Some(libc::ENOENT) =>
            {
                return validate_missing_published_target(target, source_root);
            }
            Err(error) => return Err(format!("Target publicado indisponível: {error}")),
        };
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err("Target publicado não é um arquivo regular.".to_string());
        }
        let canonical = fs::canonicalize(target)
            .map_err(|error| format!("Falha ao canonicalizar target publicado: {error}"))?;
        if canonical != target || !canonical.starts_with(source_root) {
            return Err("Target publicado não é canônico ou escapou da root.".to_string());
        }
        Ok(())
    }

    fn validate_missing_published_target(target: &Path, source_root: &Path) -> Result<(), String> {
        let mut ancestor = target
            .parent()
            .ok_or_else(|| "Target publicado ausente não possui diretório pai.".to_string())?;
        loop {
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                        return Err(
                            "Ancestral de target publicado ausente não é diretório real."
                                .to_string(),
                        );
                    }
                    let canonical = fs::canonicalize(ancestor).map_err(|error| {
                        format!("Falha ao canonicalizar ancestral de target ausente: {error}")
                    })?;
                    if canonical != ancestor || !canonical.starts_with(source_root) {
                        return Err(
                            "Target publicado ausente possui ancestral inseguro.".to_string(),
                        );
                    }
                    return Ok(());
                }
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                    if ancestor == source_root {
                        return Err("Root do target publicado ausente desapareceu.".to_string());
                    }
                    ancestor = ancestor.parent().ok_or_else(|| {
                        "Target publicado ausente escapou da root do source.".to_string()
                    })?;
                }
                Err(error) => {
                    return Err(format!(
                        "Falha ao validar ancestral de target ausente: {error}"
                    ))
                }
            }
        }
    }

    fn read_link_at(directory_fd: RawFd, name: &CStr) -> Result<PathBuf, String> {
        let mut capacity = 256;
        loop {
            let mut bytes = vec![0_u8; capacity];
            let length = unsafe {
                // SAFETY: bytes is writable for capacity bytes; name and directory_fd remain valid.
                libc::readlinkat(
                    directory_fd,
                    name.as_ptr(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if length < 0 {
                return Err(format!(
                    "Falha ao ler target de symlink managed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let length = length as usize;
            if length < bytes.len() {
                bytes.truncate(length);
                return Ok(PathBuf::from(OsString::from_vec(bytes)));
            }
            capacity = capacity
                .checked_mul(2)
                .ok_or_else(|| "Target de symlink excede o tamanho suportado.".to_string())?;
        }
    }

    fn classify_existing_tree(path: &Path, canonical_roots: &[PathBuf]) -> ExistingTree {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return ExistingTree::Missing,
            Err(_) => return ExistingTree::Invalid,
        };
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return ExistingTree::Invalid;
        }
        let marker = path.join(MANIFEST_NAME);
        match fs::symlink_metadata(&marker) {
            Ok(_) => match managed_generation_snapshot(path) {
                Ok(snapshot) => ExistingTree::Managed {
                    compatible: mapping_covers_roots(&snapshot.sources, canonical_roots),
                },
                Err(_) => ExistingTree::Invalid,
            },
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let entries = match sorted_entries(path) {
                    Ok(entries) => entries,
                    Err(_) => return ExistingTree::Invalid,
                };
                if entries.iter().any(|entry| {
                    fs::symlink_metadata(entry.path())
                        .map(|metadata| !metadata.file_type().is_symlink())
                        .unwrap_or(true)
                }) {
                    return ExistingTree::Unexpected;
                }
                match import_legacy_source_mapping(path, canonical_roots) {
                    Ok(mapping) => ExistingTree::Legacy {
                        compatible: mapping_covers_roots(&mapping, canonical_roots),
                    },
                    Err(_) => ExistingTree::Invalid,
                }
            }
            Err(_) => ExistingTree::Invalid,
        }
    }

    fn mapping_covers_roots(
        mapping: &BTreeMap<String, PathBuf>,
        canonical_roots: &[PathBuf],
    ) -> bool {
        let mapped = mapping.values().collect::<HashSet<_>>();
        mapped.len() == canonical_roots.len()
            && canonical_roots.iter().all(|root| mapped.contains(root))
    }

    fn validate_live_for_publication(
        live_library_path: &Path,
        staged_manifest: &MirrorManifest,
    ) -> Result<ManagedMirrorPreviousKind, String> {
        let marker = live_library_path.join(MANIFEST_NAME);
        match fs::symlink_metadata(&marker) {
            Ok(_) => {
                managed_generation_snapshot_with_target_validation(
                    live_library_path,
                    PublishedTargetValidation::AllowMissingFromPreviousGeneration,
                )?;
                Ok(ManagedMirrorPreviousKind::Managed)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let roots = staged_manifest.sources.values().cloned().collect::<Vec<_>>();
                load_existing_source_mapping(live_library_path, &roots)?;
                Ok(ManagedMirrorPreviousKind::Legacy)
            }
            Err(error) => Err(format!(
                "Falha ao examinar marker da library existente: {error}"
            )),
        }
    }

    fn directory_identity(path: &Path, description: &str) -> Result<EntryIdentity, String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("Falha ao examinar {description}: {error}"))?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("{description} não é um diretório real."));
        }
        Ok(EntryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn require_identity(
        path: &Path,
        expected: EntryIdentity,
        description: &str,
    ) -> Result<(), String> {
        let actual = directory_identity(path, description)?;
        if actual != expected {
            return Err(format!(
                "{description} mudou desde a validação; operação atômica recusada."
            ));
        }
        Ok(())
    }

    fn atomic_rename_noreplace(source: &Path, destination: &Path) -> Result<(), String> {
        validate_sibling_paths(source, destination)?;
        if source == destination {
            return Err("Origem e destino do rename precisam ser distintos.".to_string());
        }
        fs::symlink_metadata(source)
            .map_err(|error| format!("Origem indisponível para rename: {error}"))?;
        let source = path_c_string(source)?;
        let destination = path_c_string(destination)?;
        let result = unsafe {
            // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        Err(rename_noreplace_error(std::io::Error::last_os_error()))
    }

    pub(crate) fn atomic_exchange(left: &Path, right: &Path) -> Result<(), String> {
        let left_metadata = fs::symlink_metadata(left)
            .map_err(|error| format!("Path esquerdo indisponível para exchange: {error}"))?;
        let right_metadata = fs::symlink_metadata(right)
            .map_err(|error| format!("Path direito indisponível para exchange: {error}"))?;
        if left == right
            || (left_metadata.dev() == right_metadata.dev()
                && left_metadata.ino() == right_metadata.ino())
        {
            return Err("Os paths do exchange precisam ser distintos.".to_string());
        }
        if left_metadata.dev() != right_metadata.dev() {
            return Err("Os paths do exchange não estão no mesmo filesystem.".to_string());
        }
        let left = path_c_string(left)?;
        let right = path_c_string(right)?;
        let result = unsafe {
            // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
            libc::renameat2(
                libc::AT_FDCWD,
                left.as_ptr(),
                libc::AT_FDCWD,
                right.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        Err(rename_exchange_error(std::io::Error::last_os_error()))
    }

    pub(crate) fn rollback_exchange(left: &Path, right: &Path) -> Result<(), String> {
        atomic_exchange(left, right)
    }

    pub(crate) fn cleanup_managed_generation(path: &Path) -> Result<(), String> {
        cleanup_managed_generation_with_hook(path, None, &mut |_| Ok(()))
    }

    pub(crate) fn cleanup_prepared_managed_generation(
        prepared: &MirrorBuildResult,
    ) -> Result<(), String> {
        cleanup_managed_generation_with_hook(
            &prepared.staging_path,
            Some(&prepared.manifest),
            &mut |_| Ok(()),
        )
    }

    fn cleanup_legacy_generation(path: &Path) -> Result<(), String> {
        let root = OpenRoot::open(path)?;
        validate_tree_at(
            root.directory.as_raw_fd(),
            TreeLevel::LegacyRoot,
            &mut |_| Ok(()),
        )?;
        remove_tree_contents_at(root.directory.as_raw_fd(), TreeLevel::LegacyRoot)?;
        root.remove()
    }

    fn cleanup_managed_generation_with_hook<F>(
        path: &Path,
        expected_manifest: Option<&MirrorManifest>,
        before_directory_open: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&OsStr) -> Result<(), String>,
    {
        cleanup_managed_generation_with_hooks(
            path,
            expected_manifest,
            before_directory_open,
            &mut |_| Ok(()),
        )
    }

    fn cleanup_managed_generation_with_hooks<F, G>(
        path: &Path,
        expected_manifest: Option<&MirrorManifest>,
        before_directory_open: &mut F,
        before_remove: &mut G,
    ) -> Result<(), String>
    where
        F: FnMut(&OsStr) -> Result<(), String>,
        G: FnMut(&OsStr) -> Result<(), String>,
    {
        let root = OpenRoot::open(path)?;
        let manifest = read_manifest_at(root.directory.as_raw_fd())?;
        if expected_manifest.is_some_and(|expected| expected != &manifest) {
            return Err("A staging não pertence mais à tentativa de salvamento.".to_string());
        }
        let allowed_sources = manifest
            .sources
            .keys()
            .map(OsString::from)
            .collect::<HashSet<_>>();
        validate_tree_at(
            root.directory.as_raw_fd(),
            TreeLevel::ManagedRoot(&allowed_sources),
            before_directory_open,
        )?;
        remove_tree_contents_at_with_hook(
            root.directory.as_raw_fd(),
            TreeLevel::ManagedRoot(&allowed_sources),
            before_remove,
        )?;
        root.remove()
    }

    pub(crate) fn canonical_configured_roots(configured_roots: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        let mut roots = Vec::new();
        let mut seen = HashSet::new();
        for configured in configured_roots {
            let root = fs::canonicalize(configured).map_err(|error| {
                format!(
                    "Falha ao resolver root local ({}): {error}",
                    configured.display()
                )
            })?;
            let metadata = fs::symlink_metadata(&root)
                .map_err(|error| format!("Falha ao examinar root local: {error}"))?;
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                return Err(format!(
                    "A root local não é um diretório real: {}",
                    root.display()
                ));
            }
            fs::read_dir(&root).map_err(|error| {
                format!(
                    "A root local não está acessível ({}): {error}",
                    root.display()
                )
            })?;
            if seen.insert(root.clone()) {
                roots.push(root);
            }
        }
        Ok(roots)
    }

    pub(crate) const NEW_SOURCE_OVERLAP: &str = "LOCAL_SOURCE_NEW_OVERLAP";

    // Save-only admission. Do not apply this policy to loading/resolving existing
    // sources: old overlaps must remain readable with their existing IDs.
    pub(crate) fn validate_new_source_overlaps(
        previous: &[PathBuf],
        proposed: &[PathBuf],
    ) -> Result<(), String> {
        fn pairs(roots: &[PathBuf]) -> HashSet<(PathBuf, PathBuf)> {
            let mut result = HashSet::new();
            for (index, first) in roots.iter().enumerate() {
                for second in &roots[index + 1..] {
                    if first != second {
                        if first.starts_with(second) {
                            result.insert((second.clone(), first.clone()));
                        } else if second.starts_with(first) {
                            result.insert((first.clone(), second.clone()));
                        }
                    }
                }
            }
            result
        }
        let proposed_pairs = pairs(&canonical_configured_roots(proposed)?);
        if proposed_pairs.is_empty() {
            return Ok(());
        }
        let previous_pairs = pairs(&canonical_configured_roots(previous)?);
        if proposed_pairs.is_subset(&previous_pairs) {
            Ok(())
        } else {
            Err(NEW_SOURCE_OVERLAP.to_string())
        }
    }

    fn scan_source(root: &Path) -> Result<ScannedSource, String> {
        let mut source = ScannedSource {
            root: root.to_path_buf(),
            files: Vec::new(),
            playable: HashSet::new(),
            cues: Vec::new(),
            excluded: 0,
        };
        scan_directory(root, root, &mut source)?;
        Ok(source)
    }

    fn scan_directory(
        root: &Path,
        directory: &Path,
        source: &mut ScannedSource,
    ) -> Result<(), String> {
        for entry in sorted_entries(directory)? {
            let file_type = entry
                .file_type()
                .map_err(|error| format!("Falha ao classificar entrada local: {error}"))?;
            let physical = entry.path();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                scan_directory(root, &physical, source)?;
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let class = classify_path(&physical);
            if class == LocalMediaClass::Ignored {
                source.excluded += 1;
                continue;
            }
            let relative = physical
                .strip_prefix(root)
                .map_err(|_| "Entrada local escapou da root durante traversal.".to_string())?
                .to_path_buf();
            match class {
                LocalMediaClass::PlayableAudio => {
                    source.playable.insert(physical.clone());
                    source.files.push(ScannedFile { physical, relative });
                }
                LocalMediaClass::ArtworkSidecar => {
                    source.files.push(ScannedFile { physical, relative });
                }
                LocalMediaClass::ControlSidecar => source.cues.push(physical),
                LocalMediaClass::Ignored => unreachable!(),
            }
        }
        Ok(())
    }

    fn publish_source(
        staging: &Path,
        source_id: &str,
        source: &ScannedSource,
        coverage: &mut crate::index_coverage::Inventory,
    ) -> Result<(), String> {
        coverage.exclude(source_id, source.excluded);
        for file in &source.files {
            let metadata = publish_link(staging, source_id, file)?;
            if classify_path(&file.physical) == LocalMediaClass::PlayableAudio {
                coverage.observe(source_id, &file.relative, &file.physical, crate::index_coverage::Kind::Audio, &metadata)?;
            }
        }
        for cue in &source.cues {
            if cue_targets(cue, &source.root, &source.playable)? == CueDisposition::Publish {
                let relative = cue
                    .strip_prefix(&source.root)
                    .map_err(|_| "CUE escapou da root durante publicação.".to_string())?
                    .to_path_buf();
                let metadata = publish_link(
                    staging,
                    source_id,
                    &ScannedFile {
                        physical: cue.clone(),
                        relative: relative.clone(),
                    },
                )?;
                coverage.observe(source_id, &relative, cue, crate::index_coverage::Kind::Cue, &metadata)?;
            } else { coverage.exclude(source_id, 1); }
        }
        Ok(())
    }

    fn publish_link(staging: &Path, source_id: &str, file: &ScannedFile) -> Result<fs::Metadata, String> {
        let metadata = publishable_file_metadata(&file.physical)?;
        let destination = staging.join(source_id).join(&file.relative);
        let parent = destination
            .parent()
            .ok_or_else(|| "Destino do mirror sem diretório pai.".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Falha ao criar diretório virtual: {error}"))?;
        symlink(&file.physical, &destination)
            .map_err(|error| format!("Falha ao publicar symlink filtrado: {error}"))?;
        Ok(metadata)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CueDisposition {
        Publish,
        Excluded,
    }

    fn cue_targets(
        cue: &Path,
        root: &Path,
        playable: &HashSet<PathBuf>,
    ) -> Result<CueDisposition, String> {
        cue_targets_with_reader(cue, root, playable, |path| fs::read(path))
    }

    fn cue_targets_with_reader(
        cue: &Path,
        root: &Path,
        playable: &HashSet<PathBuf>,
        read: impl FnOnce(&Path) -> std::io::Result<Vec<u8>>,
    ) -> Result<CueDisposition, String> {
        // This CUE was selected by scan_source. Loss of access is operational,
        // not a content exclusion. The reader seam keeps I/O failures testable.
        validate_publishable_file(cue)?;
        let bytes = read(cue).map_err(|error| format!("Falha ao ler CUE: {error}"))?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(CueDisposition::Excluded);
        };
        let mut targets = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed
                .as_bytes()
                .get(..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"FILE"))
            {
                if trimmed.len() == 4 || !trimmed.as_bytes()[4].is_ascii_whitespace() {
                    return Ok(CueDisposition::Excluded);
                }
            }
            let mut keyword_end = 0;
            for (index, character) in trimmed.char_indices() {
                if character.is_whitespace() {
                    keyword_end = index;
                    break;
                }
            }
            if keyword_end == 0 || !trimmed[..keyword_end].eq_ignore_ascii_case("FILE") {
                continue;
            }
            let Ok(reference) = parse_cue_file_reference(trimmed[keyword_end..].trim()) else {
                return Ok(CueDisposition::Excluded);
            };
            let relative = Path::new(&reference);
            if relative.is_absolute()
                || reference.contains("://")
                || looks_like_windows_absolute_path(&reference)
                || reference
                    .split('/')
                    .any(|component| component.is_empty() || matches!(component, "." | ".."))
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Ok(CueDisposition::Excluded);
            }
            let parent = cue
                .parent()
                .ok_or_else(|| "CUE sem diretório pai.".to_string())?;
            let target = parent.join(relative);
            // Missing/unselected references were never promised by the scan.
            // A selected target that disappears must instead fail validation.
            if classify_path(&target) != LocalMediaClass::PlayableAudio
                || !playable.contains(&target)
            {
                return Ok(CueDisposition::Excluded);
            }
            validate_publishable_file(&target)?;
            let canonical = fs::canonicalize(&target)
                .map_err(|error| format!("Falha ao resolver target CUE: {error}"))?;
            if !canonical.starts_with(root) || canonical != target {
                return Ok(CueDisposition::Excluded);
            }
            targets.push(target);
        }
        if targets.is_empty() {
            return Ok(CueDisposition::Excluded);
        }
        Ok(CueDisposition::Publish)
    }

    fn parse_cue_file_reference(value: &str) -> Result<String, String> {
        if value.starts_with('"') {
            let remainder = &value[1..];
            let closing = remainder
                .find('"')
                .ok_or_else(|| "Diretiva FILE com aspas incompletas.".to_string())?;
            let reference = &remainder[..closing];
            let trailing = remainder[closing + 1..]
                .split_whitespace()
                .collect::<Vec<_>>();
            if reference.is_empty() || trailing.len() != 1 {
                return Err("Diretiva FILE entre aspas é ambígua.".to_string());
            }
            Ok(reference.to_string())
        } else {
            let fields = value.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 2 || fields[0].is_empty() {
                return Err("Diretiva FILE sem aspas é ambígua.".to_string());
            }
            Ok(fields[0].to_string())
        }
    }

    fn looks_like_windows_absolute_path(value: &str) -> bool {
        let bytes = value.as_bytes();
        value.starts_with("\\\\")
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'))
    }

    fn validate_publishable_file(path: &Path) -> Result<(), String> { publishable_file_metadata(path).map(|_| ()) }

    fn publishable_file_metadata(path: &Path) -> Result<fs::Metadata, String> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            format!(
                "Arquivo selecionado indisponível ({}): {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(format!(
                "Arquivo selecionado não é regular: {}",
                path.display()
            ));
        }
        Ok(metadata)
    }

    fn validate_published_links(
        staging: &Path,
        scanned: &[(String, ScannedSource)],
    ) -> Result<(), String> {
        for (source_id, source) in scanned {
            if !source.root.is_dir() {
                return Err(format!(
                    "Root removida durante build: {}",
                    source.root.display()
                ));
            }
            let source_dir = staging.join(source_id);
            if source_dir.exists() {
                validate_generated_entries(&source_dir, &source.root)?;
            }
        }
        Ok(())
    }

    fn validate_generated_entries(path: &Path, root: &Path) -> Result<(), String> {
        for entry in sorted_entries(path)? {
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("Falha ao validar staging: {error}"))?;
            if metadata.file_type().is_symlink() {
                let target = fs::canonicalize(entry.path())
                    .map_err(|error| format!("Symlink publicado ficou inválido: {error}"))?;
                if !target.starts_with(root) {
                    return Err("Symlink publicado escapou da root física.".to_string());
                }
                validate_publishable_file(&target)?;
            } else if metadata.file_type().is_dir() {
                validate_generated_entries(&entry.path(), root)?;
            } else {
                return Err(format!(
                    "Entrada inesperada na staging: {}",
                    entry.path().display()
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn validate_manifest(manifest: &MirrorManifest) -> Result<(), String> {
        if !matches!(
            (manifest.schema_version, manifest.registry_id.as_deref()),
            (1, None) | (2, Some(_))
        ) || manifest.registry_id.as_deref()
            .is_some_and(|id| !crate::source_identity::valid_namespace(id))
        {
            return Err(format!(
                "Schema de mirror não suportado: {}",
                manifest.schema_version
            ));
        }
        validate_generation_id(&manifest.generation_id)?;
        let mut roots = HashSet::new();
        for (source_id, root) in &manifest.sources {
            if valid_source_id(source_id).is_none() || !is_safe_absolute_root(root) {
                return Err("Manifesto contém source inválido.".to_string());
            }
            if !roots.insert(root) {
                return Err("Manifesto contém root física duplicada.".to_string());
            }
        }
        Ok(())
    }

    fn validate_generation_id(value: &str) -> Result<(), String> {
        let lowercase = value.to_ascii_lowercase();
        if value.is_empty()
            || value.chars().any(char::is_control)
            || value.contains("://")
            || lowercase.contains("x-plex-token")
        {
            return Err("Generation ID inválido.".to_string());
        }
        Ok(())
    }

    fn rename_exchange_error(error: std::io::Error) -> String {
        let detail = match error.raw_os_error() {
            Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP) => {
                "renameat2(RENAME_EXCHANGE) não é suportado"
            }
            Some(libc::EXDEV) => "os paths não estão no mesmo filesystem",
            _ => "renameat2(RENAME_EXCHANGE) falhou",
        };
        format!("{detail}: {error}")
    }

    fn rename_noreplace_error(error: std::io::Error) -> String {
        let detail = match error.raw_os_error() {
            Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP) => {
                "renameat2(RENAME_NOREPLACE) não é suportado"
            }
            Some(libc::EEXIST) => "o destino do rename já existe",
            Some(libc::EXDEV) => "os paths não estão no mesmo filesystem",
            _ => "renameat2(RENAME_NOREPLACE) falhou",
        };
        format!("{detail}: {error}")
    }

    fn validate_sibling_paths(staging: &Path, live: &Path) -> Result<(), String> {
        if staging == live {
            return Err("Staging e library precisam ser paths distintos.".to_string());
        }
        let staging_parent = staging
            .parent()
            .ok_or_else(|| "Staging sem parent.".to_string())?;
        let live_parent = live
            .parent()
            .ok_or_else(|| "Library sem parent.".to_string())?;
        let staging_parent = fs::canonicalize(staging_parent)
            .map_err(|error| format!("Parent da staging indisponível: {error}"))?;
        let live_parent = fs::canonicalize(live_parent)
            .map_err(|error| format!("Parent da library indisponível: {error}"))?;
        if staging_parent != live_parent {
            return Err(
                "Staging e library precisam compartilhar o mesmo parent/filesystem.".to_string(),
            );
        }
        Ok(())
    }

    pub(crate) fn valid_source_id(value: &str) -> Option<String> {
        if value == MANIFEST_NAME {
            return None;
        }
        let mut components = Path::new(value).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) if !name.is_empty() => {
                name.to_str().map(str::to_string)
            }
            _ => None,
        }
    }

    fn is_safe_absolute_root(root: &Path) -> bool {
        root.is_absolute()
            && root
                .components()
                .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
            && !root
                .as_os_str()
                .as_bytes()
                .iter()
                .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    }

    fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, String> {
        let mut entries = fs::read_dir(path)
            .map_err(|error| format!("Falha ao listar diretório ({}): {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Falha ao ler entrada de diretório: {error}"))?;
        entries.sort_by_key(fs::DirEntry::file_name);
        Ok(entries)
    }

    fn path_exists_without_following(path: &Path) -> Result<bool, String> {
        match fs::symlink_metadata(path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "Falha ao examinar path ({}): {error}",
                path.display()
            )),
        }
    }

    fn path_c_string(path: &Path) -> Result<CString, String> {
        CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("Path contém NUL: {}", path.display()))
    }

    fn validate_cleanup_tree(path: &Path, manifest: &MirrorManifest) -> Result<(), String> {
        let allowed_sources = manifest
            .sources
            .keys()
            .map(OsString::from)
            .collect::<HashSet<_>>();
        for entry in sorted_entries(path)? {
            let name = entry.file_name();
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("Falha ao validar cleanup: {error}"))?;
            if name == OsStr::new(MANIFEST_NAME) {
                if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                    return Err("Manifesto inseguro durante cleanup.".to_string());
                }
            } else if allowed_sources.contains(&name)
                && metadata.file_type().is_dir()
                && !metadata.file_type().is_symlink()
            {
                validate_cleanup_source(&entry.path())?;
            } else {
                return Err(format!(
                    "Entrada não gerenciada impede cleanup: {}",
                    entry.path().display()
                ));
            }
        }
        Ok(())
    }

    fn validate_cleanup_source(path: &Path) -> Result<(), String> {
        for entry in sorted_entries(path)? {
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| format!("Falha ao validar source para cleanup: {error}"))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.file_type().is_dir() {
                validate_cleanup_source(&entry.path())?;
                continue;
            }
            return Err(format!(
                "Arquivo não gerenciado impede cleanup: {}",
                entry.path().display()
            ));
        }
        Ok(())
    }

    fn remove_owned_staging(path: &Path) -> Result<(), String> {
        let root = OpenRoot::open(path)?;
        validate_tree_at(
            root.directory.as_raw_fd(),
            TreeLevel::OwnedStagingRoot,
            &mut |_| Ok(()),
        )?;
        remove_tree_contents_at(root.directory.as_raw_fd(), TreeLevel::OwnedStagingRoot)?;
        root.remove()
    }

    enum TreeLevel<'a> {
        ManagedRoot(&'a HashSet<OsString>),
        OwnedStagingRoot,
        LegacyRoot,
        Nested,
    }

    struct OpenRoot {
        parent: OwnedFd,
        directory: OwnedFd,
        name: CString,
        dev: libc::dev_t,
        ino: libc::ino_t,
    }

    impl OpenRoot {
        fn open(path: &Path) -> Result<Self, String> {
            let parent_path = path
                .parent()
                .ok_or_else(|| "Root de cleanup sem parent.".to_string())?;
            let name = path
                .file_name()
                .ok_or_else(|| "Root de cleanup sem nome.".to_string())?;
            let parent = open_directory_path(parent_path)?;
            let name = os_str_c_string(name)?;
            let listed = stat_at(parent.as_raw_fd(), &name)?
                .ok_or_else(|| format!("Root de cleanup ausente: {}", path.display()))?;
            if file_kind(&listed) != libc::S_IFDIR {
                return Err("Root de cleanup não é um diretório real.".to_string());
            }
            let directory = open_directory_at(parent.as_raw_fd(), &name)?;
            let opened = stat_fd(directory.as_raw_fd())?;
            if listed.st_dev != opened.st_dev || listed.st_ino != opened.st_ino {
                return Err("Root de cleanup mudou durante a abertura.".to_string());
            }
            Ok(Self {
                parent,
                directory,
                name,
                dev: opened.st_dev,
                ino: opened.st_ino,
            })
        }

        fn remove(self) -> Result<(), String> {
            let current = stat_at(self.parent.as_raw_fd(), &self.name)?
                .ok_or_else(|| "Root de cleanup desapareceu antes da remoção.".to_string())?;
            if file_kind(&current) != libc::S_IFDIR
                || current.st_dev != self.dev
                || current.st_ino != self.ino
            {
                return Err("Root de cleanup foi substituída antes da remoção.".to_string());
            }
            unlink_at(self.parent.as_raw_fd(), &self.name, libc::AT_REMOVEDIR)
        }
    }

    fn read_manifest_at(root_fd: RawFd) -> Result<MirrorManifest, String> {
        let name = CString::new(MANIFEST_NAME).expect("manifest name is static and NUL-free");
        let fd = open_file_at(root_fd, &name)?;
        let metadata = stat_fd(fd.as_raw_fd())?;
        if file_kind(&metadata) != libc::S_IFREG {
            return Err("O manifesto do mirror não é um arquivo regular.".to_string());
        }
        let mut bytes = Vec::new();
        let mut file = fs::File::from(fd);
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("Falha ao ler manifesto do mirror: {error}"))?;
        let manifest: MirrorManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Manifesto do mirror inválido: {error}"))?;
        validate_manifest(&manifest)?;
        Ok(manifest)
    }

    fn validate_tree_at<F>(
        directory_fd: RawFd,
        level: TreeLevel<'_>,
        before_directory_open: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&OsStr) -> Result<(), String>,
    {
        for name in directory_names(directory_fd)? {
            let c_name = os_str_c_string(&name)?;
            let Some(metadata) = stat_at(directory_fd, &c_name)? else {
                continue;
            };
            match file_kind(&metadata) {
                libc::S_IFLNK if !matches!(level, TreeLevel::ManagedRoot(_)) => {}
                libc::S_IFDIR if directory_allowed(&level, &name) => {
                    before_directory_open(&name)?;
                    let child = open_directory_at(directory_fd, &c_name)?;
                    ensure_same_entry(directory_fd, &c_name, child.as_raw_fd())?;
                    validate_tree_at(child.as_raw_fd(), TreeLevel::Nested, before_directory_open)?;
                }
                libc::S_IFREG if regular_allowed(&level, &name) => {}
                _ => {
                    return Err(format!(
                        "Entrada não gerenciada impede cleanup: {}",
                        name.to_string_lossy()
                    ))
                }
            }
        }
        Ok(())
    }

    fn remove_tree_contents_at(directory_fd: RawFd, level: TreeLevel<'_>) -> Result<(), String> {
        remove_tree_contents_at_with_hook(directory_fd, level, &mut |_| Ok(()))
    }

    fn remove_tree_contents_at_with_hook<F>(
        directory_fd: RawFd,
        level: TreeLevel<'_>,
        before_remove: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&OsStr) -> Result<(), String>,
    {
        let managed_root = matches!(&level, TreeLevel::ManagedRoot(_));
        for name in directory_names(directory_fd)? {
            if managed_root && name == OsStr::new(MANIFEST_NAME) {
                continue;
            }
            before_remove(&name)?;
            let c_name = os_str_c_string(&name)?;
            let Some(metadata) = stat_at(directory_fd, &c_name)? else {
                continue;
            };
            match file_kind(&metadata) {
                libc::S_IFLNK if !managed_root => {
                    unlink_at_if_kind(directory_fd, &c_name, libc::S_IFLNK, 0)?
                }
                libc::S_IFDIR if directory_allowed(&level, &name) => {
                    let child = open_directory_at(directory_fd, &c_name)?;
                    ensure_same_entry(directory_fd, &c_name, child.as_raw_fd())?;
                    remove_tree_contents_at_with_hook(
                        child.as_raw_fd(),
                        TreeLevel::Nested,
                        before_remove,
                    )?;
                    ensure_same_entry(directory_fd, &c_name, child.as_raw_fd())?;
                    unlink_at(directory_fd, &c_name, libc::AT_REMOVEDIR)?;
                }
                libc::S_IFREG if regular_allowed(&level, &name) => {
                    unlink_at_if_kind(directory_fd, &c_name, libc::S_IFREG, 0)?;
                }
                _ => {
                    return Err(format!(
                        "Cleanup recusou entrada inesperada: {}",
                        name.to_string_lossy()
                    ))
                }
            }
        }
        if managed_root {
            // Keep the ownership marker available for a retry after partial cleanup.
            // There is still a crash window between unlinking it and removing the root.
            let remaining = directory_names(directory_fd)?;
            if remaining != [OsString::from(MANIFEST_NAME)] {
                return Err(
                    "Cleanup managed encontrou entradas após remover os sources.".to_string(),
                );
            }
            before_remove(OsStr::new(MANIFEST_NAME))?;
            let manifest_name = os_str_c_string(OsStr::new(MANIFEST_NAME))?;
            if !stat_at(directory_fd, &manifest_name)?
                .is_some_and(|metadata| file_kind(&metadata) == libc::S_IFREG)
            {
                return Err("Manifesto mudou antes da remoção final.".to_string());
            }
            unlink_at_if_kind(directory_fd, &manifest_name, libc::S_IFREG, 0)?;
        }
        Ok(())
    }

    fn directory_allowed(level: &TreeLevel<'_>, name: &OsStr) -> bool {
        match level {
            TreeLevel::ManagedRoot(allowed) => allowed.contains(name),
            TreeLevel::OwnedStagingRoot | TreeLevel::Nested => true,
            TreeLevel::LegacyRoot => false,
        }
    }

    fn regular_allowed(level: &TreeLevel<'_>, name: &OsStr) -> bool {
        matches!(
            level,
            TreeLevel::ManagedRoot(_) | TreeLevel::OwnedStagingRoot
        ) && name == OsStr::new(MANIFEST_NAME)
    }

    fn open_directory_path(path: &Path) -> Result<OwnedFd, String> {
        // Settings' intent guard retains this descriptor for the whole transaction.
        // Duplicate that descriptor instead of following a procfs symlink or
        // reopening a possibly replaced configuration path. Ordinary paths below
        // still require O_NOFOLLOW, including all physical source paths.
        if path.parent() == Some(Path::new("/proc/self/fd")) {
            let descriptor = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.parse::<RawFd>().ok())
                .filter(|descriptor| *descriptor >= 0)
                .ok_or_else(|| "Descritor do parent ancorado inválido.".to_string())?;
            let duplicate = unsafe {
                // SAFETY: fcntl either duplicates a live descriptor or returns an error;
                // the caller retains its own descriptor and we own only the duplicate.
                libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 0)
            };
            let directory = owned_fd(duplicate, "Falha ao duplicar parent ancorado")?;
            if file_kind(&stat_fd(directory.as_raw_fd())?) != libc::S_IFDIR {
                return Err("Parent ancorado não é um diretório.".to_string());
            }
            return Ok(directory);
        }
        let path = path_c_string(path)?;
        let fd = unsafe {
            // SAFETY: path is a live NUL-terminated C string and flags require no mode argument.
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        owned_fd(fd, "Falha ao abrir parent do cleanup")
    }

    fn open_directory_at(parent_fd: RawFd, name: &CStr) -> Result<OwnedFd, String> {
        let fd = unsafe {
            // SAFETY: name is NUL-terminated and parent_fd remains open for the call.
            libc::openat(
                parent_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        owned_fd(fd, "Falha ao abrir diretório do cleanup sem seguir symlink")
    }

    fn open_file_at(parent_fd: RawFd, name: &CStr) -> Result<OwnedFd, String> {
        let fd = unsafe {
            // SAFETY: name is NUL-terminated and parent_fd remains open for the call.
            libc::openat(
                parent_fd,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        owned_fd(fd, "Falha ao abrir manifesto sem seguir symlink")
    }

    fn owned_fd(fd: RawFd, context: &str) -> Result<OwnedFd, String> {
        if fd < 0 {
            return Err(format!("{context}: {}", std::io::Error::last_os_error()));
        }
        Ok(unsafe {
            // SAFETY: a non-negative return from open/openat is a newly owned descriptor.
            OwnedFd::from_raw_fd(fd)
        })
    }

    fn stat_at(parent_fd: RawFd, name: &CStr) -> Result<Option<libc::stat>, String> {
        let mut metadata = MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            // SAFETY: metadata points to writable storage and name/parent_fd are valid for the call.
            libc::fstatat(
                parent_fd,
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            return Ok(Some(unsafe {
                // SAFETY: fstatat initialized metadata on success.
                metadata.assume_init()
            }));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == ErrorKind::NotFound {
            return Ok(None);
        }
        Err(format!("Falha ao inspecionar entrada do cleanup: {error}"))
    }

    fn stat_fd(fd: RawFd) -> Result<libc::stat, String> {
        let mut metadata = MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            // SAFETY: metadata points to writable storage and fd is open for the call.
            libc::fstat(fd, metadata.as_mut_ptr())
        };
        if result != 0 {
            return Err(format!(
                "Falha ao inspecionar diretório aberto: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe {
            // SAFETY: fstat initialized metadata on success.
            metadata.assume_init()
        })
    }

    fn ensure_same_entry(parent_fd: RawFd, name: &CStr, child_fd: RawFd) -> Result<(), String> {
        let current = stat_at(parent_fd, name)?
            .ok_or_else(|| "Diretório desapareceu durante cleanup.".to_string())?;
        let opened = stat_fd(child_fd)?;
        if file_kind(&current) != libc::S_IFDIR
            || current.st_dev != opened.st_dev
            || current.st_ino != opened.st_ino
        {
            return Err("Diretório foi substituído durante cleanup.".to_string());
        }
        Ok(())
    }

    fn directory_names(directory_fd: RawFd) -> Result<Vec<OsString>, String> {
        let duplicate = unsafe {
            // SAFETY: "." is NUL-terminated and directory_fd remains open for the call. Opening
            // it creates an independent directory cursor for fdopendir/readdir.
            libc::openat(
                directory_fd,
                c".".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if duplicate < 0 {
            return Err(format!(
                "Falha ao abrir cursor do dirfd do cleanup: {}",
                std::io::Error::last_os_error()
            ));
        }
        let stream = unsafe {
            // SAFETY: duplicate is owned here; fdopendir takes ownership on success.
            libc::fdopendir(duplicate)
        };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            unsafe {
                // SAFETY: fdopendir failed, so duplicate remains owned by this function.
                libc::close(duplicate);
            }
            return Err(format!("Falha ao listar dirfd do cleanup: {error}"));
        }

        let mut names = Vec::new();
        loop {
            unsafe {
                // SAFETY: Linux exposes thread-local errno through __errno_location.
                *libc::__errno_location() = 0;
            }
            let entry = unsafe {
                // SAFETY: stream is a valid DIR pointer until closedir below.
                libc::readdir(stream)
            };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(0) {
                    unsafe {
                        // SAFETY: stream is valid and closed exactly once.
                        libc::closedir(stream);
                    }
                    return Err(format!("Falha durante listagem do cleanup: {error}"));
                }
                break;
            }
            let name = unsafe {
                // SAFETY: d_name is NUL-terminated for a successful readdir result.
                CStr::from_ptr((*entry).d_name.as_ptr())
            }
            .to_bytes();
            if name != b"." && name != b".." {
                names.push(OsStr::from_bytes(name).to_os_string());
            }
        }
        let close_result = unsafe {
            // SAFETY: stream is valid and closed exactly once.
            libc::closedir(stream)
        };
        if close_result != 0 {
            return Err(format!(
                "Falha ao fechar listagem do cleanup: {}",
                std::io::Error::last_os_error()
            ));
        }
        names.sort();
        Ok(names)
    }

    fn unlink_at(parent_fd: RawFd, name: &CStr, flags: libc::c_int) -> Result<(), String> {
        let result = unsafe {
            // SAFETY: name is NUL-terminated and parent_fd remains open for the call.
            libc::unlinkat(parent_fd, name.as_ptr(), flags)
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == ErrorKind::NotFound {
            return Ok(());
        }
        Err(format!("Falha ao remover entrada do cleanup: {error}"))
    }

    fn unlink_at_if_kind(
        parent_fd: RawFd,
        name: &CStr,
        expected_kind: libc::mode_t,
        flags: libc::c_int,
    ) -> Result<(), String> {
        let Some(current) = stat_at(parent_fd, name)? else {
            return Ok(());
        };
        if file_kind(&current) != expected_kind {
            return Err("Entrada mudou de tipo antes da remoção.".to_string());
        }
        unlink_at(parent_fd, name, flags)
    }

    fn file_kind(metadata: &libc::stat) -> libc::mode_t {
        metadata.st_mode & libc::S_IFMT
    }

    fn os_str_c_string(value: &OsStr) -> Result<CString, String> {
        CString::new(value.as_bytes()).map_err(|_| "Nome de entrada contém NUL.".to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        struct Fixture {
            root: PathBuf,
        }

        impl Fixture {
            fn new(name: &str) -> Self {
                let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                let root = std::env::temp_dir().join(format!(
                    "sonante-media-1b-{}-{name}-{id}",
                    std::process::id()
                ));
                fs::create_dir(&root).unwrap();
                Self { root }
            }

            fn path(&self, value: &str) -> PathBuf {
                self.root.join(value)
            }

            fn build(&self, roots: &[PathBuf]) -> Result<MirrorBuildResult, String> {
                build_filtered_staging(
                    &self.path("library.next"),
                    &self.path("library"),
                    roots,
                    "test-generation",
                )
            }
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                if self.root.exists() {
                    fs::remove_dir_all(&self.root).unwrap();
                }
            }
        }

        fn write(path: &Path, bytes: &[u8]) {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }

        fn build_generation(
            fixture: &Fixture,
            name: &str,
            live: &Path,
            roots: &[PathBuf],
            generation_id: &str,
        ) -> PathBuf {
            let path = fixture.path(name);
            build_filtered_staging(&path, live, roots, generation_id).unwrap();
            path
        }

        fn legacy_generation(path: &Path, source_id: &str, root: &Path) {
            fs::create_dir(path).unwrap();
            symlink(root, path.join(source_id)).unwrap();
        }

        fn expect_prepared(
            result: Result<ManagedGenerationPreparation, String>,
        ) -> MirrorBuildResult {
            match result.unwrap() {
                ManagedGenerationPreparation::Prepared(prepared) => prepared,
                ManagedGenerationPreparation::Unchanged => {
                    panic!("a preparação deveria produzir uma nova geração")
                }
            }
        }

        #[test]
        fn managed_publication_validation_keeps_both_generations_untouched() {
            let fixture = Fixture::new("validate-phases");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            write(&music.join("new.flac"), b"new");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let live_identity = directory_identity(&live, "live").unwrap();
            let staging_identity = directory_identity(&staging, "staging").unwrap();

            let validated = validate_prepared_managed_publication(&live, &prepared).unwrap();

            assert_eq!(validated.published, staging_identity);
            assert_eq!(
                validated.previous,
                Some((live_identity, ManagedMirrorPreviousKind::Managed))
            );
            assert_eq!(directory_identity(&live, "live").unwrap(), live_identity);
            assert_eq!(directory_identity(&staging, "staging").unwrap(), staging_identity);
        }

        #[test]
        fn managed_publication_validation_failure_never_swaps_generations() {
            let fixture = Fixture::new("validate-missing-target");
            let music = fixture.path("Music");
            let track = music.join("track.flac");
            write(&track, b"track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-first",
            ));
            let staging_identity = directory_identity(&staging, "staging").unwrap();
            fs::remove_file(track).unwrap();

            assert!(validate_prepared_managed_publication(&live, &prepared).is_err());
            assert!(!live.exists());
            assert_eq!(directory_identity(&staging, "staging").unwrap(), staging_identity);
        }

        #[test]
        fn validated_publication_rechecks_identity_before_atomic_exchange() {
            let fixture = Fixture::new("publish-validated-identity");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            write(&music.join("new.flac"), b"new");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let validated = validate_prepared_managed_publication(&live, &prepared).unwrap();
            let live_identity = directory_identity(&live, "live").unwrap();
            let replaced_staging = fixture.path("replaced-staging");
            fs::rename(&staging, &replaced_staging).unwrap();
            fs::create_dir(&staging).unwrap();

            assert!(publish_validated_managed_generation(validated).is_err());
            assert_eq!(directory_identity(&live, "live").unwrap(), live_identity);
            assert!(is_managed_generation(&replaced_staging));
            assert!(fs::read_dir(&staging).unwrap().next().is_none());
        }

        #[test]
        fn validated_publication_rejects_replaced_live_generation() {
            let fixture = Fixture::new("publish-validated-live-identity");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            write(&music.join("new.flac"), b"new");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let validated = validate_prepared_managed_publication(&live, &prepared).unwrap();
            let staging_identity = directory_identity(&staging, "staging").unwrap();
            let replaced_live = fixture.path("replaced-live");
            fs::rename(&live, &replaced_live).unwrap();
            fs::create_dir(&live).unwrap();

            assert!(publish_validated_managed_generation(validated).is_err());
            assert_eq!(directory_identity(&staging, "staging").unwrap(), staging_identity);
            assert!(is_managed_generation(&replaced_live));
            assert!(fs::read_dir(&live).unwrap().next().is_none());
        }

        #[test]
        fn validated_first_publication_is_atomic_and_rollback_restores_staging() {
            let fixture = Fixture::new("publish-validated-first");
            let music = fixture.path("Music");
            let track = music.join("track.flac");
            write(&track, b"physical-track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-first",
            ));
            let validated = validate_prepared_managed_publication(&live, &prepared).unwrap();
            let staged_identity = directory_identity(&staging, "staging").unwrap();

            let publication = publish_validated_managed_generation(validated).unwrap();
            assert_eq!(publication.kind(), ManagedMirrorPublicationKind::FirstPublication);
            assert_eq!(directory_identity(&live, "live").unwrap(), staged_identity);
            assert!(!staging.exists());

            publication.rollback().unwrap();
            assert!(!live.exists());
            assert_eq!(directory_identity(&staging, "staging").unwrap(), staged_identity);
            assert_eq!(fs::read(track).unwrap(), b"physical-track");
        }

        #[test]
        fn dormant_publication_exchanges_legacy_and_rolls_back_exactly() {
            let fixture = Fixture::new("publish-legacy");
            let music = fixture.path("Music");
            write(&music.join("Album/track.flac"), b"track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            legacy_generation(&live, "Existing ID", &music);
            let legacy_identity = directory_identity(&live, "legacy").unwrap();
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let managed_identity = directory_identity(&staging, "managed").unwrap();

            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();

            assert_eq!(
                publication.kind(),
                ManagedMirrorPublicationKind::ReplacedExisting
            );
            assert_eq!(
                publication.previous_kind(),
                Some(ManagedMirrorPreviousKind::Legacy)
            );
            assert_eq!(directory_identity(&live, "live").unwrap(), managed_identity);
            assert_eq!(
                directory_identity(&staging, "previous").unwrap(),
                legacy_identity
            );
            assert!(is_managed_generation(&live));
            assert!(fs::symlink_metadata(staging.join("Existing ID"))
                .unwrap()
                .file_type()
                .is_symlink());

            publication.rollback().unwrap();

            assert_eq!(directory_identity(&live, "legacy").unwrap(), legacy_identity);
            assert_eq!(
                directory_identity(&staging, "managed").unwrap(),
                managed_identity
            );
            assert!(fs::symlink_metadata(live.join("Existing ID"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(is_managed_generation(&staging));
        }

        #[test]
        fn committed_publication_cleans_legacy_tree_without_touching_target() {
            let fixture = Fixture::new("commit-cleanup-legacy");
            let music = fixture.path("Music");
            let track = music.join("Album/track.flac");
            write(&track, b"physical-track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            legacy_generation(&live, "Existing ID", &music);
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();

            publication.cleanup_previous().unwrap();

            assert!(is_managed_generation(&live));
            assert!(!staging.exists());
            assert_eq!(fs::read(track).unwrap(), b"physical-track");
        }

        #[test]
        fn rolled_back_publication_cleans_rejected_managed_generation() {
            let fixture = Fixture::new("rollback-cleanup-managed");
            let music = fixture.path("Music");
            let track = music.join("Album/track.flac");
            write(&track, b"physical-track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            legacy_generation(&live, "Existing ID", &music);
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();

            publication.rollback().unwrap();
            publication.cleanup_rejected_after_rollback().unwrap();

            assert!(fs::symlink_metadata(live.join("Existing ID"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(!staging.exists());
            assert_eq!(fs::read(track).unwrap(), b"physical-track");
        }

        #[test]
        fn dormant_publication_exchanges_managed_generations_and_rolls_back_exactly() {
            let fixture = Fixture::new("publish-managed");
            let music = fixture.path("Music");
            write(&music.join("Album/old.flac"), b"old");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            write(&music.join("Album/new.flac"), b"new");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let old_identity = directory_identity(&live, "old").unwrap();
            let new_identity = directory_identity(&staging, "new").unwrap();

            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();

            assert_eq!(
                publication.previous_kind(),
                Some(ManagedMirrorPreviousKind::Managed)
            );
            assert!(live.join("Music/Album/new.flac").exists());
            assert!(!staging.join("Music/Album/new.flac").exists());
            assert_eq!(directory_identity(&live, "new").unwrap(), new_identity);
            assert_eq!(directory_identity(&staging, "old").unwrap(), old_identity);

            publication.rollback().unwrap();

            assert!(!live.join("Music/Album/new.flac").exists());
            assert!(staging.join("Music/Album/new.flac").exists());
            assert_eq!(directory_identity(&live, "old").unwrap(), old_identity);
            assert_eq!(directory_identity(&staging, "new").unwrap(), new_identity);
        }

        #[test]
        fn dormant_first_publication_uses_atomic_inverse_for_rollback() {
            let fixture = Fixture::new("publish-first");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-first",
            ));
            let managed_identity = directory_identity(&staging, "managed").unwrap();

            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();

            assert_eq!(
                publication.kind(),
                ManagedMirrorPublicationKind::FirstPublication
            );
            assert!(!staging.exists());
            assert_eq!(directory_identity(&live, "live").unwrap(), managed_identity);

            fs::create_dir(&staging).unwrap();
            write(&staging.join("collision.txt"), b"do-not-replace");
            assert!(publication.rollback().is_err());
            assert_eq!(directory_identity(&live, "live").unwrap(), managed_identity);
            assert_eq!(
                fs::read(staging.join("collision.txt")).unwrap(),
                b"do-not-replace"
            );
            fs::remove_file(staging.join("collision.txt")).unwrap();
            fs::remove_dir(&staging).unwrap();
            publication.rollback().unwrap();

            assert!(!live.exists());
            assert_eq!(
                directory_identity(&staging, "staging").unwrap(),
                managed_identity
            );
            assert!(is_managed_generation(&staging));
        }

        #[test]
        fn dormant_publication_failure_preserves_live_and_staging() {
            let fixture = Fixture::new("publish-failure");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            let staging_identity = directory_identity(&staging, "staging").unwrap();
            write(&live.join("unexpected.txt"), b"keep-live");
            let live_identity = directory_identity(&live, "live").unwrap();

            assert!(publish_prepared_managed_generation(&live, &prepared).is_err());

            assert_eq!(fs::read(live.join("unexpected.txt")).unwrap(), b"keep-live");
            assert_eq!(directory_identity(&live, "live").unwrap(), live_identity);
            assert_eq!(
                directory_identity(&staging, "staging").unwrap(),
                staging_identity
            );
            assert!(is_managed_generation(&staging));
        }

        #[test]
        fn dormant_publication_rejects_invalid_managed_generation() {
            let fixture = Fixture::new("publish-invalid");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            write(&staging.join("unexpected.txt"), b"invalid");
            let staging_identity = directory_identity(&staging, "staging").unwrap();

            assert!(publish_prepared_managed_generation(&live, &prepared).is_err());

            assert!(!live.exists());
            assert_eq!(
                directory_identity(&staging, "staging").unwrap(),
                staging_identity
            );
            assert_eq!(
                fs::read(staging.join("unexpected.txt")).unwrap(),
                b"invalid"
            );
        }

        #[test]
        fn dormant_publication_and_rollback_never_modify_physical_targets() {
            let fixture = Fixture::new("publish-targets");
            let music = fixture.path("Music");
            let physical = music.join("Album/track.flac");
            write(&physical, b"physical-audio-bytes");
            let physical_metadata = fs::metadata(&physical).unwrap();
            let physical_identity = (physical_metadata.dev(), physical_metadata.ino());
            let live = fixture.path("library");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-first",
            ));

            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();
            publication.rollback().unwrap();

            assert_eq!(fs::read(&physical).unwrap(), b"physical-audio-bytes");
            let after = fs::metadata(&physical).unwrap();
            assert_eq!((after.dev(), after.ino()), physical_identity);
            assert_eq!(
                fs::read_link(staging.join("Music/Album/track.flac")).unwrap(),
                fs::canonicalize(&physical).unwrap()
            );
        }


        fn namespace_migration_fixture() -> (Fixture, PathBuf, PathBuf, MirrorBuildResult) {
            let fixture = Fixture::new("metadata-only-upgrade");
            let music = fixture.path("Music");
            let live = fixture.path("library");
            let next = fixture.path("library.next");
            write(&music.join("Album/track.flac"), b"fixture");
            fs::create_dir_all(live.join("Existing/Album")).unwrap();
            symlink(
                music.join("Album/track.flac"),
                live.join("Existing/Album/track.flac"),
            )
            .unwrap();
            write_manifest(
                &live,
                &MirrorManifest {
                    schema_version: 1,
                    generation_id: "old-v1".into(),
                    registry_id: None,
                    sources: BTreeMap::from([("Existing".into(), music.clone())]),
                },
            )
            .unwrap();
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &next,
                &[music.clone()],
                "identity-upgrade",
            ));
            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::MetadataOnly);
            (fixture, music, live, prepared)
        }
        #[test]
        fn metadata_migration_is_confirmed_once_without_mpd_and_real_change_still_refreshes() {
            let (fixture, music, live, prepared) = namespace_migration_fixture();
            assert_eq!(
                crate::library_refresh::refresh(
                    "never-open-a-socket",
                    &prepared.refresh_plan,
                    true,
                    || false
                )
                .unwrap(),
                crate::library_refresh::Outcome::Completed
            );
            let publication = publish_prepared_managed_generation(&live, &prepared).unwrap();
            publication.cleanup_previous_durable().unwrap();
            assert_eq!(read_manifest(&live).unwrap().schema_version, 2);
            assert_eq!(
                prepare_managed_generation(
                    &live,
                    &fixture.path("library.next"),
                    &[music.clone()],
                    "again"
                )
                .unwrap(),
                ManagedGenerationPreparation::Unchanged
            );
            write(&music.join("Album/added.flac"), b"added");
            let changed = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[music],
                "media-changed",
            ));
            assert_ne!(changed.refresh_plan, ManagedRefreshPlan::MetadataOnly);
        }

        #[test]
        fn explicit_rescan_keeps_a_generation_until_audit_or_cancel_even_when_equivalent() {
            let (fixture, music, live, prepared) = namespace_migration_fixture();
            publish_prepared_managed_generation(&live, &prepared)
                .unwrap()
                .cleanup_previous_durable()
                .unwrap();
            let forced = expect_prepared(prepare_managed_generation_for_refresh(
                &live,
                &fixture.path("library.next"),
                &[music],
                "manual-rescan",
                true,
            ));
            assert_eq!(forced.refresh_plan, ManagedRefreshPlan::Full);
            let publication = publish_prepared_managed_generation(&live, &forced).unwrap();
            let result =
                crate::library_refresh::finish(Ok(crate::library_refresh::Outcome::Cancelled), |_| {
                    panic!("no implicit recovery")
                });
            assert!(result.is_err());
            assert!(fixture.path("library.next").is_dir());
            assert!(crate::managed_save_intent::check_startup(&fixture.root).is_err());
            // Only the fixture explicitly proves/rolls back its own publication.
            publication.rollback().unwrap();
            publication.cleanup_rejected_after_rollback().unwrap();
        }
        #[test]
        fn metadata_only_flag_cannot_hide_a_change_after_the_publication_guard() {
            let (_fixture, music, live, prepared) = namespace_migration_fixture();
            let before = fs::read(live.join(MANIFEST_NAME)).unwrap();
            let result = publish_prepared_managed_generation_guarded(&live, &prepared, || {
                write(&music.join("Album/new.flac"), b"new");
                symlink(
                    music.join("Album/new.flac"),
                    prepared.staging_path.join("Existing/Album/new.flac"),
                )
                .unwrap();
                Ok(())
            });
            assert!(result.is_err());
            assert_eq!(fs::read(live.join(MANIFEST_NAME)).unwrap(), before);
            assert!(prepared.staging_path.is_dir());
        }

        #[test]
        fn metadata_manifest_changed_by_guard_is_not_published_as_the_synced_manifest() {
            let (_fixture,_music,live,prepared)=namespace_migration_fixture();
            let result=publish_prepared_managed_generation_guarded(&live,&prepared,||{
                let mut changed=read_manifest(&prepared.staging_path)?;
                changed.generation_id="different-attempt".into();write_manifest(&prepared.staging_path,&changed)
            });
            assert!(result.is_err());assert_eq!(read_manifest(&live).unwrap().schema_version,1);
        }
        #[test]
        fn metadata_parent_replacement_during_fsync_cannot_claim_durable_confirmation() {
            let (fixture,_music,live,prepared)=namespace_migration_fixture();
            let moved=fixture.root.with_extension("moved");assert!(!moved.exists());
            let result=publish_prepared_with_sync(&live,&prepared,||Ok(()),|_|{
                fs::rename(&fixture.root,&moved).unwrap();fs::create_dir(&fixture.root).unwrap();Ok(())
            });
            assert!(result.unwrap_err().starts_with(PUBLICATION_UNCONFIRMED));
            fs::remove_dir(&fixture.root).unwrap();fs::rename(&moved,&fixture.root).unwrap();
            assert_eq!(read_manifest(&live).unwrap().schema_version,2);assert_eq!(read_manifest(&prepared.staging_path).unwrap().schema_version,1);
        }

        #[test]
        fn metadata_migration_faults_before_and_after_exchange_preserve_identity_history() {
            for after_exchange in [false, true] {
                let (fixture, _music, live, prepared) = namespace_migration_fixture();
                let ledger = fixture.path("local-source-identities.json");
                let history = fs::read(&ledger).unwrap();
                let result = publish_prepared_with_sync(
                    &live,
                    &prepared,
                    || {
                        if after_exchange {
                            Ok(())
                        } else {
                            Err("injected before exchange".into())
                        }
                    },
                    |_| Err("injected parent fsync".into()),
                );
                let error = result.unwrap_err();
                assert_eq!(error.starts_with(PUBLICATION_UNCONFIRMED), after_exchange);
                assert_eq!(
                    read_manifest(&live).unwrap().schema_version,
                    if after_exchange { 2 } else { 1 }
                );
                assert_eq!(
                    read_manifest(&prepared.staging_path)
                        .unwrap()
                        .schema_version,
                    if after_exchange { 1 } else { 2 }
                );
                assert_eq!(fs::read(ledger).unwrap(), history);
                if after_exchange {
                    assert!(crate::managed_save_intent::check_startup(&fixture.root).is_err());
                }
            }
        }
        #[test]
        fn metadata_cleanup_failure_does_not_rollback_confirmed_generation_or_release_ids() {
            let (fixture, _music, live, prepared) = namespace_migration_fixture();
            let history = fs::read(fixture.path("local-source-identities.json")).unwrap();
            let publication = publish_prepared_managed_generation(&live, &prepared).unwrap();
            write(&prepared.staging_path.join("foreign-file"), b"keep");
            assert!(publication.cleanup_previous_durable().is_err());
            assert_eq!(read_manifest(&live).unwrap().schema_version, 2);
            assert!(prepared.staging_path.join("foreign-file").exists());
            assert_eq!(
                fs::read(fixture.path("local-source-identities.json")).unwrap(),
                history
            );
        }

        #[test]
        fn managed_generation_preparation_discards_equivalent_staging() {
            let fixture = Fixture::new("prepare-equivalent");
            let music = fixture.path("Music");
            write(&music.join("Album/track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "live-generation",
            );
            let before = managed_generation_snapshot(&live).unwrap();
            let staging = fixture.path("library.next");

            let result = prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "next-generation",
            )
            .unwrap();

            assert_eq!(result, ManagedGenerationPreparation::Unchanged);
            assert!(!staging.exists());
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
        }

        #[test]
        fn managed_generation_preparation_keeps_different_staging() {
            let fixture = Fixture::new("prepare-different");
            let music = fixture.path("Music");
            write(&music.join("Album/track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "live-generation",
            );
            write(&music.join("Album/new.dsf"), b"new");
            let before = managed_generation_snapshot(&live).unwrap();
            let staging = fixture.path("library.next");

            let result = prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "next-generation",
            )
            .unwrap();

            let ManagedGenerationPreparation::Prepared(prepared) = result else {
                panic!("a geração diferente deveria permanecer preparada");
            };
            assert_eq!(prepared.staging_path, staging);
            assert_eq!(
                prepared.refresh_plan,
                ManagedRefreshPlan::Targeted {
                    directories: vec!["Music/Album".to_string()],
                    changed_files: 1,
                }
            );
            assert_eq!(
                compare_managed_generations(&live, &prepared.staging_path).unwrap(),
                MirrorComparison::Different
            );
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
        }

        #[test]
        fn settings_save_targets_only_new_managed_sources() {
            let fixture = Fixture::new("save-added-sources");
            let old = fixture.path("Old");
            let added_one = fixture.path("Added One");
            let added_two = fixture.path("Added \"Two\"");
            write(&old.join("Album/old.flac"), b"old");
            write(&added_one.join("Album/one.flac"), b"one");
            write(&added_two.join("Album/two.flac"), b"two");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&old),
                "managed-live",
            );
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[old, added_one, added_two],
                "managed-next",
            ));
            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::Full);
            assert_eq!(
                plan_added_sources_refresh_diagnostic(&live, &prepared).unwrap(),
                (
                    Some(ManagedRefreshPlan::Targeted {
                        directories: vec!["Added \"Two\"".to_string(), "Added One".to_string()],
                        changed_files: 2,
                    }),
                    "none",
                )
            );
        }

        #[test]
        fn settings_save_falls_back_for_removal_legacy_and_old_source_changes() {
            let fixture = Fixture::new("save-fallbacks");
            let old = fixture.path("Old");
            let added = fixture.path("Added");
            write(&old.join("Album/old.flac"), b"old");
            write(&added.join("Album/new.flac"), b"new");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                &[old.clone(), added.clone()],
                "managed-live",
            );
            let removal = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("remove.next"),
                std::slice::from_ref(&old),
                "remove",
            ));
            assert_eq!(plan_added_sources_refresh(&live, &removal).unwrap(), None);
            assert_eq!(plan_added_sources_refresh_diagnostic(&live, &removal).unwrap().1, "previous_mapping_changed");

            let legacy = fixture.path("legacy");
            legacy_generation(&legacy, "Old", &old);
            let legacy_next = expect_prepared(prepare_managed_generation(
                &legacy,
                &fixture.path("legacy.next"),
                &[old.clone(), added.clone()],
                "legacy-next",
            ));
            assert_eq!(plan_added_sources_refresh(&legacy, &legacy_next).unwrap(), None);
            assert_eq!(plan_added_sources_refresh_diagnostic(&legacy, &legacy_next).unwrap().1, "legacy");

            let replacement = fixture.path("Other/Old");
            write(&replacement.join("Album/replacement.flac"), b"replacement");
            let changed_mapping = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("mapping.next"),
                &[added.clone(), replacement],
                "mapping-next",
            ));
            assert_eq!(plan_added_sources_refresh(&live, &changed_mapping).unwrap(), None);
            assert_eq!(plan_added_sources_refresh_diagnostic(&live, &changed_mapping).unwrap().1, "previous_mapping_changed");

            let clean_source = fixture.path("Clean Source");
            write(&clean_source.join("Album/track.flac"), b"track");
            write(&old.join("Album/another.flac"), b"another");
            let changed_old = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("changed.next"),
                &[old, added, clean_source],
                "changed-next",
            ));
            assert_eq!(plan_added_sources_refresh(&live, &changed_old).unwrap(), None);
            assert_eq!(plan_added_sources_refresh_diagnostic(&live, &changed_old).unwrap().1, "entry_outside_added_sources");
        }

        #[test]
        fn settings_save_pure_removal_keeps_cue_entries_and_rejects_mixed_changes() {
            let fixture = Fixture::new("save-pure-removal");
            let kept = fixture.path("Mantida Á");
            let removed = fixture.path("Removida B");
            write(&kept.join("Álbum/image.flac"), b"kept");
            write(&kept.join("Álbum/Imagem.CUE"),
                b"FILE \"image.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n");
            write(&removed.join("old.flac"), b"removed");
            let live = build_generation(&fixture, "library", &fixture.path("missing"),
                &[kept.clone(), removed.clone()], "before");
            let prepared = expect_prepared(prepare_managed_generation(
                &live, &fixture.path("library.next"), std::slice::from_ref(&kept), "after",
            ));
            let plan = plan_removed_sources_refresh(&live, &prepared).unwrap().unwrap();
            assert_eq!(plan.removed_ids, ["Removida B"]);
            assert_eq!(plan.remaining_ids, ["Mantida Á"]);
            assert!(matches!(&plan.refresh_plan, ManagedRefreshPlan::Targeted { directories, .. }
                if directories == &vec!["Removida B".to_string()]));
            let original = fs::read(kept.join("Álbum/image.flac")).unwrap();
            let publication = publish_targeted_removed_sources_generation_guarded(
                &live, &prepared, &plan, || Ok(()),
            ).unwrap();
            assert!(!live.join("Removida B").exists());
            assert!(live.join("Mantida Á/Álbum/Imagem.CUE").exists());
            assert!(prepared.staging_path.join("Removida B").exists());
            publication.rollback().unwrap();
            publication.cleanup_rejected_after_rollback().unwrap();
            assert_eq!(fs::read(kept.join("Álbum/image.flac")).unwrap(), original);
            assert_eq!(fs::read(removed.join("old.flac")).unwrap(), b"removed");

            // Removal plus any surviving-entry change must retain full refresh.
            write(&kept.join("new.flac"), b"new");
            let mixed = expect_prepared(prepare_managed_generation(
                &live, &fixture.path("mixed.next"), std::slice::from_ref(&kept), "mixed",
            ));
            assert_eq!(plan_removed_sources_refresh(&live, &mixed).unwrap(), None);
            cleanup_prepared_managed_generation(&mixed).unwrap();
            fs::remove_file(kept.join("new.flac")).unwrap();

            // Removal plus an added/replaced source is not a pure deletion.
            let other = fixture.path("Other");
            write(&other.join("new.flac"), b"other");
            let mixed = expect_prepared(prepare_managed_generation(
                &live, &fixture.path("mixed.next"), &[kept.clone(), other], "mixed-added",
            ));
            assert_eq!(plan_removed_sources_refresh(&live, &mixed).unwrap(), None);
            cleanup_prepared_managed_generation(&mixed).unwrap();
            let legacy = fixture.path("legacy");
            legacy_generation(&legacy, "Mantida Á", &kept);
            assert_eq!(plan_removed_sources_refresh(&legacy, &prepared).unwrap(), None);
        }

        #[test]
        fn settings_save_removal_rechecks_surviving_entries_before_exchange() {
            let fixture = Fixture::new("save-removal-recheck");
            let kept = fixture.path("Keep");
            let removed = fixture.path("Remove");
            write(&kept.join("one.flac"), b"kept");
            write(&removed.join("one.flac"), b"removed");
            let live = build_generation(&fixture, "library", &fixture.path("missing"),
                &[kept.clone(), removed], "before");
            let prepared = expect_prepared(prepare_managed_generation(
                &live, &fixture.path("library.next"), &[kept.clone()], "after",
            ));
            let plan = plan_removed_sources_refresh(&live, &prepared).unwrap().unwrap();
            write(&kept.join("two.flac"), b"changed");
            symlink(kept.join("two.flac"), prepared.staging_path.join("Keep/two.flac")).unwrap();
            assert_eq!(publish_targeted_removed_sources_generation_guarded(
                &live, &prepared, &plan, || panic!("changed plan must not reach exchange"),
            ).unwrap_err(), "LOCAL_SOURCE_REMOVAL_PLAN_CHANGED");
            assert!(live.join("Remove/one.flac").exists());
            cleanup_prepared_managed_generation(&prepared).unwrap();
        }

        #[test]
        fn settings_save_refuses_invalid_new_managed_path() {
            let fixture = Fixture::new("save-invalid-staging");
            let old = fixture.path("Old");
            let added = fixture.path("Added");
            let outside = fixture.path("Outside");
            write(&old.join("old.flac"), b"old");
            write(&added.join("new.flac"), b"new");
            write(&outside.join("other.flac"), b"outside");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&old),
                "managed-live",
            );
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[old, added],
                "managed-next",
            ));
            let link = prepared.staging_path.join("Added/new.flac");
            fs::remove_file(&link).unwrap();
            symlink(outside.join("other.flac"), &link).unwrap();
            assert!(plan_added_sources_refresh(&live, &prepared).is_err());
        }

        #[test]
        fn targeted_addition_rechecks_managed_topology_before_exchange() {
            let fixture = Fixture::new("targeted-topology-changed");
            let old = fixture.path("Old");
            let added = fixture.path("Added");
            write(&old.join("old.flac"), b"old");
            write(&added.join("new.flac"), b"new");
            let live = build_generation(
                &fixture, "library", &fixture.path("missing-live"),
                std::slice::from_ref(&old), "managed-live",
            );
            let prepared = expect_prepared(prepare_managed_generation(
                &live, &fixture.path("library.next"), &[old.clone(), added], "next",
            ));
            let (Some(plan), _) = plan_added_sources_refresh_diagnostic(&live, &prepared).unwrap() else {
                panic!("a adição deveria permitir update direcionado");
            };
            fs::rename(&live, fixture.path("old-managed")).unwrap();
            legacy_generation(&live, "Old", &old);
            assert_eq!(
                publish_targeted_added_sources_generation(&live, &prepared, &plan).unwrap_err(),
                TARGETED_ADDITION_PLAN_CHANGED,
            );
            assert!(live.join("Old").is_symlink());
            assert!(prepared.staging_path.exists());
        }

        #[test]
        fn targeted_addition_rechecks_plan_before_exchange() {
            let fixture = Fixture::new("targeted-plan-changed");
            let old = fixture.path("Old");
            let added = fixture.path("Added");
            write(&old.join("old.flac"), b"old");
            write(&added.join("new.flac"), b"new");
            let live = build_generation(
                &fixture, "library", &fixture.path("missing-live"),
                std::slice::from_ref(&old), "managed-live",
            );
            let prepared = expect_prepared(prepare_managed_generation(
                &live, &fixture.path("library.next"), &[old.clone(), added], "next",
            ));
            let (Some(plan), _) = plan_added_sources_refresh_diagnostic(&live, &prepared).unwrap() else {
                panic!("a adição deveria permitir update direcionado");
            };
            write(&old.join("late.flac"), b"late");
            symlink(old.join("late.flac"), live.join("Old/late.flac")).unwrap();
            assert_eq!(
                publish_targeted_added_sources_generation(&live, &prepared, &plan).unwrap_err(),
                TARGETED_ADDITION_PLAN_CHANGED,
            );
            assert!(!live.join("Added").exists());
            assert!(prepared.staging_path.exists());
        }

        #[test]
        fn cleanup_refuses_staging_from_another_attempt() {
            let fixture = Fixture::new("staging-owned-by-attempt");
            let source = fixture.path("Source");
            write(&source.join("track.flac"), b"track");
            let prepared = expect_prepared(prepare_managed_generation(
                &fixture.path("library"),
                &fixture.path("library.next"),
                std::slice::from_ref(&source),
                "first-attempt",
            ));
            fs::rename(&prepared.staging_path, fixture.path("first-staging")).unwrap();
            build_generation(
                &fixture,
                "library.next",
                &fixture.path("library"),
                &[source],
                "second-attempt",
            );
            assert!(cleanup_prepared_managed_generation(&prepared).is_err());
            assert!(prepared.staging_path.exists());
            assert!(fixture.path("first-staging").exists());
        }

        #[test]
        fn removed_physical_files_are_replaced_as_stale_previous_entries() {
            let fixture = Fixture::new("prepare-removed-targets");
            let music = fixture.path("Music");
            let survivor = music.join("Album/keep.flac");
            let artwork = music.join("Album/cover.jpg");
            let ignored = music.join("Album/notes.prg");
            let removed = [
                music.join("Album/removed-a.flac"),
                music.join("Album/removed-b.mp3"),
                music.join("Album/removed-c.dsf"),
            ];
            write(&survivor, b"keep-audio");
            write(&artwork, b"keep-artwork");
            write(&ignored, b"keep-ignored-physical");
            for (index, path) in removed.iter().enumerate() {
                write(path, format!("removed-{index}").as_bytes());
            }
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            for path in &removed {
                fs::remove_file(path).unwrap();
            }
            let staging = fixture.path("library.next");

            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            assert_eq!(
                prepared.refresh_plan,
                ManagedRefreshPlan::Targeted {
                    directories: vec!["Music/Album".to_string()],
                    changed_files: 3,
                }
            );
            let publication =
                publish_prepared_managed_generation(&live, &prepared).unwrap();

            for path in &removed {
                let relative = path.strip_prefix(&music).unwrap();
                assert!(fs::symlink_metadata(live.join("Music").join(relative)).is_err());
            }
            assert_eq!(fs::read(&survivor).unwrap(), b"keep-audio");
            assert_eq!(fs::read(&artwork).unwrap(), b"keep-artwork");
            assert_eq!(fs::read(&ignored).unwrap(), b"keep-ignored-physical");

            publication.cleanup_previous().unwrap();

            assert!(!staging.exists());
            assert_eq!(fs::read(&survivor).unwrap(), b"keep-audio");
            assert_eq!(fs::read(&artwork).unwrap(), b"keep-artwork");
            assert_eq!(fs::read(&ignored).unwrap(), b"keep-ignored-physical");
            assert!(removed.iter().all(|path| !path.exists()));
        }

        #[test]
        fn missing_previous_target_outside_source_root_remains_rejected() {
            let fixture = Fixture::new("prepare-missing-escape");
            let music = fixture.path("Music");
            let outside = fixture.path("Outside");
            let track = music.join("track.flac");
            write(&track, b"track");
            fs::create_dir(&outside).unwrap();
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            let live_link = live.join("Music/track.flac");
            fs::remove_file(&live_link).unwrap();
            let unsafe_target = outside.join("missing.flac");
            symlink(&unsafe_target, &live_link).unwrap();
            let staging = fixture.path("library.next");

            assert!(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            )
            .is_err());

            assert!(!staging.exists());
            assert_eq!(fs::read_link(live_link).unwrap(), unsafe_target);
            assert_eq!(fs::read(track).unwrap(), b"track");
        }

        #[test]
        fn newly_prepared_generation_still_rejects_a_missing_target() {
            let fixture = Fixture::new("publish-new-missing-target");
            let music = fixture.path("Music");
            let track = music.join("track.flac");
            write(&track, b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            write(&music.join("new.flac"), b"new");
            let staging = fixture.path("library.next");
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));
            fs::remove_file(music.join("new.flac")).unwrap();
            let live_identity = directory_identity(&live, "live").unwrap();
            let staging_identity = directory_identity(&staging, "staging").unwrap();

            assert!(publish_prepared_managed_generation(&live, &prepared).is_err());

            assert_eq!(directory_identity(&live, "live").unwrap(), live_identity);
            assert_eq!(
                directory_identity(&staging, "staging").unwrap(),
                staging_identity
            );
            assert_eq!(fs::read(track).unwrap(), b"track");
        }

        #[test]
        fn managed_generation_preparation_keeps_staging_for_legacy_live() {
            let fixture = Fixture::new("prepare-legacy");
            let music = fixture.path("Music");
            write(&music.join("Album/track.flac"), b"track");
            let live = fixture.path("library");
            legacy_generation(&live, "Existing ID", &music);
            let legacy_target = fs::read_link(live.join("Existing ID")).unwrap();
            let staging = fixture.path("library.next");

            let result = prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "next-generation",
            )
            .unwrap();

            let ManagedGenerationPreparation::Prepared(prepared) = result else {
                panic!("a topologia legacy deveria produzir staging preparada");
            };
            assert_eq!(prepared.staging_path, staging);
            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::Full);
            assert!(is_managed_generation(&staging));
            assert_eq!(fs::read_link(live.join("Existing ID")).unwrap(), legacy_target);
        }

        #[test]
        fn cue_change_forces_full_refresh_fallback() {
            let fixture = Fixture::new("prepare-cue-fallback");
            let music = fixture.path("Music");
            write(&music.join("Album/track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            write(
                &music.join("Album/image.cue"),
                b"FILE \"track.flac\" WAVE\nTRACK 01 AUDIO\n",
            );
            let staging = fixture.path("library.next");

            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));

            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::Full);
            cleanup_managed_generation(&staging).unwrap();
        }

        #[test]
        fn broad_change_forces_full_refresh_fallback() {
            let fixture = Fixture::new("prepare-broad-fallback");
            let music = fixture.path("Music");
            write(&music.join("Album/existing.flac"), b"existing");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            for index in 0..=MAX_TARGETED_CHANGED_FILES {
                write(
                    &music.join(format!("Album/new-{index}.flac")),
                    b"new",
                );
            }
            let staging = fixture.path("library.next");

            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));

            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::Full);
            cleanup_managed_generation(&staging).unwrap();
        }

        #[test]
        fn removing_last_file_without_surviving_update_scope_uses_full_refresh() {
            let fixture = Fixture::new("prepare-last-removal-fallback");
            let music = fixture.path("Music");
            let removed = music.join("Album/removed.flac");
            write(&removed, b"removed");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed-live",
            );
            fs::remove_file(&removed).unwrap();
            let staging = fixture.path("library.next");

            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "managed-next",
            ));

            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::Full);
            cleanup_managed_generation(&staging).unwrap();
        }

        #[test]
        fn managed_generation_preparation_rejects_incompatible_manifest_without_mutation() {
            let fixture = Fixture::new("prepare-invalid-manifest");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = fixture.path("library");
            fs::create_dir(&live).unwrap();
            let marker = live.join(MANIFEST_NAME);
            let incompatible = br#"{"schema_version":99,"generation_id":"old","sources":{}}"#;
            write(&marker, incompatible);
            let staging = fixture.path("library.next");

            assert!(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "next-generation",
            )
            .is_err());

            assert_eq!(fs::read(marker).unwrap(), incompatible);
            assert!(!staging.exists());
        }

        #[test]
        fn managed_generation_preparation_refuses_preexisting_staging() {
            let fixture = Fixture::new("prepare-existing-staging");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "live-generation",
            );
            let before = managed_generation_snapshot(&live).unwrap();
            let staging = fixture.path("library.next");
            write(&staging.join("keep.txt"), b"keep");

            assert!(prepare_managed_generation(
                &live,
                &staging,
                std::slice::from_ref(&music),
                "next-generation",
            )
            .is_err());

            assert_eq!(fs::read(staging.join("keep.txt")).unwrap(), b"keep");
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
        }

        #[test]
        fn managed_generation_preparation_failure_never_mutates_live() {
            let fixture = Fixture::new("prepare-failure");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "live-generation",
            );
            let before = managed_generation_snapshot(&live).unwrap();
            let staging = fixture.path("library.next");
            let missing_root = fixture.path("removed-root");

            assert!(prepare_managed_generation(
                &live,
                &staging,
                &[music, missing_root],
                "next-generation",
            )
            .is_err());

            assert!(!staging.exists());
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
        }

        #[test]
        fn managed_generation_comparison_ignores_only_generation_id_and_creation_order() {
            let fixture = Fixture::new("comparison-equivalent");
            let music = fixture.path("Music");
            write(&music.join("Album/track.flac"), b"track");
            write(&music.join("Album/cover.jpg"), b"cover");
            write(
                &music.join("Album/image.cue"),
                b"FILE \"track.flac\" WAVE\n  TRACK 01 AUDIO\n",
            );
            let first = build_generation(
                &fixture,
                "generation-a",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "generation-a",
            );
            let second = build_generation(
                &fixture,
                "generation-b",
                &first,
                std::slice::from_ref(&music),
                "generation-b",
            );

            assert_eq!(
                compare_managed_generations(&first, &second).unwrap(),
                MirrorComparison::Equivalent
            );
        }

        #[test]
        fn managed_generation_comparison_detects_paths_targets_sources_and_sidecars() {
            let fixture = Fixture::new("comparison-different");
            let music = fixture.path("one/Music");
            let other_music = fixture.path("two/Music");
            for root in [&music, &other_music] {
                write(&root.join("Album/track.flac"), b"track");
                write(&root.join("Album/other.flac"), b"other");
                write(&root.join("Album/cover.jpg"), b"cover");
                write(
                    &root.join("Album/image.cue"),
                    b"FILE \"track.flac\" WAVE\n  TRACK 01 AUDIO\n",
                );
            }
            let first = build_generation(
                &fixture,
                "generation-a",
                &fixture.path("missing-live-a"),
                std::slice::from_ref(&music),
                "generation-a",
            );
            let changed_source = build_generation(
                &fixture,
                "generation-source",
                &fixture.path("missing-live-b"),
                std::slice::from_ref(&other_music),
                "generation-source",
            );
            assert_eq!(
                compare_managed_generations(&first, &changed_source).unwrap(),
                MirrorComparison::Different
            );

            let with_added_file = fixture.path("generation-added");
            write(&music.join("Album/added.dsf"), b"added");
            build_filtered_staging(
                &with_added_file,
                &first,
                std::slice::from_ref(&music),
                "generation-added",
            )
            .unwrap();
            assert_eq!(
                compare_managed_generations(&first, &with_added_file).unwrap(),
                MirrorComparison::Different
            );
            fs::remove_file(music.join("Album/added.dsf")).unwrap();

            let without_track = build_generation(
                &fixture,
                "generation-no-track",
                &first,
                std::slice::from_ref(&music),
                "generation-no-track",
            );
            fs::remove_file(without_track.join("Music/Album/track.flac")).unwrap();
            assert_eq!(
                compare_managed_generations(&first, &without_track).unwrap(),
                MirrorComparison::Different
            );

            let changed_target = build_generation(
                &fixture,
                "generation-target",
                &first,
                std::slice::from_ref(&music),
                "generation-target",
            );
            let track_link = changed_target.join("Music/Album/track.flac");
            fs::remove_file(&track_link).unwrap();
            symlink(music.join("Album/other.flac"), &track_link).unwrap();
            assert_eq!(
                compare_managed_generations(&first, &changed_target).unwrap(),
                MirrorComparison::Different
            );

            let without_artwork = build_generation(
                &fixture,
                "generation-no-art",
                &first,
                std::slice::from_ref(&music),
                "generation-no-art",
            );
            fs::remove_file(without_artwork.join("Music/Album/cover.jpg")).unwrap();
            assert_eq!(
                compare_managed_generations(&first, &without_artwork).unwrap(),
                MirrorComparison::Different
            );

            let without_cue = build_generation(
                &fixture,
                "generation-no-cue",
                &first,
                std::slice::from_ref(&music),
                "generation-no-cue",
            );
            fs::remove_file(without_cue.join("Music/Album/image.cue")).unwrap();
            assert_eq!(
                compare_managed_generations(&first, &without_cue).unwrap(),
                MirrorComparison::Different
            );
        }

        #[test]
        fn managed_generation_comparison_rejects_corruption_and_unsafe_targets() {
            let fixture = Fixture::new("comparison-invalid");
            let music = fixture.path("Music");
            let outside = fixture.path("Outside");
            write(&music.join("track.flac"), b"track");
            write(&outside.join("escape.flac"), b"outside");
            let valid = build_generation(
                &fixture,
                "generation-valid",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "generation-valid",
            );
            let corrupt = build_generation(
                &fixture,
                "generation-corrupt",
                &valid,
                std::slice::from_ref(&music),
                "generation-corrupt",
            );
            write(&corrupt.join("Music/unexpected.txt"), b"unexpected");
            assert!(compare_managed_generations(&valid, &corrupt).is_err());

            let escaped = build_generation(
                &fixture,
                "generation-escaped",
                &valid,
                std::slice::from_ref(&music),
                "generation-escaped",
            );
            fs::remove_file(escaped.join("Music/track.flac")).unwrap();
            symlink(
                outside.join("escape.flac"),
                escaped.join("Music/track.flac"),
            )
            .unwrap();
            assert!(compare_managed_generations(&valid, &escaped).is_err());
        }

        #[test]
        fn crash_state_classifies_only_determinable_generation_pairs() {
            let fixture = Fixture::new("crash-state");
            let current = fixture.path("Current");
            let proposed = fixture.path("Proposed");
            write(&current.join("track.flac"), b"current");
            write(&proposed.join("track.flac"), b"proposed");

            let legacy = fixture.path("legacy");
            legacy_generation(&legacy, "Current", &current);
            assert_eq!(
                classify_mirror_crash_state(
                    &legacy,
                    &fixture.path("missing-next"),
                    std::slice::from_ref(&current),
                ),
                MirrorCrashState::LegacyOnly
            );

            let managed_current = build_generation(
                &fixture,
                "managed-current",
                &fixture.path("missing-live-current"),
                std::slice::from_ref(&current),
                "managed-current",
            );
            assert_eq!(
                classify_mirror_crash_state(
                    &managed_current,
                    &fixture.path("missing-next-managed"),
                    std::slice::from_ref(&current),
                ),
                MirrorCrashState::ManagedLiveNormal
            );

            let managed_proposed = build_generation(
                &fixture,
                "managed-proposed",
                &fixture.path("missing-live-proposed"),
                std::slice::from_ref(&proposed),
                "managed-proposed",
            );
            assert_eq!(
                classify_mirror_crash_state(
                    &managed_current,
                    &managed_proposed,
                    std::slice::from_ref(&current),
                ),
                MirrorCrashState::Ambiguous
            );
            // The same on-disk pair could be pre-exchange staging or a
            // post-exchange previous generation. Compatibility cannot prove history.
            assert_eq!(
                classify_mirror_crash_state(
                    &managed_proposed,
                    &managed_current,
                    std::slice::from_ref(&current),
                ),
                MirrorCrashState::LikelyPostExchangePreConfig
            );

            let second_current = build_generation(
                &fixture,
                "managed-current-2",
                &managed_current,
                std::slice::from_ref(&current),
                "managed-current-2",
            );
            assert_eq!(
                classify_mirror_crash_state(
                    &managed_current,
                    &second_current,
                    std::slice::from_ref(&current),
                ),
                MirrorCrashState::Ambiguous
            );

            let previous_legacy = fixture.path("previous-legacy");
            legacy_generation(&previous_legacy, "Current", &current);
            assert_eq!(
                classify_mirror_crash_state(
                    &managed_current,
                    &previous_legacy,
                    std::slice::from_ref(&current),
                ),
                MirrorCrashState::PreviousGenerationPendingCleanup
            );
        }

        #[test]
        fn crash_state_never_recovers_ambiguous_or_invalid_trees_implicitly() {
            let fixture = Fixture::new("crash-invalid");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let managed = build_generation(
                &fixture,
                "managed",
                &fixture.path("missing-live"),
                std::slice::from_ref(&music),
                "managed",
            );

            let unexpected = fixture.path("unexpected-next");
            fs::create_dir(&unexpected).unwrap();
            write(&unexpected.join("real-file"), b"unexpected");
            assert_eq!(
                classify_mirror_crash_state(
                    &managed,
                    &unexpected,
                    std::slice::from_ref(&music),
                ),
                MirrorCrashState::Ambiguous
            );

            let corrupt = build_generation(
                &fixture,
                "corrupt",
                &managed,
                std::slice::from_ref(&music),
                "corrupt",
            );
            fs::write(corrupt.join(MANIFEST_NAME), b"invalid json").unwrap();
            assert_eq!(
                classify_mirror_crash_state(
                    &corrupt,
                    &fixture.path("missing-next"),
                    std::slice::from_ref(&music),
                ),
                MirrorCrashState::Invalid
            );

            let root_symlink = fixture.path("root-symlink");
            symlink(&managed, &root_symlink).unwrap();
            assert_eq!(
                classify_mirror_crash_state(
                    &root_symlink,
                    &fixture.path("missing-next-2"),
                    std::slice::from_ref(&music),
                ),
                MirrorCrashState::Invalid
            );
        }

        fn assert_link(path: &Path, target: &Path) {
            assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
            assert_eq!(
                fs::canonicalize(path).unwrap(),
                fs::canonicalize(target).unwrap()
            );
        }

        #[test]
        fn builder_filters_files_preserves_paths_and_omits_empty_directories() {
            let fixture = Fixture::new("filtered");
            let music = fixture.path("Music");
            let audio = music.join("Artista Único/Álbum Com Espaço/01 Track.FlAc");
            let artwork = music.join("Somente Arte/Imagem.WeBp");
            let ignored = music.join("Artista Único/Álbum Com Espaço/program.prg");
            write(&audio, b"audio-original");
            write(&artwork, b"art-original");
            write(&ignored, b"ignored-original");
            fs::create_dir_all(music.join("Empty")).unwrap();
            fs::create_dir_all(music.join("IgnoredOnly")).unwrap();
            write(&music.join("IgnoredOnly/disc.iso"), b"iso");
            let audio_before = fs::read(&audio).unwrap();
            let audio_inode = fs::metadata(&audio).unwrap().ino();

            let result = fixture.build(std::slice::from_ref(&music)).unwrap();

            let source = result.staging_path.join("Music");
            assert_link(
                &source.join("Artista Único/Álbum Com Espaço/01 Track.FlAc"),
                &audio,
            );
            assert_link(&source.join("Somente Arte/Imagem.WeBp"), &artwork);
            assert!(!source
                .join("Artista Único/Álbum Com Espaço/program.prg")
                .exists());
            assert!(!source.join("Empty").exists());
            assert!(!source.join("IgnoredOnly").exists());
            assert_eq!(fs::read(&audio).unwrap(), audio_before);
            assert_eq!(fs::metadata(&audio).unwrap().ino(), audio_inode);
        }

        #[test]
        fn builder_handles_multi_root_duplicates_basenames_aliases_and_overlap() {
            let fixture = Fixture::new("roots");
            let first = fixture.path("one/Music");
            let second = fixture.path("two/Music");
            let third = fixture.path("three/Music");
            let nested = first.join("Nested");
            write(&first.join("first.flac"), b"first");
            write(&second.join("second.opus"), b"second");
            write(&third.join("third.aac"), b"third");
            write(&nested.join("nested.mp3"), b"nested");
            let alias = fixture.path("alias");
            symlink(&first, &alias).unwrap();

            let result = fixture
                .build(&[
                    first.clone(),
                    second.clone(),
                    third.clone(),
                    alias,
                    nested.clone(),
                ])
                .unwrap();

            assert_eq!(result.manifest.sources.len(), 4);
            assert_eq!(
                result.manifest.sources["Music"],
                fs::canonicalize(&first).unwrap()
            );
            assert_eq!(
                result.manifest.sources["Music (2)"],
                fs::canonicalize(&second).unwrap()
            );
            assert_eq!(
                result.manifest.sources["Music (3)"],
                fs::canonicalize(&third).unwrap()
            );
            assert_eq!(
                result.manifest.sources["Nested"],
                fs::canonicalize(&nested).unwrap()
            );
            assert_link(
                &result.staging_path.join("Music/Nested/nested.mp3"),
                &nested.join("nested.mp3"),
            );
            assert_link(
                &result.staging_path.join("Nested/nested.mp3"),
                &nested.join("nested.mp3"),
            );
        }

        #[test]
        fn overlap_admission_orders_aliases_and_sibling_boundaries() {
            let fixture = Fixture::new("overlap-admission");
            let parent = fixture.path("musicas");
            let child = parent.join("reggae");
            let sibling = parent.join("reggae-extra");
            fs::create_dir_all(&child).unwrap();
            fs::create_dir_all(&sibling).unwrap();
            let alias = fixture.path("alias");
            symlink(&child, &alias).unwrap();
            for previous in [vec![], vec![child.clone()], vec![parent.clone()]] {
                for next in [
                    vec![parent.clone(), child.clone()],
                    vec![child.clone(), parent.clone()],
                ] {
                    assert_eq!(
                        validate_new_source_overlaps(&previous, &next),
                        Err(NEW_SOURCE_OVERLAP.into())
                    );
                }
            }
            assert!(
                validate_new_source_overlaps(&[], &[child.clone(), child.clone(), alias.clone()]).is_ok()
            );
            assert!(validate_new_source_overlaps(&[child.clone()], &[alias, sibling]).is_ok());
            assert!(!fixture.path("library.next").exists());
            assert!(!fixture.path("library").exists());
        }

        #[test]
        fn overlap_admission_grandfathers_pairs_but_not_new_conflicts() {
            let fixture = Fixture::new("old-overlap");
            let parent = fixture.path("musicas");
            let child = parent.join("reggae");
            let deeper = child.join("dub");
            let disjoint = fixture.path("separate");
            fs::create_dir_all(&deeper).unwrap();
            fs::create_dir_all(&disjoint).unwrap();
            let previous = vec![parent.clone(), child.clone()];
            assert!(validate_new_source_overlaps(&previous, &[child.clone(), parent.clone()]).is_ok());
            assert!(
                validate_new_source_overlaps(&previous, &[parent.clone(), child.clone(), disjoint]).is_ok()
            );
            assert!(validate_new_source_overlaps(&previous, std::slice::from_ref(&parent)).is_ok());
            assert!(validate_new_source_overlaps(&previous, std::slice::from_ref(&child)).is_ok());
            // Equal/fewer conflict counts do not excuse a different new pair.
            assert_eq!(
                validate_new_source_overlaps(&previous, &[parent.clone(), deeper.clone()]),
                Err(NEW_SOURCE_OVERLAP.into())
            );
            assert_eq!(
                validate_new_source_overlaps(&previous, &[parent, child, deeper]),
                Err(NEW_SOURCE_OVERLAP.into())
            );
        }

        // Diagnostic fixtures only: current acceptance is characterized here,
        // not endorsed as a future overlap policy. No MPD process is started.
        #[test]
        fn local_index_diagnostic_sequential_overlap_and_reference_loss() {
            use crate::audio::{AudioEngine, MediaLocator, MpdQueueEntry, MpdQueueStructure};
            use crate::playlists::{NewPlaylistItem, PlaylistItemMetadata, PlaylistStore};
            for child_first in [true, false] {
                let fixture = Fixture::new(if child_first {
                    "child-parent"
                } else {
                    "parent-child"
                });
                let parent = fixture.path("dados/musicas");
                let child = parent.join("reggae");
                let track = child.join("Artista Único/Álbum sem ordem/01 faixa.flac");
                write(&track, b"inert fixture, not decoded");
                for index in 0..2048 {
                    write(
                        &parent.join(format!(
                            "outros/Grupo {}/Disco {}/faixa {}.mp3",
                            index % 17,
                            index % 13,
                            index
                        )),
                        b"fixture",
                    );
                }
                write(&parent.join("outros/ignored.prg"), b"ignored");
                let (initial, added, old_uri, other_uri) = if child_first {
                    (
                        &child,
                        &parent,
                        "reggae/Artista Único/Álbum sem ordem/01 faixa.flac",
                        "musicas/reggae/Artista Único/Álbum sem ordem/01 faixa.flac",
                    )
                } else {
                    (
                        &parent,
                        &child,
                        "musicas/reggae/Artista Único/Álbum sem ordem/01 faixa.flac",
                        "reggae/Artista Único/Álbum sem ordem/01 faixa.flac",
                    )
                };
                let live = build_generation(
                    &fixture,
                    "library",
                    &fixture.path("missing"),
                    std::slice::from_ref(initial),
                    "initial",
                );
                let old_manifest = read_manifest(&live).unwrap();
                // Replacing the source in one Save is not a URI migration:
                // Settings refuses an added source without a targeted-safe plan.
                let replacement = expect_prepared(prepare_managed_generation(
                    &live,
                    &fixture.path("library.next"),
                    std::slice::from_ref(added),
                    "replace",
                ));
                assert_eq!(
                    plan_added_sources_refresh(&live, &replacement).unwrap(),
                    None
                );
                let previous = crate::local_media::resolve_local_sources(
                    &live,
                    &[initial.to_string_lossy().into_owned()],
                )
                .unwrap();
                assert_eq!(
                    crate::added_source_save_needs_targeted_refresh(
                        &previous,
                        &replacement,
                        &ManagedRefreshPlan::Full,
                    ),
                    Err(crate::ADDED_SOURCE_FULL_RESCAN_REQUIRED)
                );
                cleanup_prepared_managed_generation(&replacement).unwrap();
                let roots = vec![initial.clone(), added.clone()];
                let prepared = expect_prepared(prepare_managed_generation(
                    &live,
                    &fixture.path("library.next"),
                    &roots,
                    "overlap",
                ));
                assert_eq!(prepared.manifest.sources.len(), 2);
                for (id, root) in &old_manifest.sources {
                    assert_eq!(prepared.manifest.sources.get(id), Some(root));
                }
                let plan = plan_added_sources_refresh(&live, &prepared)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    plan,
                    ManagedRefreshPlan::Targeted {
                        directories: vec![if child_first { "musicas" } else { "reggae" }.into()],
                        changed_files: if child_first { 2049 } else { 1 },
                    }
                );
                let previous = crate::local_media::resolve_local_sources(
                    &live,
                    &[initial.to_string_lossy().into_owned()],
                )
                .unwrap();
                assert_eq!(
                    crate::added_source_save_needs_targeted_refresh(&previous, &prepared, &plan),
                    Ok(true)
                );
                let publication =
                    publish_targeted_added_sources_generation(&live, &prepared, &plan).unwrap();
                publication.cleanup_previous().unwrap();
                let folders = roots
                    .iter()
                    .map(|root| root.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                let old = crate::local_media::resolve_local_path(&live, old_uri, &folders).unwrap();
                let other = crate::local_media::resolve_local_path(&live, other_uri, &folders).unwrap();
                assert_eq!(old.physical_path, other.physical_path);
                assert_eq!(
                    fs::metadata(live.join(old_uri)).unwrap().ino(),
                    fs::metadata(live.join(other_uri)).unwrap().ino()
                );
                assert_ne!(old.source_id, other.source_id);
                let records = vec![format!("file: {old_uri}"), format!("file: {other_uri}")];
                let audit = AudioEngine::audit_raw_local_catalog(&records).unwrap();
                assert!(audit.is_clean());
                assert_eq!(audit.total_files, 2); // syntactically valid distinct URIs, one physical file

                let store = PlaylistStore::new(fixture.path("playlists.json"));
                let playlist = store
                    .create_with_items(
                        "Overlap fixture",
                        vec![NewPlaylistItem {
                            media_locator: MediaLocator::Local {
                                uri: old_uri.into(),
                            },
                            metadata: PlaylistItemMetadata {
                                title: "Track".into(),
                                artist: "Artist".into(),
                                album: "Album".into(),
                                duration: None,
                            },
                        }],
                    )
                    .unwrap();
                let playlist_before = fs::read(fixture.path("playlists.json")).unwrap();
                let removal = expect_prepared(prepare_managed_generation(
                    &live,
                    &fixture.path("library.next"),
                    std::slice::from_ref(added),
                    "remove-original",
                ));
                let removal_plan = plan_removed_sources_refresh(&live, &removal)
                    .unwrap()
                    .unwrap();
                let queue = MpdQueueStructure {
                    entries: vec![MpdQueueEntry {
                        uri: old_uri.into(),
                        pos: 0,
                        id: 7,
                    }],
                    playlist_version: 1,
                };
                assert!(crate::queue_references_removed_sources(
                    &removal_plan,
                    &[],
                    &queue
                ));
                let publication = publish_prepared_managed_generation(&live, &removal).unwrap();
                publication.cleanup_previous().unwrap();
                let folders = vec![added.to_string_lossy().into_owned()];
                assert_eq!(
                    crate::local_media::resolve_local_path(&live, old_uri, &folders),
                    Err(crate::local_media::LocalPathResolveError::SourceNotFound)
                );
                assert!(crate::local_media::resolve_local_path(&live, other_uri, &folders).is_ok());
                // Favorites resolve an album directory through this same resolver.
                assert_eq!(
                    crate::local_media::resolve_local_path(
                        &live,
                        old_uri.rsplit_once('/').unwrap().0,
                        &folders
                    ),
                    Err(crate::local_media::LocalPathResolveError::SourceNotFound)
                );
                assert_eq!(store.get(&playlist.id).unwrap(), playlist);
                assert_eq!(
                    fs::read(fixture.path("playlists.json")).unwrap(),
                    playlist_before
                );
                assert_eq!(fs::read(&track).unwrap(), b"inert fixture, not decoded");
            }
        }

        #[test]
        fn local_index_diagnostic_equal_alias_and_sibling_prefixes() {
            let fixture = Fixture::new("aliases-siblings");
            let child = fixture.path("musicas/reggae");
            let sibling = fixture.path("musicas/reggae-extra");
            write(&child.join("song.flac"), b"one");
            write(&sibling.join("song.flac"), b"two");
            let alias = fixture.path("alias");
            symlink(&child, &alias).unwrap();
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing"),
                std::slice::from_ref(&child),
                "initial",
            );
            assert!(matches!(
                prepare_managed_generation(
                    &live,
                    &fixture.path("library.next"),
                    &[child.clone(), child.clone(), alias.clone()],
                    "alias"
                ),
                Ok(ManagedGenerationPreparation::Unchanged)
            ));
            assert!(!fixture.path("library.next").exists());
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[child.clone(), alias, sibling.clone()],
                "siblings",
            ));
            assert_eq!(prepared.manifest.sources.len(), 2);
            assert!(!child.starts_with(&sibling) && !sibling.starts_with(&child));
            assert_link(
                &prepared.staging_path.join("reggae/song.flac"),
                &child.join("song.flac"),
            );
            assert_link(
                &prepared.staging_path.join("reggae-extra/song.flac"),
                &sibling.join("song.flac"),
            );
            cleanup_prepared_managed_generation(&prepared).unwrap();
        }

        #[test]
        fn local_index_diagnostic_nested_read_denial_preserves_live_and_allows_retry() {
            use std::sync::Arc;
            let fixture = Fixture::new("read-denial");
            let parent = fixture.path("musicas");
            let child = parent.join("reggae");
            let denied = parent.join("Unreadable");
            write(&child.join("song.flac"), b"old");
            write(&denied.join("song.flac"), b"unreadable");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing"),
                std::slice::from_ref(&child),
                "initial",
            );
            let before = managed_generation_snapshot(&live).unwrap();
            let mode = fs::metadata(&denied).unwrap().permissions().mode();
            fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();
            let read_denied = fs::read_dir(&denied).is_err();
            let coordinator = Arc::new(crate::config_save_operation::Coordinator::new());
            let (_, operation) = coordinator.reserve().unwrap();
            let result = prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[child.clone(), parent.clone()],
                "denied",
            );
            fs::set_permissions(&denied, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                read_denied,
                "fixture requires directory permissions to be enforced"
            );
            let error = result.unwrap_err();
            assert!(error.contains("Falha ao listar diretório"));
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
            assert!(!fixture.path("library.next").exists());
            operation.finish(Err(error), false); // pre-intent error is Failed, not Uncertain
            assert_eq!(
                coordinator.snapshot().unwrap().unwrap().phase,
                crate::config_save_operation::Phase::Failed
            );
            let (_, retry) = coordinator.reserve().unwrap();
            let prepared = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[child, parent],
                "retry",
            ));
            cleanup_prepared_managed_generation(&prepared).unwrap();
            retry.finish(Ok(()), false);
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
        }

        #[test]
        fn local_index_diagnostic_file_disappears_after_scan_cleanup_and_retry() {
            let fixture = Fixture::new("file-read-race");
            let parent = fixture.path("musicas");
            let child = parent.join("reggae");
            let added = parent.join("other/new.flac");
            write(&child.join("old.flac"), b"old");
            write(&added, b"new");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing"),
                std::slice::from_ref(&child),
                "initial",
            );
            let before = managed_generation_snapshot(&live).unwrap();
            let staging = fixture.path("library.next");
            let roots = [child, parent];
            let error = build_filtered_staging_with_hook(&staging, &live, &roots, "raced", || {
                fs::remove_file(&added).unwrap();
                Ok(())
            })
            .unwrap_err();
            assert!(error.contains("Arquivo selecionado indisponível"));
            assert!(!staging.exists());
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
            write(&added, b"new");
            let prepared = expect_prepared(prepare_managed_generation(&live, &staging, &roots, "retry"));
            cleanup_prepared_managed_generation(&prepared).unwrap();
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
        }

        #[test]
        fn local_index_diagnostic_unreadable_file_and_cue_are_not_directory_failures() {
            let fixture = Fixture::new("unreadable-files");
            let music = fixture.path("Music");
            let audio = music.join("track.flac");
            let cue = music.join("image.cue");
            write(&audio, b"fixture");
            write(&cue, b"FILE \"track.flac\" WAVE\nTRACK 01 AUDIO\n");
            let mode = fs::metadata(&audio).unwrap().permissions().mode();
            fs::set_permissions(&audio, fs::Permissions::from_mode(0o000)).unwrap();
            fs::set_permissions(&cue, fs::Permissions::from_mode(0o000)).unwrap();
            let audio_read = fs::read(&audio);
            let cue_read = fs::read(&cue);
            let result = fixture.build(std::slice::from_ref(&music));
            fs::set_permissions(&audio, fs::Permissions::from_mode(mode)).unwrap();
            fs::set_permissions(&cue, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                audio_read.is_err() && cue_read.is_err(),
                "fixture needs enforced file permissions"
            );
            assert!(result.unwrap_err().starts_with("Falha ao ler CUE:"));
            assert!(!fixture.path("library.next").exists());
            // Restored access permits a clean retry; audio byte decoding remains
            // outside the builder's contract.
            let built = fixture.build(std::slice::from_ref(&music)).unwrap();
            assert_link(&built.staging_path.join("Music/track.flac"), &audio);
            assert_link(&built.staging_path.join("Music/image.cue"), &cue);
            assert!(crate::audio::AudioEngine::audit_raw_local_catalog(&[])
                .unwrap()
                .is_clean());
            cleanup_prepared_managed_generation(&built).unwrap();
        }

        #[test]
        fn legacy_mapping_preserves_ids_ignores_stale_links_and_rejects_ambiguity() {
            let fixture = Fixture::new("legacy");
            let first = fixture.path("one/Music");
            let second = fixture.path("two/Music");
            fs::create_dir_all(&first).unwrap();
            fs::create_dir_all(&second).unwrap();
            let library = fixture.path("library");
            fs::create_dir(&library).unwrap();
            symlink(&first, library.join("Music (2)")).unwrap();
            symlink(&second, library.join("Music")).unwrap();

            let result = fixture.build(&[second.clone(), first.clone()]).unwrap();
            assert_eq!(
                result.manifest.sources["Music"],
                fs::canonicalize(&second).unwrap()
            );
            assert_eq!(
                result.manifest.sources["Music (2)"],
                fs::canonicalize(&first).unwrap()
            );

            cleanup_managed_generation(&result.staging_path).unwrap();
            let broken = library.join("Removed");
            symlink(fixture.path("missing-root"), &broken).unwrap();
            let removed_result = fixture.build(std::slice::from_ref(&first)).unwrap();
            assert_eq!(
                removed_result.manifest.sources,
                BTreeMap::from([("Music (2)".to_string(), fs::canonicalize(&first).unwrap())])
            );
            assert!(fs::symlink_metadata(library.join("Music"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(fs::symlink_metadata(&broken)
                .unwrap()
                .file_type()
                .is_symlink());

            cleanup_managed_generation(&removed_result.staging_path).unwrap();
            write(&library.join("unexpected.txt"), b"keep");
            let error = fixture.build(std::slice::from_ref(&first)).unwrap_err();
            assert!(error.contains("entrada inesperada"));
            assert_eq!(fs::read(library.join("unexpected.txt")).unwrap(), b"keep");
            fs::remove_file(library.join("unexpected.txt")).unwrap();

            fs::create_dir(library.join("unexpected-directory")).unwrap();
            let error = fixture.build(std::slice::from_ref(&first)).unwrap_err();
            assert!(error.contains("entrada inesperada"));
            fs::remove_dir(library.join("unexpected-directory")).unwrap();

            symlink(&first, library.join("Duplicate")).unwrap();
            let error = fixture.build(std::slice::from_ref(&first)).unwrap_err();
            assert!(error.contains("IDs ambíguos"));
        }

        #[test]
        fn managed_generation_preserves_reordered_survivors_and_allocates_new_collisions() {
            let fixture = Fixture::new("managed-next");
            let first = fixture.path("one/Music");
            let second = fixture.path("two/Music");
            let new_root = fixture.path("three/Music");
            write(&first.join("first.flac"), b"first");
            write(&second.join("second.flac"), b"second");
            write(&new_root.join("new.flac"), b"new");

            let first_generation = build_filtered_staging(
                &fixture.path("generation-one"),
                &fixture.path("absent-live"),
                &[first.clone(), second.clone()],
                "generation-one",
            )
            .unwrap();
            let first_id = first_generation
                .manifest
                .sources
                .iter()
                .find_map(|(id, root)| {
                    (root == &fs::canonicalize(&first).unwrap()).then(|| id.clone())
                })
                .unwrap();
            let second_id = first_generation
                .manifest
                .sources
                .iter()
                .find_map(|(id, root)| {
                    (root == &fs::canonicalize(&second).unwrap()).then(|| id.clone())
                })
                .unwrap();

            let reordered = build_filtered_staging(
                &fixture.path("generation-reordered"),
                &first_generation.staging_path,
                &[second.clone(), first.clone()],
                "generation-reordered",
            )
            .unwrap();
            assert_eq!(
                reordered.manifest.sources[&first_id],
                fs::canonicalize(&first).unwrap()
            );
            assert_eq!(
                reordered.manifest.sources[&second_id],
                fs::canonicalize(&second).unwrap()
            );

            let removed = build_filtered_staging(
                &fixture.path("generation-removed"),
                &first_generation.staging_path,
                std::slice::from_ref(&first),
                "generation-removed",
            )
            .unwrap();
            assert_eq!(removed.manifest.sources.len(), 1);
            assert_eq!(
                removed.manifest.sources[&first_id],
                fs::canonicalize(&first).unwrap()
            );
            assert!(!removed.manifest.sources.contains_key(&second_id));

            let only_first = build_filtered_staging(
                &fixture.path("generation-only-first"),
                &fixture.path("another-absent-live"),
                std::slice::from_ref(&first),
                "generation-only-first",
            )
            .unwrap();
            let added = build_filtered_staging(
                &fixture.path("generation-added"),
                &only_first.staging_path,
                &[first.clone(), new_root.clone()],
                "generation-added",
            )
            .unwrap();
            assert_eq!(
                added.manifest.sources[&first_id],
                fs::canonicalize(&first).unwrap()
            );
            assert_eq!(
                added.manifest.sources["Music (3)"],
                fs::canonicalize(&new_root).unwrap()
            );
        }

        #[test]
        fn managed_marker_corruption_never_falls_back_to_legacy() {
            let fixture = Fixture::new("managed-corrupt");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let canonical_music = fs::canonicalize(&music).unwrap();

            let invalid_json = fixture.path("invalid-json");
            fs::create_dir(&invalid_json).unwrap();
            write(&invalid_json.join(MANIFEST_NAME), b"not-json");
            assert!(build_filtered_staging(
                &fixture.path("next-invalid-json"),
                &invalid_json,
                std::slice::from_ref(&music),
                "next-invalid-json"
            )
            .is_err());

            let unknown_schema = fixture.path("unknown-schema");
            fs::create_dir(&unknown_schema).unwrap();
            let json = serde_json::json!({
                "schema_version": 99,
                "generation_id": "old",
                "sources": { "Music": canonical_music }
            });
            write(
                &unknown_schema.join(MANIFEST_NAME),
                serde_json::to_vec(&json).unwrap().as_slice(),
            );
            assert!(build_filtered_staging(
                &fixture.path("next-unknown-schema"),
                &unknown_schema,
                std::slice::from_ref(&music),
                "next-unknown-schema"
            )
            .is_err());

            let symlink_marker = fixture.path("symlink-marker");
            fs::create_dir(&symlink_marker).unwrap();
            let marker_target = fixture.path("marker-target");
            write(&marker_target, b"{}");
            symlink(&marker_target, symlink_marker.join(MANIFEST_NAME)).unwrap();
            assert!(build_filtered_staging(
                &fixture.path("next-symlink-marker"),
                &symlink_marker,
                std::slice::from_ref(&music),
                "next-symlink-marker"
            )
            .is_err());

            let directory_marker = fixture.path("directory-marker");
            fs::create_dir(&directory_marker).unwrap();
            fs::create_dir(directory_marker.join(MANIFEST_NAME)).unwrap();
            assert!(build_filtered_staging(
                &fixture.path("next-directory-marker"),
                &directory_marker,
                std::slice::from_ref(&music),
                "next-directory-marker"
            )
            .is_err());

            let duplicate_root = fixture.path("duplicate-root");
            fs::create_dir(&duplicate_root).unwrap();
            let duplicate_json = serde_json::json!({
                "schema_version": 1,
                "generation_id": "duplicate-root",
                "sources": {
                    "Music": canonical_music,
                    "Music (2)": canonical_music,
                }
            });
            write(
                &duplicate_root.join(MANIFEST_NAME),
                serde_json::to_vec(&duplicate_json).unwrap().as_slice(),
            );
            assert!(build_filtered_staging(
                &fixture.path("next-duplicate-root"),
                &duplicate_root,
                std::slice::from_ref(&music),
                "next-duplicate-root"
            )
            .is_err());

            let incompatible = fixture.path("incompatible");
            fs::create_dir(&incompatible).unwrap();
            let manifest = MirrorManifest {
                schema_version: 1,
                registry_id: None,
                generation_id: "incompatible".to_string(),
                sources: BTreeMap::from([("Music".to_string(), canonical_music)]),
            };
            write_manifest(&incompatible, &manifest).unwrap();
            write(&incompatible.join("unexpected.txt"), b"unexpected");
            assert!(build_filtered_staging(
                &fixture.path("next-incompatible"),
                &incompatible,
                std::slice::from_ref(&music),
                "next-incompatible"
            )
            .is_err());

            for staging in [
                "next-invalid-json",
                "next-unknown-schema",
                "next-symlink-marker",
                "next-directory-marker",
                "next-duplicate-root",
                "next-incompatible",
            ] {
                assert!(!fixture.path(staging).exists());
            }
        }

        #[test]
        fn internal_file_directory_broken_and_loop_symlinks_are_ignored() {
            let fixture = Fixture::new("links");
            let music = fixture.path("Music");
            let outside = fixture.path("outside.flac");
            write(&music.join("real.flac"), b"real");
            write(&outside, b"outside");
            fs::create_dir_all(music.join("RealDir")).unwrap();
            write(&music.join("RealDir/hidden.flac"), b"hidden");
            symlink(music.join("real.flac"), music.join("inside-link.flac")).unwrap();
            symlink(&outside, music.join("outside-link.flac")).unwrap();
            symlink(music.join("RealDir"), music.join("dir-link")).unwrap();
            symlink(music.join("missing"), music.join("broken.flac")).unwrap();
            symlink("loop-b", music.join("loop-a")).unwrap();
            symlink("loop-a", music.join("loop-b")).unwrap();

            let result = fixture.build(std::slice::from_ref(&music)).unwrap();
            let source = result.staging_path.join("Music");

            assert_link(&source.join("real.flac"), &music.join("real.flac"));
            assert_link(
                &source.join("RealDir/hidden.flac"),
                &music.join("RealDir/hidden.flac"),
            );
            for ignored in [
                "inside-link.flac",
                "outside-link.flac",
                "dir-link",
                "broken.flac",
                "loop-a",
                "loop-b",
            ] {
                assert!(!source.join(ignored).exists());
            }
        }

        // Regression: selected CUE and audio loss are both operational failures.
        #[test]
        fn local_index_gap_regression_selected_cue_or_audio_loss_aborts_and_retries() {
            for removed in ["album.cue", "track.flac"] {
                let fixture = Fixture::new("index-gap-race");
                let old = fixture.path("Old");
                let added = fixture.path("Added");
                write(&old.join("old.flac"), b"old fixture");
                write(&added.join("track.flac"), b"inert audio fixture");
                let cue = b"FILE \"track.flac\" WAVE\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n";
                write(&added.join("album.cue"), cue);
                let live = build_generation(
                    &fixture,
                    "library",
                    &fixture.path("missing"),
                    std::slice::from_ref(&old),
                    "previous",
                );
                let before = managed_generation_snapshot(&live).unwrap();
                let staging = fixture.path("library.next");
                let roots = [old, added.clone()];
                let result =
                    build_filtered_staging_with_hook(&staging, &live, &roots, "raced", || {
                        fs::remove_file(added.join(removed)).unwrap();
                        Ok(())
                    });
                assert!(result
                    .unwrap_err()
                    .contains("Arquivo selecionado indisponível"));
                assert!(!staging.exists());
                assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
                write(
                    &added.join(removed),
                    if removed == "album.cue" {
                        cue
                    } else {
                        b"restored fixture"
                    },
                );
                let retried = build_filtered_staging(&staging, &live, &roots, "retry").unwrap();
                assert_link(&staging.join("Added/album.cue"), &added.join("album.cue"));
                assert_link(&staging.join("Added/track.flac"), &added.join("track.flac"));
                cleanup_prepared_managed_generation(&retried).unwrap();
                assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
            }
        }

        #[test]
        fn local_index_gap_diagnostic_allowlist_and_cue_safety_are_not_decoding() {
            let fixture = Fixture::new("index-gap-candidates");
            let root = fixture.path("Music");
            write(&root.join("empty.flac"), b"");
            write(&root.join("inert.opus"), b"not an audio stream");
            write(&root.join("auxiliary.txt"), b"auxiliary");
            // Safe FILE syntax alone does not establish a playable CUE track.
            write(&root.join("file-only.cue"), b"FILE \"empty.flac\" WAVE\n");
            write(&root.join("invalid-encoding.cue"), b"\xff\xfe");
            let source = scan_source(&root).unwrap();
            assert_eq!(
                cue_targets(&root.join("file-only.cue"), &root, &source.playable).unwrap(),
                CueDisposition::Publish
            );
            assert_eq!(
                cue_targets(&root.join("invalid-encoding.cue"), &root, &source.playable).unwrap(),
                CueDisposition::Excluded
            );
            let built = fixture.build(std::slice::from_ref(&root)).unwrap();
            for name in ["empty.flac", "inert.opus", "file-only.cue"] {
                assert_link(
                    &built.staging_path.join("Music").join(name),
                    &root.join(name),
                );
            }
            for name in ["auxiliary.txt", "invalid-encoding.cue"] {
                assert!(!built.staging_path.join("Music").join(name).exists());
            }
            // No decoder or MPD is involved: this establishes mirror admission only.
            cleanup_prepared_managed_generation(&built).unwrap();
        }

        #[test]
        fn cue_read_io_failures_abort_owned_staging_and_allow_retry() {
            for kind in [
                std::io::ErrorKind::PermissionDenied,
                std::io::ErrorKind::Other,
            ] {
                let fixture = Fixture::new("cue-read-injected");
                let old = fixture.path("Old");
                let added = fixture.path("Added");
                write(&old.join("old.flac"), b"old");
                write(&added.join("track.flac"), b"inert fixture");
                let cue = added.join("album.cue");
                write(
                    &cue,
                    b"FILE \"track.flac\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n",
                );
                let live = build_generation(
                    &fixture,
                    "library",
                    &fixture.path("missing"),
                    std::slice::from_ref(&old),
                    "previous",
                );
                let before = managed_generation_snapshot(&live).unwrap();
                let selected = scan_source(&added).unwrap();
                let staging = fixture.path("library.next");
                let roots = [old, added.clone()];
                let coordinator =
                    std::sync::Arc::new(crate::config_save_operation::Coordinator::new());
                let (_, operation) = coordinator.reserve().unwrap();
                // The existing pre-publication seam injects a failure from the
                // production CUE reader/checker; permissions need not be enforced.
                let error = build_filtered_staging_with_hook(
                    &staging,
                    &live,
                    &roots,
                    "read-failed",
                    || {
                        cue_targets_with_reader(&cue, &added, &selected.playable, |_| {
                            Err(std::io::Error::new(kind, "injected CUE read failure"))
                        })?;
                        Ok(())
                    },
                )
                .unwrap_err();
                assert!(error.starts_with("Falha ao ler CUE:"));
                operation.finish(Err(error), false);
                assert_eq!(
                    coordinator.snapshot().unwrap().unwrap().phase,
                    crate::config_save_operation::Phase::Failed
                );
                assert!(!staging.exists());
                assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
                let (_, retry) = coordinator.reserve().unwrap();
                let retried = build_filtered_staging(&staging, &live, &roots, "retry").unwrap();
                assert_link(&staging.join("Added/album.cue"), &cue);
                cleanup_prepared_managed_generation(&retried).unwrap();
                retry.finish(Ok(()), false);
                assert_eq!(managed_generation_snapshot(&live).unwrap(), before);
            }
        }

        #[test]
        fn cue_inventory_distinguishes_unselected_references_from_selected_target_loss() {
            let fixture = Fixture::new("cue-selected-target");
            let root = fixture.path("Music");
            let target = root.join("track.flac");
            let cue = root.join("album.cue");
            write(&target, b"inert fixture");
            write(&cue, b"FILE \"track.flac\" WAVE\nTRACK 01 AUDIO\n");
            let selected = scan_source(&root).unwrap();
            assert_eq!(
                cue_targets(&cue, &root, &selected.playable).unwrap(),
                CueDisposition::Publish
            );
            fs::remove_file(&target).unwrap();
            assert!(cue_targets(&cue, &root, &selected.playable).is_err());
            // This reference was absent at scan time: expected exclusion, not I/O.
            assert_eq!(
                cue_targets(&cue, &root, &HashSet::new()).unwrap(),
                CueDisposition::Excluded
            );
            let outside = fixture.path("outside.flac");
            write(&outside, b"outside");
            symlink(&outside, &target).unwrap();
            assert!(cue_targets(&cue, &root, &selected.playable).is_err());
            assert_eq!(
                cue_targets(&cue, &root, &HashSet::new()).unwrap(),
                CueDisposition::Excluded
            );
            fs::remove_file(&target).unwrap();
            write(&target, b"restored");
            assert_eq!(
                cue_targets(&cue, &root, &selected.playable).unwrap(),
                CueDisposition::Publish
            );
        }

        #[test]
        fn valid_cues_publish_single_image_multiple_files_unicode_and_spaces() {
            let fixture = Fixture::new("cue-valid");
            let old = fixture.path("Old");
            write(&old.join("old.flac"), b"old");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&old),
                "previous",
            );
            let before = managed_generation_snapshot(&live).unwrap();
            let music = fixture.path("Music");
            let album = music.join("Artista/Álbum Com Espaço");
            write(&album.join("imagem única.FLAC"), b"one");
            write(&album.join("segunda.opus"), b"two");
            write(
                &album.join("single.cue"),
                b"FILE \"imagem \xC3\xBAnica.FLAC\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
            );
            write(
                &album.join("multi.CUE"),
                b"FILE \"imagem \xC3\xBAnica.FLAC\" WAVE\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE segunda.opus WAVE\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n",
            );

            let result = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[old, music],
                "cue-addition",
            ));
            let published = result.staging_path.join("Music/Artista/Álbum Com Espaço");

            assert_eq!(
                plan_added_sources_refresh_diagnostic(&live, &result).unwrap(),
                (
                    Some(ManagedRefreshPlan::Targeted {
                        directories: vec!["Music".to_string()],
                        changed_files: 4,
                    }),
                    "none",
                )
            );
            assert_eq!(managed_generation_snapshot(&live).unwrap(), before);

            assert_link(&published.join("single.cue"), &album.join("single.cue"));
            assert_link(&published.join("multi.CUE"), &album.join("multi.CUE"));
            assert_link(
                &published.join("imagem única.FLAC"),
                &album.join("imagem única.FLAC"),
            );
            assert_link(&published.join("segunda.opus"), &album.join("segunda.opus"));
        }

        #[test]
        fn unsafe_or_incomplete_cues_are_omitted_without_hiding_allowed_audio() {
            let fixture = Fixture::new("cue-invalid");
            let old = fixture.path("Old");
            write(&old.join("old.flac"), b"old");
            let live = build_generation(
                &fixture,
                "library",
                &fixture.path("missing-live"),
                std::slice::from_ref(&old),
                "previous",
            );
            let music = fixture.path("Music");
            let album = music.join("Album");
            let outside = fixture.path("outside.flac");
            write(&album.join("good.flac"), b"good");
            write(&album.join("ignored.prg"), b"ignored");
            write(&outside, b"outside");
            symlink(&outside, album.join("linked.flac")).unwrap();
            let invalid = [
                ("missing.cue", "FILE missing.flac WAVE\n"),
                ("ignored.cue", "FILE ignored.prg WAVE\n"),
                ("absolute.cue", "FILE /tmp/outside.flac WAVE\n"),
                ("parent.cue", "FILE ../outside.flac WAVE\n"),
                ("symlink.cue", "FILE linked.flac WAVE\n"),
                ("url.cue", "FILE \"https://example.test/a.flac\" WAVE\n"),
                ("windows.cue", "FILE \"C:\\outside.flac\" WAVE\n"),
                ("dot.cue", "FILE ./good.flac WAVE\n"),
                ("syntax.cue", "FILE \"unterminated.flac WAVE\n"),
                ("empty.cue", "TRACK 01 AUDIO\n"),
            ];
            for (name, content) in invalid {
                write(&album.join(name), content.as_bytes());
            }

            let result = expect_prepared(prepare_managed_generation(
                &live,
                &fixture.path("library.next"),
                &[old, music],
                "invalid-cue-addition",
            ));
            let published = result.staging_path.join("Music/Album");

            assert_eq!(
                plan_added_sources_refresh_diagnostic(&live, &result).unwrap(),
                (
                    Some(ManagedRefreshPlan::Targeted {
                        directories: vec!["Music".to_string()],
                        changed_files: 1,
                    }),
                    "none",
                )
            );
            assert_link(&published.join("good.flac"), &album.join("good.flac"));
            for (name, _) in invalid {
                assert!(!published.join(name).exists(), "{name}");
            }
        }

        #[test]
        fn manifest_roundtrip_rejects_corruption_unknown_schema_and_missing_marker() {
            let fixture = Fixture::new("manifest");
            let generation = fixture.path("generation");
            fs::create_dir(&generation).unwrap();
            let physical = fs::canonicalize(&fixture.root).unwrap();
            let manifest = MirrorManifest {
                schema_version: 1,
                registry_id: None,
                generation_id: "generation-1".to_string(),
                sources: BTreeMap::from([("Music".to_string(), physical.clone())]),
            };

            write_manifest(&generation, &manifest).unwrap();
            assert_eq!(read_manifest(&generation).unwrap(), manifest);
            assert!(is_managed_generation(&generation));

            write(&generation.join("unexpected.txt"), b"unexpected");
            assert!(read_manifest(&generation).is_ok());
            assert!(!is_managed_generation(&generation));
            fs::remove_file(generation.join("unexpected.txt")).unwrap();
            assert!(is_managed_generation(&generation));

            let missing = fixture.path("missing-marker");
            fs::create_dir(&missing).unwrap();
            assert!(!is_managed_generation(&missing));

            fs::write(generation.join(MANIFEST_NAME), b"not-json").unwrap();
            assert!(read_manifest(&generation).is_err());
            assert!(!is_managed_generation(&generation));
            fs::write(
                generation.join(MANIFEST_NAME),
                format!(
                    "{{\"schema_version\":99,\"generation_id\":\"x\",\"sources\":{{\"Music\":{:?}}}}}",
                    physical.to_string_lossy()
                ),
            )
            .unwrap();
            assert!(read_manifest(&generation).is_err());
            assert!(!is_managed_generation(&generation));
        }

        #[test]
        fn manifest_name_is_reserved_and_new_source_uses_a_safe_suffix() {
            let fixture = Fixture::new("reserved-id");
            let reserved_root = fixture.path(MANIFEST_NAME);
            write(&reserved_root.join("track.flac"), b"track");

            assert!(valid_source_id(MANIFEST_NAME).is_none());
            let result = fixture.build(std::slice::from_ref(&reserved_root)).unwrap();
            assert!(!result.manifest.sources.contains_key(MANIFEST_NAME));
            assert_eq!(
                result.manifest.sources[".sonante-mirror.json (2)"],
                fs::canonicalize(&reserved_root).unwrap()
            );

            let invalid_manifest = MirrorManifest {
                schema_version: 1,
                registry_id: None,
                generation_id: "reserved-id".to_string(),
                sources: BTreeMap::from([(
                    MANIFEST_NAME.to_string(),
                    fs::canonicalize(&reserved_root).unwrap(),
                )]),
            };
            assert!(validate_manifest(&invalid_manifest).is_err());
        }

        #[test]
        fn atomic_exchange_and_second_exchange_restore_exact_tree_identity() {
            let fixture = Fixture::new("exchange");
            let left = fixture.path("left");
            let right = fixture.path("right");
            fs::create_dir(&left).unwrap();
            fs::create_dir(&right).unwrap();
            write(&left.join("left.txt"), b"left");
            write(&right.join("right.txt"), b"right");
            let left_inode = fs::metadata(&left).unwrap().ino();
            let right_inode = fs::metadata(&right).unwrap().ino();

            atomic_exchange(&left, &right).unwrap();
            assert_eq!(fs::metadata(&left).unwrap().ino(), right_inode);
            assert_eq!(fs::read(left.join("right.txt")).unwrap(), b"right");
            assert_eq!(fs::metadata(&right).unwrap().ino(), left_inode);

            rollback_exchange(&left, &right).unwrap();
            assert_eq!(fs::metadata(&left).unwrap().ino(), left_inode);
            assert_eq!(fs::read(left.join("left.txt")).unwrap(), b"left");
            assert_eq!(fs::metadata(&right).unwrap().ino(), right_inode);
            assert!(atomic_exchange(&left, &fixture.path("absent")).is_err());
            assert!(left.join("left.txt").exists());
            assert!(atomic_exchange(&left, &left).is_err());
            assert!(
                rename_exchange_error(std::io::Error::from_raw_os_error(libc::ENOSYS))
                    .contains("não é suportado")
            );
            assert!(
                rename_exchange_error(std::io::Error::from_raw_os_error(libc::EXDEV))
                    .contains("mesmo filesystem")
            );
        }

        #[test]
        fn cleanup_requires_manifest_and_unlinks_without_touching_physical_target() {
            let fixture = Fixture::new("cleanup");
            let physical = fixture.path("physical.flac");
            let external_directory = fixture.path("external-directory");
            write(&physical, b"physical");
            write(&external_directory.join("keep.txt"), b"external");
            let unmanaged = fixture.path("unmanaged");
            fs::create_dir(&unmanaged).unwrap();
            assert!(cleanup_managed_generation(&unmanaged).is_err());
            assert!(unmanaged.exists());

            let generation = fixture.path("generation");
            fs::create_dir_all(generation.join("Music/Album")).unwrap();
            symlink(&physical, generation.join("Music/Album/track.flac")).unwrap();
            symlink(
                &external_directory,
                generation.join("Music/Album/external-directory"),
            )
            .unwrap();
            let manifest = MirrorManifest {
                schema_version: 1,
                registry_id: None,
                generation_id: "cleanup-test".to_string(),
                sources: BTreeMap::from([(
                    "Music".to_string(),
                    fs::canonicalize(&fixture.root).unwrap(),
                )]),
            };
            write_manifest(&generation, &manifest).unwrap();

            cleanup_managed_generation(&generation).unwrap();

            assert!(!generation.exists());
            assert_eq!(fs::read(&physical).unwrap(), b"physical");
            assert_eq!(
                fs::read(external_directory.join("keep.txt")).unwrap(),
                b"external"
            );
        }

        #[test]
        fn cleanup_managed_removes_manifest_only_after_source_contents() {
            let fixture = Fixture::new("cleanup-manifest-last");
            let physical = fixture.path("physical.flac");
            write(&physical, b"physical");
            let generation = fixture.path("generation");
            fs::create_dir_all(generation.join("Music/Album")).unwrap();
            symlink(&physical, generation.join("Music/Album/track.flac")).unwrap();
            write_manifest(
                &generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "manifest-last".to_string(),
                    sources: BTreeMap::from([(
                        "Music".to_string(),
                        fs::canonicalize(&fixture.root).unwrap(),
                    )]),
                },
            )
            .unwrap();

            let mut order = Vec::new();
            cleanup_managed_generation_with_hooks(
                &generation,
                None,
                &mut |_| Ok(()),
                &mut |name| {
                    order.push(name.to_os_string());
                    if name == OsStr::new(MANIFEST_NAME) {
                        assert!(!generation.join("Music").exists());
                    } else {
                        assert!(generation.join(MANIFEST_NAME).exists());
                    }
                    Ok(())
                },
            )
            .unwrap();

            assert_eq!(order.last(), Some(&OsString::from(MANIFEST_NAME)));
            assert!(!generation.exists());
            assert_eq!(fs::read(&physical).unwrap(), b"physical");
        }

        #[test]
        fn cleanup_managed_retries_after_partial_removal_with_manifest_intact() {
            let fixture = Fixture::new("cleanup-retry");
            let physical = fixture.path("physical.flac");
            write(&physical, b"physical");
            let generation = fixture.path("generation");
            fs::create_dir_all(generation.join("Music/Album")).unwrap();
            symlink(&physical, generation.join("Music/Album/a.flac")).unwrap();
            symlink(&physical, generation.join("Music/Album/z.flac")).unwrap();
            write_manifest(
                &generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "retry".to_string(),
                    sources: BTreeMap::from([(
                        "Music".to_string(),
                        fs::canonicalize(&fixture.root).unwrap(),
                    )]),
                },
            )
            .unwrap();

            let result = cleanup_managed_generation_with_hooks(
                &generation,
                None,
                &mut |_| Ok(()),
                &mut |name| {
                    if name == OsStr::new("z.flac") {
                        return Err("falha simulada durante remoção".to_string());
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(!generation.join("Music/Album/a.flac").exists());
            assert!(generation.join("Music/Album/z.flac").exists());
            assert!(generation.join(MANIFEST_NAME).exists());
            assert_eq!(fs::read(&physical).unwrap(), b"physical");

            cleanup_managed_generation(&generation).unwrap();
            assert!(!generation.exists());
            assert_eq!(fs::read(&physical).unwrap(), b"physical");
        }

        #[test]
        fn cleanup_managed_rejects_swapped_source_during_removal() {
            let fixture = Fixture::new("cleanup-removal-swap");
            let external = fixture.path("external");
            write(&external.join("keep.txt"), b"external");
            let generation = fixture.path("generation");
            let source = generation.join("Music");
            fs::create_dir_all(source.join("Album")).unwrap();
            let displaced = fixture.path("displaced-source");
            write_manifest(
                &generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "removal-swap".to_string(),
                    sources: BTreeMap::from([(
                        "Music".to_string(),
                        fs::canonicalize(&fixture.root).unwrap(),
                    )]),
                },
            )
            .unwrap();

            let result = cleanup_managed_generation_with_hooks(
                &generation,
                None,
                &mut |_| Ok(()),
                &mut |name| {
                    if name == OsStr::new("Music") {
                        fs::rename(&source, &displaced).unwrap();
                        symlink(&external, &source).unwrap();
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(generation.join(MANIFEST_NAME).exists());
            assert!(fs::symlink_metadata(&source).unwrap().file_type().is_symlink());
            assert!(displaced.exists());
            assert_eq!(fs::read(external.join("keep.txt")).unwrap(), b"external");
        }

        #[test]
        fn cleanup_managed_validates_unknown_entry_before_any_unlink() {
            let fixture = Fixture::new("cleanup-unknown-before-unlink");
            let physical = fixture.path("physical.flac");
            write(&physical, b"physical");
            let generation = fixture.path("generation");
            fs::create_dir_all(generation.join("Music")).unwrap();
            symlink(&physical, generation.join("Music/track.flac")).unwrap();
            write(&generation.join("unknown"), b"keep");
            write_manifest(
                &generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "unknown-before-unlink".to_string(),
                    sources: BTreeMap::from([(
                        "Music".to_string(),
                        fs::canonicalize(&fixture.root).unwrap(),
                    )]),
                },
            )
            .unwrap();

            assert!(cleanup_managed_generation(&generation).is_err());
            assert!(generation.join(MANIFEST_NAME).exists());
            assert!(generation.join("Music/track.flac").exists());
            assert_eq!(fs::read(generation.join("unknown")).unwrap(), b"keep");
            assert_eq!(fs::read(&physical).unwrap(), b"physical");
        }

        #[test]
        fn cleanup_managed_keeps_manifest_if_unknown_entry_appears_during_removal() {
            let fixture = Fixture::new("cleanup-late-unknown");
            let generation = fixture.path("generation");
            fs::create_dir_all(generation.join("Music")).unwrap();
            write_manifest(
                &generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "late-unknown".to_string(),
                    sources: BTreeMap::from([(
                        "Music".to_string(),
                        fs::canonicalize(&fixture.root).unwrap(),
                    )]),
                },
            )
            .unwrap();

            let result = cleanup_managed_generation_with_hooks(
                &generation,
                None,
                &mut |_| Ok(()),
                &mut |name| {
                    if name == OsStr::new("Music") {
                        write(&generation.join("unknown"), b"keep");
                    }
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(!generation.join("Music").exists());
            assert!(generation.join(MANIFEST_NAME).exists());
            assert_eq!(fs::read(generation.join("unknown")).unwrap(), b"keep");
        }

        #[test]
        fn cleanup_rejects_invalid_structure_and_root_symlink() {
            let fixture = Fixture::new("cleanup-rejections");
            let physical_root = fs::canonicalize(&fixture.root).unwrap();

            let invalid_manifest = fixture.path("invalid-manifest");
            fs::create_dir(&invalid_manifest).unwrap();
            write(&invalid_manifest.join(MANIFEST_NAME), b"not-json");
            assert!(cleanup_managed_generation(&invalid_manifest).is_err());
            assert!(invalid_manifest.exists());

            let unexpected_regular = fixture.path("unexpected-regular");
            fs::create_dir_all(unexpected_regular.join("Music/Album")).unwrap();
            write(
                &unexpected_regular.join("Music/Album/unexpected.bin"),
                b"unexpected",
            );
            write_manifest(
                &unexpected_regular,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "unexpected-regular".to_string(),
                    sources: BTreeMap::from([("Music".to_string(), physical_root.clone())]),
                },
            )
            .unwrap();
            assert!(cleanup_managed_generation(&unexpected_regular).is_err());
            assert_eq!(
                fs::read(unexpected_regular.join("Music/Album/unexpected.bin")).unwrap(),
                b"unexpected"
            );

            let actual_generation = fixture.path("actual-generation");
            fs::create_dir(&actual_generation).unwrap();
            write_manifest(
                &actual_generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "actual-generation".to_string(),
                    sources: BTreeMap::new(),
                },
            )
            .unwrap();
            let root_link = fixture.path("generation-link");
            symlink(&actual_generation, &root_link).unwrap();
            assert!(cleanup_managed_generation(&root_link).is_err());
            assert!(fs::symlink_metadata(&root_link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(actual_generation.join(MANIFEST_NAME).exists());
        }

        #[test]
        fn cleanup_directory_swap_to_symlink_fails_without_following_target() {
            let fixture = Fixture::new("cleanup-race");
            let external = fixture.path("external");
            write(&external.join("keep.txt"), b"external");
            let generation = fixture.path("generation");
            let album = generation.join("Music/Album");
            fs::create_dir_all(&album).unwrap();
            let displaced = fixture.path("displaced-album");
            write_manifest(
                &generation,
                &MirrorManifest {
                    schema_version: 1,
                registry_id: None,
                    generation_id: "race".to_string(),
                    sources: BTreeMap::from([(
                        "Music".to_string(),
                        fs::canonicalize(&fixture.root).unwrap(),
                    )]),
                },
            )
            .unwrap();

            let mut swapped = false;
            let result = cleanup_managed_generation_with_hook(&generation, None, &mut |name| {
                if name == OsStr::new("Album") {
                    fs::rename(&album, &displaced).unwrap();
                    symlink(&external, &album).unwrap();
                    swapped = true;
                }
                Ok(())
            });

            assert!(swapped);
            assert!(result.is_err());
            assert_eq!(fs::read(external.join("keep.txt")).unwrap(), b"external");
            assert!(fs::symlink_metadata(&album)
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(displaced.exists());
        }

        #[test]
        fn staging_and_root_failures_do_not_replace_or_delete_existing_paths() {
            let fixture = Fixture::new("failures");
            let music = fixture.path("Music");
            write(&music.join("track.flac"), b"track");
            let staging = fixture.path("library.next");
            fs::create_dir(&staging).unwrap();
            write(&staging.join("keep"), b"keep");

            assert!(fixture.build(std::slice::from_ref(&music)).is_err());
            assert_eq!(fs::read(staging.join("keep")).unwrap(), b"keep");
            fs::remove_dir_all(&staging).unwrap();

            let missing = fixture.path("removed-root");
            assert!(fixture.build(&[missing]).is_err());
            assert!(!staging.exists());

            let live = fixture.path("library");
            assert!(build_filtered_staging(
                &live,
                &live,
                std::slice::from_ref(&music),
                "same-path"
            )
            .is_err());
            assert!(!live.exists());

            let removable = fixture.path("Removable");
            write(&removable.join("track.flac"), b"track");
            let result = build_filtered_staging_with_hook(
                &staging,
                &fixture.path("library"),
                std::slice::from_ref(&removable),
                "removed-during-build",
                || {
                    fs::remove_dir_all(&removable).unwrap();
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(!staging.exists());
        }

        #[test]
        fn inaccessible_nested_directory_fails_closed_when_permissions_are_enforced() {
            let fixture = Fixture::new("permissions");
            let music = fixture.path("Music");
            let denied = music.join("Denied");
            fs::create_dir_all(&denied).unwrap();
            write(&denied.join("track.flac"), b"track");
            let original_mode = fs::metadata(&denied).unwrap().permissions().mode();
            fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();

            let result = fixture.build(std::slice::from_ref(&music));

            fs::set_permissions(&denied, fs::Permissions::from_mode(original_mode)).unwrap();
            if result.is_err() {
                assert!(!fixture.path("library.next").exists());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_RESOLVER_ID: AtomicU64 = AtomicU64::new(0);

    struct ResolverFixture {
        base: PathBuf,
        library: PathBuf,
    }

    impl ResolverFixture {
        fn new(name: &str) -> Self {
            let id = NEXT_RESOLVER_ID.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "sonante-media-resolver-{}-{name}-{id}",
                std::process::id()
            ));
            let library = base.join("library");
            fs::create_dir_all(&library).unwrap();
            Self { base, library }
        }

        fn folders(roots: &[&Path]) -> Vec<String> {
            roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect()
        }

        fn publish_managed(&self, roots: &[PathBuf]) {
            let staging = self.base.join("library.next");
            mirror::build_filtered_staging(
                &staging,
                &self.library,
                roots,
                "resolver-generation",
            )
            .unwrap();
            for entry in fs::read_dir(&self.library).unwrap() {
                let path = entry.unwrap().path();
                if fs::symlink_metadata(&path).unwrap().file_type().is_symlink() {
                    fs::remove_file(path).unwrap();
                } else {
                    fs::remove_dir_all(path).unwrap();
                }
            }
            fs::remove_dir(&self.library).unwrap();
            fs::rename(staging, &self.library).unwrap();
        }
    }

    impl Drop for ResolverFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).unwrap();
        }
    }

    #[test]
    fn resolver_supports_legacy_and_managed_with_unicode_spaces_and_files() {
        let fixture = ResolverFixture::new("dual-topology");
        let root = fixture.base.join("Música Local");
        let track = root.join("Artista/Álbum/01 Faixa.FLAC");
        fs::create_dir_all(track.parent().unwrap()).unwrap();
        fs::write(&track, b"audio").unwrap();
        symlink(&root, fixture.library.join("Música Local")).unwrap();
        let folders = ResolverFixture::folders(&[&root]);

        let legacy = resolve_local_sources(&fixture.library, &folders).unwrap();
        assert_eq!(legacy.topology(), LocalLibraryTopology::Legacy);
        assert_eq!(
            resolve_local_path(
                &fixture.library,
                "Música Local/Artista/Álbum/01 Faixa.FLAC",
                &folders,
            )
            .unwrap()
            .physical_path,
            track
        );

        fixture.publish_managed(std::slice::from_ref(&root));
        let managed = resolve_local_sources(&fixture.library, &folders).unwrap();
        assert_eq!(managed.topology(), LocalLibraryTopology::Managed);
        assert_eq!(
            resolve_local_path(
                &fixture.library,
                "Música Local/Artista/Álbum/01 Faixa.FLAC",
                &folders,
            )
            .unwrap()
            .physical_path,
            track
        );
    }

    #[test]
    fn managed_mapping_survives_reordering_and_filters_removed_roots() {
        let fixture = ResolverFixture::new("managed-mapping");
        let first = fixture.base.join("one/Music");
        let second = fixture.base.join("two/Music");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fixture.publish_managed(&[first.clone(), second.clone()]);
        let reordered = ResolverFixture::folders(&[&second, &first]);

        let sources = resolve_local_sources(&fixture.library, &reordered).unwrap();
        assert_eq!(sources.sources().get("Music"), Some(&first));
        assert_eq!(sources.sources().get("Music (2)"), Some(&second));

        let only_first = ResolverFixture::folders(&[&first]);
        let sources = resolve_local_sources(&fixture.library, &only_first).unwrap();
        assert_eq!(sources.sources().len(), 1);
        assert_eq!(sources.sources().get("Music"), Some(&first));
        assert_eq!(
            resolve_local_path(&fixture.library, "Music (2)", &only_first),
            Err(LocalPathResolveError::SourceNotFound)
        );
    }

    #[test]
    fn resolver_accepts_colons_in_legacy_and_managed_linux_paths() {
        let fixture = ResolverFixture::new("colon-paths");
        let root = fixture.base.join("Music: Hi-Res");
        let track = root.join("Artist/01: Intro.flac");
        fs::create_dir_all(track.parent().unwrap()).unwrap();
        fs::write(&track, b"audio").unwrap();
        symlink(&root, fixture.library.join("Music: Hi-Res")).unwrap();
        let folders = ResolverFixture::folders(&[&root]);

        assert_eq!(
            resolve_local_path(&fixture.library, "Music: Hi-Res", &folders)
                .unwrap()
                .physical_path,
            root
        );
        assert_eq!(
            resolve_local_path(
                &fixture.library,
                "Music: Hi-Res/Artist/01: Intro.flac",
                &folders,
            )
            .unwrap()
            .physical_path,
            track
        );

        fixture.publish_managed(std::slice::from_ref(&root));
        assert_eq!(
            resolve_local_path(&fixture.library, "Music: Hi-Res", &folders)
                .unwrap()
                .physical_path,
            root
        );
        assert_eq!(
            resolve_local_path(
                &fixture.library,
                "Music: Hi-Res/Artist/01: Intro.flac",
                &folders,
            )
            .unwrap()
            .physical_path,
            track
        );
    }

    #[test]
    fn resolver_fails_closed_for_manifest_mismatch_and_corruption() {
        let fixture = ResolverFixture::new("managed-trust");
        let root = fixture.base.join("Music");
        let added = fixture.base.join("Added");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&added).unwrap();
        fixture.publish_managed(std::slice::from_ref(&root));

        let mismatch = ResolverFixture::folders(&[&root, &added]);
        assert!(matches!(
            resolve_local_sources(&fixture.library, &mismatch),
            Err(LocalPathResolveError::Unavailable(_))
        ));

        fs::write(
            fixture.library.join(mirror::MANIFEST_NAME),
            b"not json",
        )
        .unwrap();
        let folders = ResolverFixture::folders(&[&root]);
        assert!(matches!(
            resolve_local_sources(&fixture.library, &folders),
            Err(LocalPathResolveError::Unavailable(_))
        ));
    }

    #[test]
    fn resolver_rejects_unknown_sources_unsafe_paths_and_symlink_escape() {
        let fixture = ResolverFixture::new("path-validation");
        let root = fixture.base.join("Music");
        let outside = fixture.base.join("Outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&root, fixture.library.join("Music")).unwrap();
        symlink(&outside, root.join("Escape")).unwrap();
        let folders = ResolverFixture::folders(&[&root]);

        assert_eq!(
            resolve_local_path(&fixture.library, "Unknown", &folders),
            Err(LocalPathResolveError::SourceNotFound)
        );
        assert_eq!(
            resolve_local_path(&fixture.library, "scheme:value", &folders),
            Err(LocalPathResolveError::SourceNotFound)
        );
        assert_eq!(
            resolve_local_path(
                &fixture.library,
                "https://example.invalid/file.flac",
                &folders,
            ),
            Err(LocalPathResolveError::InvalidPath)
        );
        for invalid in [
            "",
            "/Music",
            "Music/.",
            "Music/..",
            "Music//Album",
            "Music/bad\0name",
            "Music/bad\rname",
            "Music/bad\nname",
            ".sonante-mirror.json",
        ] {
            assert!(resolve_local_path(&fixture.library, invalid, &folders).is_err(), "{invalid:?}");
        }
        assert_eq!(
            resolve_local_path(&fixture.library, "Music/Escape", &folders),
            Err(LocalPathResolveError::OutsideRoot)
        );
    }

    #[test]
    fn classifies_the_exact_playable_audio_allowlist() {
        for extension in [
            "flac", "mp3", "wav", "aif", "aiff", "ogg", "oga", "opus", "aac", "m4a", "mp4", "wv",
            "mpc", "dsf", "dff",
        ] {
            assert_eq!(
                classify_path(Path::new(&format!("track.{extension}"))),
                LocalMediaClass::PlayableAudio,
                "{extension}"
            );
        }
        for extension in ["FLAC", "FlAc", "MP3", "OpUs", "DSF", "DfF", "MP4", "M4a"] {
            assert_eq!(
                classify_path(Path::new(&format!("track.{extension}"))),
                LocalMediaClass::PlayableAudio,
                "{extension}"
            );
        }
    }

    #[test]
    fn classifies_control_and_artwork_sidecars_case_insensitively() {
        for extension in ["cue", "CUE", "CuE"] {
            assert_eq!(
                classify_path(Path::new(&format!("image.{extension}"))),
                LocalMediaClass::ControlSidecar
            );
        }
        for extension in ["jpg", "jpeg", "png", "webp", "JPG", "JpEg", "PNG", "WeBp"] {
            assert_eq!(
                classify_path(Path::new(&format!("cover.{extension}"))),
                LocalMediaClass::ArtworkSidecar,
                "{extension}"
            );
        }
    }

    #[test]
    fn ignores_everything_outside_the_product_policy() {
        for path in [
            "track.prg",
            "disc.iso",
            "track.wave",
            "track.aifc",
            "track.ape",
            "track.tta",
            "track.tak",
            "track.wma",
            "track.caf",
            "track.mka",
            "track.unknown",
            "README",
            "empty.",
        ] {
            assert_eq!(
                classify_path(Path::new(path)),
                LocalMediaClass::Ignored,
                "{path}"
            );
        }

        let non_utf8 = OsString::from_vec(b"track.\xff".to_vec());
        assert_eq!(
            classify_path(Path::new(&non_utf8)),
            LocalMediaClass::Ignored
        );
    }

    #[test]
    fn catalog_tracks_are_playable_files_or_existing_virtual_cue_shape() {
        for uri in [
            "Music/Artist/Album/track.flac",
            "Music/Artist/Album/track.MP3",
            "Music/Artist/Album/image.cue/track0001",
            "Music/Artist/Album/IMAGE.CUE/track12",
        ] {
            assert!(is_catalog_track_uri(uri), "{uri}");
        }
        for uri in [
            "Music/Artist/Album/image.cue",
            "Music/Artist/Album/cover.jpg",
            "Music/Artist/Album/program.prg",
            "Music/Artist/Album/image.cue/track",
            "Music/Artist/Album/image.cue/foo0001",
            "Album.cue/track0001",
        ] {
            assert!(!is_catalog_track_uri(uri), "{uri}");
        }
    }
}
