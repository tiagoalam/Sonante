//! Tauri commands that back the original Sonante Local library with SQLite.
//! The catalog is isolated under the 0.6.4 development profile.

use crate::audio::{self, AudioState, MediaLocator};
use crate::library_doctor::{self, ArtworkStatus, DoctorError};
use crate::tags;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sonante_sqlite_catalog::{
    disc_number_from_path, Album, AlbumId, AlbumTrack, Artist, Catalog, IndexPhase, IndexProgress,
    Metrics, Source, SourceLibraryView, TrackMetadata, TrackMetadataValues, VerifiedTrackFile,
};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqliteCueRangePlayback {
    pub(crate) path: String,
    pub(crate) start_frame: u64,
    pub(crate) end_frame: Option<u64>,
}

fn sqlite_cue_range_uri(path: &str, start_frame: u64, end_frame: Option<u64>) -> Result<String, String> {
    if !Path::new(path).is_absolute() || path.contains(['\0', '\r', '\n'])
        || end_frame.is_some_and(|end| end <= start_frame)
    {
        return Err("SQLITE_CUE_PATH_NOT_MPD_COMPATIBLE".into());
    }
    Ok(format!(
        "sonante-cue-range://{}/{}/{}",
        URL_SAFE_NO_PAD.encode(path.as_bytes()),
        start_frame,
        end_frame.map(|end| end.to_string()).unwrap_or_else(|| "open".into()),
    ))
}

#[derive(Clone)]
pub struct SqliteLibraryState {
    inner: Arc<Inner>,
}

struct Inner {
    catalog: Mutex<Catalog>,
    database_path: PathBuf,
    active_view: Mutex<Option<Arc<SourceLibraryView>>>,
    source_views: Mutex<HashMap<i64, Arc<SourceLibraryView>>>,
    selected_roots: Mutex<HashMap<String, PathBuf>>,
    next_selection: AtomicU64,
    legacy_source_migration: Mutex<()>,
}

const LEGACY_SOURCE_LEDGER_VERSION: u8 = 1;
const LEGACY_SOURCE_LEDGER_MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySourceImportLedger {
    version: u8,
    #[serde(default)]
    imported_root_hashes: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySourceMigrationStatusDto {
    pub eligible: bool,
    pub pending_sources: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySourceImportResultDto {
    pub imported_sources: usize,
    pub already_present_sources: usize,
    pub failed_source_names: Vec<String>,
}

pub(crate) struct SqliteAbTrack {
    pub(crate) file: Arc<VerifiedTrackFile>,
    pub(crate) view: Arc<SourceLibraryView>,
    pub(crate) relative_path: PathBuf,
    pub(crate) cue: Option<sonante_sqlite_catalog::CueVirtualTrack>,
}

impl SqliteLibraryState {
    pub fn open(database_path: impl AsRef<Path>) -> Result<Self, String> {
        let database_path = database_path.as_ref().to_path_buf();
        let catalog = Catalog::open(&database_path)?;
        Ok(Self {
            inner: Arc::new(Inner {
                catalog: Mutex::new(catalog),
                database_path,
                active_view: Mutex::new(None),
                source_views: Mutex::new(HashMap::new()),
                selected_roots: Mutex::new(HashMap::new()),
                next_selection: AtomicU64::new(1),
                legacy_source_migration: Mutex::new(()),
            }),
        })
    }

    fn catalog(&self) -> Result<MutexGuard<'_, Catalog>, String> {
        self.inner
            .catalog
            .lock()
            .map_err(|_| "SQLite catalog is unavailable".to_owned())
    }

    fn legacy_source_ledger_path(&self) -> Result<PathBuf, String> {
        self.inner.database_path.parent()
            .map(|parent| parent.join("legacy-source-import-v1.json"))
            .ok_or_else(|| "SQLITE_CATALOG_PARENT_UNAVAILABLE".into())
    }

    fn read_legacy_source_ledger(&self) -> Result<LegacySourceImportLedger, String> {
        let path = self.legacy_source_ledger_path()?;
        crate::persistence::prepare_private_file_for_load(&path, "legacy source import ledger")?;
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LegacySourceImportLedger { version: LEGACY_SOURCE_LEDGER_VERSION, ..Default::default() });
            }
            Err(_) => return Err("SQLITE_LEGACY_SOURCE_LEDGER_READ_FAILED".into()),
        };
        if !metadata.is_file() || metadata.len() > LEGACY_SOURCE_LEDGER_MAX_BYTES {
            return Err("SQLITE_LEGACY_SOURCE_LEDGER_INVALID".into());
        }
        let bytes = std::fs::read(&path).map_err(|_| "SQLITE_LEGACY_SOURCE_LEDGER_READ_FAILED")?;
        let ledger: LegacySourceImportLedger = serde_json::from_slice(&bytes)
            .map_err(|_| "SQLITE_LEGACY_SOURCE_LEDGER_INVALID")?;
        if ledger.version != LEGACY_SOURCE_LEDGER_VERSION
            || ledger.imported_root_hashes.len() > 4096
            || ledger.imported_root_hashes.iter().any(|hash| hash.len() != 43 || !hash.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
        {
            return Err("SQLITE_LEGACY_SOURCE_LEDGER_INVALID".into());
        }
        Ok(ledger)
    }

    fn write_legacy_source_ledger(&self, ledger: &LegacySourceImportLedger) -> Result<(), String> {
        let path = self.legacy_source_ledger_path()?;
        let bytes = serde_json::to_vec(ledger).map_err(|_| "SQLITE_LEGACY_SOURCE_LEDGER_WRITE_FAILED")?;
        if bytes.len() as u64 > LEGACY_SOURCE_LEDGER_MAX_BYTES {
            return Err("SQLITE_LEGACY_SOURCE_LEDGER_FULL".into());
        }
        crate::persistence::atomic_write_private_synced(&path, &bytes, "legacy source import ledger")
            .map_err(|_| "SQLITE_LEGACY_SOURCE_LEDGER_WRITE_FAILED".into())
    }

    fn legacy_root_hash(root: &str) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        URL_SAFE_NO_PAD.encode(Sha256::digest(root.as_bytes()))
    }

    fn legacy_root_name(root: &str) -> String {
        Path::new(root).file_name().map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty()).unwrap_or_else(|| "Biblioteca Local".into())
    }

    fn migration_roots(roots: &[String]) -> Vec<String> {
        let mut seen = BTreeSet::new();
        roots.iter().filter(|root| !root.trim().is_empty() && seen.insert(Self::legacy_root_hash(root)))
            .cloned().collect()
    }

    fn legacy_source_migration_status(&self, roots: &[String]) -> Result<LegacySourceMigrationStatusDto, String> {
        let _migration = self.inner.legacy_source_migration.lock()
            .map_err(|_| "SQLITE_LEGACY_SOURCE_MIGRATION_BUSY")?;
        let mut ledger = self.read_legacy_source_ledger()?;
        let sources = self.catalog()?.sources()?;
        let mut changed = false;
        let roots = Self::migration_roots(roots);
        for root in &roots {
            let hash = Self::legacy_root_hash(root);
            if ledger.imported_root_hashes.contains(&hash) { continue; }
            // Exact equality is enough to recognize a catalog record even if
            // its network mount is currently offline. Do not probe or scan it.
            if sources.iter().any(|source| source.canonical_path == PathBuf::from(root)) {
                ledger.imported_root_hashes.insert(hash);
                changed = true;
            }
        }
        if changed { self.write_legacy_source_ledger(&ledger)?; }
        let pending_sources = roots.iter().filter(|root| !ledger.imported_root_hashes.contains(&Self::legacy_root_hash(root))).count();
        Ok(LegacySourceMigrationStatusDto { eligible: !roots.is_empty(), pending_sources })
    }

    fn import_legacy_sources_with_progress<F>(
        &self,
        roots: &[String],
        mut progress: F,
    ) -> Result<LegacySourceImportResultDto, String>
    where F: FnMut(&str, IndexProgress) {
        let _migration = self.inner.legacy_source_migration.lock()
            .map_err(|_| "SQLITE_LEGACY_SOURCE_MIGRATION_BUSY")?;
        let roots = Self::migration_roots(roots);
        let mut ledger = self.read_legacy_source_ledger()?;
        let mut result = LegacySourceImportResultDto { imported_sources: 0, already_present_sources: 0, failed_source_names: Vec::new() };

        for root in roots {
            let root_hash = Self::legacy_root_hash(&root);
            if ledger.imported_root_hashes.contains(&root_hash) { continue; }
            let name = Self::legacy_root_name(&root);
            let configured_path = PathBuf::from(&root);
            if !configured_path.is_absolute() {
                result.failed_source_names.push(name);
                continue;
            }
            let sources = self.catalog()?.sources()?;
            if sources.iter().any(|source| source.canonical_path == configured_path) {
                ledger.imported_root_hashes.insert(root_hash);
                self.write_legacy_source_ledger(&ledger)?;
                result.already_present_sources += 1;
                continue;
            }
            let canonical = match std::fs::canonicalize(&configured_path) {
                Ok(path) if path.is_dir() => path,
                _ => { result.failed_source_names.push(name); continue; }
            };
            if sources.iter().any(|source| source.canonical_path == canonical) {
                ledger.imported_root_hashes.insert(root_hash);
                self.write_legacy_source_ledger(&ledger)?;
                result.already_present_sources += 1;
                continue;
            }
            match self.add_and_index_with_progress(&canonical, |event| progress(&name, event)) {
                Ok(_) => {
                    // Catalog publication is already durable. If ledger write
                    // fails, a retry recognizes the catalog root and won't scan it again.
                    ledger.imported_root_hashes.insert(root_hash);
                    self.write_legacy_source_ledger(&ledger)?;
                    result.imported_sources += 1;
                }
                Err(_) => result.failed_source_names.push(name),
            }
        }
        Ok(result)
    }

    /// Captures the current generation for every SQLite locator in a logical
    /// playback request. The caller must later pass this snapshot to
    /// `with_playback_generation_guard`; a changed or removed source then
    /// fails closed before any MPD queue mutation.
    pub(crate) fn playback_source_generations(
        &self,
        tracks: &[audio::TrackMetadata],
    ) -> Result<Vec<(i64, i64)>, String> {
        let mut source_ids = BTreeSet::new();
        for track in tracks {
            let Some(MediaLocator::Local { uri }) = &track.media_locator else { continue };
            if uri.starts_with("sqlite-track://") || uri.starts_with("sqlite-cue://") {
                if !Self::is_sqlite_track_locator(uri) {
                    return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
                }
                source_ids.insert(Self::sqlite_locator_source_id(uri)?
                    .ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?);
            }
        }
        let catalog = self.catalog()?;
        let sources = catalog.sources()?;
        source_ids.into_iter().map(|source_id| {
            sources.iter().find(|source| source.id == source_id)
                .map(|source| (source.id, source.generation))
                .ok_or_else(|| "SQLITE_SOURCE_REMOVED".to_owned())
        }).collect()
    }

    /// Runs the short final binding check and MPD mutation while SQLite holds
    /// an IMMEDIATE writer reservation. Scans remain outside this guard and
    /// only contend at their existing generation-confirmation transaction.
    pub(crate) fn with_playback_generation_guard<T>(
        &self,
        generations: &[(i64, i64)],
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if generations.is_empty() {
            return operation();
        }
        let mut catalog = Catalog::open(&self.inner.database_path)?;
        catalog.with_source_generation_guard(generations, operation)
    }

    fn current_view(&self, source_id: i64) -> Result<Arc<SourceLibraryView>, String> {
        let view = self
            .inner
            .source_views
            .lock()
            .map_err(|_| "SQLite library view is unavailable".to_owned())?
            .get(&source_id)
            .cloned()
            .ok_or_else(|| "VIEW_NOT_LOADED: open this source before browsing".to_owned())?;
        let catalog = self.catalog()?;
        if catalog.source_tag_edit_pending(source_id)? {
            return Err("SOURCE_TAG_EDIT_REQUIRES_RECONCILIATION: verify this source before browsing".into());
        }
        if !catalog.is_source_library_view_current(&view)? {
            return Err("VIEW_STALE: verify the source and open its view again".into());
        }
        Ok(view)
    }

    fn album_doctor(&self, encoded_album_id: &str) -> Result<library_doctor::Report, DoctorError> {
        if encoded_album_id.starts_with("sqlite-cue-album:") {
            let (source_id, _) = parse_sqlite_cue_album_id(encoded_album_id)
                .map_err(|_| DoctorError::AlbumNotFound)?;
            let view = self
                .ensure_source_view(source_id)
                .map_err(|_| DoctorError::CatalogReadFailed)?;
            let group = cue_album_groups(view.cue_tracks())
                .into_iter()
                .find(|group| cue_album_id(source_id, group) == encoded_album_id)
                .ok_or(DoctorError::AlbumNotFound)?;

            // Revalidate each distinct sheet/audio pair. Segment records from
            // one image share the same physical identity and need only one
            // descriptor-anchored check for the pair.
            let mut checked = std::collections::HashSet::new();
            let mut inspections = Vec::with_capacity(group.tracks.len());
            for track in &group.tracks {
                if checked.insert((track.cue_path.clone(), track.audio_path.clone())) {
                    view.verified_cue_track(&track.cue_path, track.track_number)
                        .map_err(|_| DoctorError::CatalogReadFailed)?
                        .ok_or(DoctorError::AlbumNotFound)?;
                }
                let uri = sqlite_cue_locator(source_id, &track.cue_path, track.track_number)
                    .map_err(|_| DoctorError::CatalogReadFailed)?;
                inspections.push(tags::LocalAlbumTrackInspection {
                    uri,
                    kind: tags::LocalAlbumTrackKind::CueVirtual,
                    tags: None,
                    issue: None,
                });
            }

            let artwork = match view.read_source_artwork_in_directory(
                &group.folder,
                audio::LOCAL_FOLDER_COVER_NAMES,
                MAX_SQLITE_COVER_BYTES,
            ) {
                Ok(Some(_)) => ArtworkStatus::External,
                Ok(None) => {
                    let mut status = ArtworkStatus::Missing;
                    let mut seen_audio = std::collections::HashSet::new();
                    for track in group
                        .tracks
                        .iter()
                        .filter(|track| seen_audio.insert(track.audio_path.clone()))
                        .take(8)
                    {
                        match view.read_cue_embedded_artwork(
                            &track.cue_path,
                            track.track_number,
                            MAX_EMBEDDED_SOURCE_BYTES,
                            MAX_SQLITE_COVER_BYTES,
                        ) {
                            Ok(Some(_)) => {
                                status = ArtworkStatus::Embedded;
                                break;
                            }
                            Ok(None) => {}
                            Err(_) => status = ArtworkStatus::Unavailable,
                        }
                    }
                    status
                }
                Err(_) => ArtworkStatus::Unavailable,
            };
            let report = library_doctor::inspect_sqlite_cue_snapshot(
                encoded_album_id,
                inspections,
                artwork,
            )?;
            if !self
                .catalog()
                .map_err(|_| DoctorError::StateUnavailable)?
                .is_source_library_view_current(&view)
                .map_err(|_| DoctorError::CatalogReadFailed)?
            {
                return Err(DoctorError::TaskFailed);
            }
            return Ok(report);
        }
        let mut parts = encoded_album_id.splitn(3, ':');
        if parts.next() != Some("sqlite") {
            return Err(DoctorError::AlbumNotFound);
        }
        let source_id = parts
            .next()
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .ok_or(DoctorError::AlbumNotFound)?;
        let encoded_identity = parts.next().ok_or(DoctorError::AlbumNotFound)?;
        let identity = URL_SAFE_NO_PAD
            .decode(encoded_identity)
            .map_err(|_| DoctorError::AlbumNotFound)?;
        if identity.is_empty() || URL_SAFE_NO_PAD.encode(&identity) != encoded_identity {
            return Err(DoctorError::AlbumNotFound);
        }

        let view = self
            .ensure_source_view(source_id)
            .map_err(|_| DoctorError::CatalogReadFailed)?;
        let album_id = AlbumId {
            source_id,
            identity,
        };
        let album = view
            .albums()
            .iter()
            .find(|album| album.id == album_id)
            .cloned()
            .ok_or(DoctorError::AlbumNotFound)?;
        let tracks = view
            .tracks_for_album(&album_id)
            .ok_or(DoctorError::AlbumNotFound)?
            .cloned()
            .collect::<Vec<_>>();

        // Doctor checks existing sidecars first, then a bounded set of embedded
        // images. Both APIs are descriptor-anchored and verify indexed stamps.
        let artwork = match view.read_album_artwork(
            &album_id,
            &[
                "cover.jpg",
                "cover.jpeg",
                "cover.png",
                "cover.webp",
                "folder.jpg",
                "folder.png",
                "front.jpg",
            ],
            MAX_SQLITE_COVER_BYTES,
        ) {
            Ok(Some(_)) => ArtworkStatus::External,
            Ok(None) => match view.read_album_embedded_artwork(
                &album_id,
                MAX_EMBEDDED_SOURCE_BYTES,
                MAX_SQLITE_COVER_BYTES,
            ) {
                Ok(Some(_)) => ArtworkStatus::Embedded,
                Ok(None) => ArtworkStatus::Missing,
                Err(_) => ArtworkStatus::Unavailable,
            },
            Err(_) => ArtworkStatus::Unavailable,
        };

        let report = library_doctor::inspect_sqlite_snapshot(
            encoded_album_id,
            &album,
            &view,
            &tracks,
            artwork,
        )?;
        if !self
            .catalog()
            .map_err(|_| DoctorError::StateUnavailable)?
            .is_source_library_view_current(&view)
            .map_err(|_| DoctorError::CatalogReadFailed)?
        {
            return Err(DoctorError::TaskFailed);
        }
        Ok(report)
    }

    /// Resolve an opaque SQLite locator to a local MPD path only after
    /// validating source generation, membership, path components, file type,
    /// size and mtime through the scanner's anchored open routine.
    pub(crate) fn resolve_playback_locator(&self, uri: &str) -> Result<String, String> {
        let locator = uri
            .strip_prefix("sqlite-track://")
            .ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
        let (source_text, encoded_path) = locator
            .split_once('/')
            .ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
        if encoded_path.is_empty()
            || encoded_path.contains('/')
            || encoded_path.contains(['?', '#'])
        {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        let source_id = source_text
            .parse::<i64>()
            .ok()
            .filter(|source_id| *source_id > 0)
            .ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
        let encoded = URL_SAFE_NO_PAD
            .decode(encoded_path)
            .map_err(|_| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
        if encoded.is_empty()
            || encoded.contains(&0)
            || URL_SAFE_NO_PAD.encode(&encoded) != encoded_path
        {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        #[cfg(target_os = "linux")]
        let relative = PathBuf::from(std::ffi::OsString::from_vec(encoded));
        #[cfg(not(target_os = "linux"))]
        return Err("SQLITE_TRACK_PLAYBACK_REQUIRES_LINUX_SECURE_SCANNER".into());

        #[cfg(target_os = "linux")]
        {
            if relative.is_absolute()
                || !relative
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
            }
            // Queue restoration can happen before the browser has opened a
            // source. Rebuild only the in-memory view from SQLite; this does
            // not scan the filesystem. The generation check below remains the
            // authority for whether this locator is still current.
            let view = self.ensure_source_view(source_id)?;
            let path = view
                .verified_track_path(&relative)
                .map_err(|_| "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED".to_owned())?
                .ok_or_else(|| "SQLITE_TRACK_NOT_INDEXED".to_owned())?;
            path.to_str()
                .map(str::to_owned)
                .filter(|path| Path::new(path).is_absolute() && !path.contains(['\0', '\r', '\n']))
                .ok_or_else(|| "SQLITE_TRACK_PATH_NOT_MPD_COMPATIBLE".to_owned())
        }
    }

    /// Resolve a CUE locator to the MPD-native virtual URI under the verified
    /// private music-directory mirror. MPD remains responsible for segment
    /// boundaries, duration, seek and queue transitions; this method never
    /// substitutes the complete physical audio file for a virtual track.
    pub(crate) fn resolve_playback_locator_with_roots(
        &self,
        uri: &str,
        configured_roots: &[String],
    ) -> Result<String, String> {
        let Some((source_id, cue_path, track_number)) = parse_sqlite_cue_locator(uri).ok() else {
            return self.resolve_playback_locator(uri);
        };
        #[cfg(not(target_os = "linux"))]
        return Err("SQLITE_CUE_PLAYBACK_REQUIRES_LINUX_SECURE_SCANNER".into());

        #[cfg(target_os = "linux")]
        {
            let view = self.ensure_source_view(source_id)?;
            let cue = view
                .verified_cue_track(&cue_path, track_number)
                .map_err(|_| "SQLITE_CUE_UNAVAILABLE_OR_CHANGED".to_owned())?
                .ok_or_else(|| "SQLITE_CUE_TRACK_NOT_INDEXED".to_owned())?;
            let _ = configured_roots;
            let path = view.verified_track_path(&cue.audio_path)
                .map_err(|_| "SQLITE_CUE_AUDIO_UNAVAILABLE_OR_CHANGED".to_owned())?
                .ok_or_else(|| "SQLITE_CUE_AUDIO_NOT_INDEXED".to_owned())?;
            let physical_path = path.to_str()
                .filter(|value| Path::new(value).is_absolute() && !value.contains(['\0', '\r', '\n']))
                .ok_or_else(|| "SQLITE_CUE_PATH_NOT_MPD_COMPATIBLE".to_owned())?;
            let mpd_uri = sqlite_cue_range_uri(physical_path, cue.start_frame, cue.end_frame)?;
            if !self.catalog()?.is_source_library_view_current(&view)? {
                return Err("VIEW_STALE: verify the source and open its view again".into());
            }
            Ok(mpd_uri)
        }
    }

    pub(crate) fn is_sqlite_track_locator(uri: &str) -> bool {
        #[cfg(target_os = "linux")]
        {
            return parse_sqlite_track_locator(uri).is_ok() || parse_sqlite_cue_locator(uri).is_ok();
        }
        #[cfg(not(target_os = "linux"))]
        {
        let Some(locator) = uri.strip_prefix("sqlite-track://") else {
            return false;
        };
        let Some((source, encoded)) = locator.split_once('/') else {
            return false;
        };
        source.parse::<i64>().is_ok_and(|id| id > 0)
            && !encoded.is_empty()
            && !encoded.contains(['/', '?', '#'])
            && URL_SAFE_NO_PAD.decode(encoded).is_ok_and(|bytes| {
                !bytes.is_empty()
                    && !bytes.contains(&0)
                    && URL_SAFE_NO_PAD.encode(bytes) == encoded
            })
        }
    }

    pub(crate) fn is_sqlite_cue_locator(uri: &str) -> bool {
        #[cfg(target_os = "linux")]
        { return parse_sqlite_cue_locator(uri).is_ok(); }
        #[cfg(not(target_os = "linux"))]
        { let _ = uri; false }
    }

    pub(crate) fn parse_sqlite_cue_range_uri(uri: &str) -> Result<SqliteCueRangePlayback, String> {
        let value = uri.strip_prefix("sonante-cue-range://")
            .ok_or("SQLITE_CUE_RANGE_URI_INVALID")?;
        let mut parts = value.split('/');
        let encoded = parts.next().ok_or("SQLITE_CUE_RANGE_URI_INVALID")?;
        let start_text = parts.next().ok_or("SQLITE_CUE_RANGE_URI_INVALID")?;
        let end_text = parts.next().ok_or("SQLITE_CUE_RANGE_URI_INVALID")?;
        if parts.next().is_some() || encoded.is_empty() || start_text.is_empty() {
            return Err("SQLITE_CUE_RANGE_URI_INVALID".into());
        }
        let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "SQLITE_CUE_RANGE_URI_INVALID")?;
        if bytes.is_empty() || bytes.contains(&0) || URL_SAFE_NO_PAD.encode(&bytes) != encoded {
            return Err("SQLITE_CUE_RANGE_URI_INVALID".into());
        }
        let path = String::from_utf8(bytes).map_err(|_| "SQLITE_CUE_PATH_NOT_MPD_COMPATIBLE")?;
        if !Path::new(&path).is_absolute() || path.contains(['\0', '\r', '\n']) {
            return Err("SQLITE_CUE_RANGE_URI_INVALID".into());
        }
        let start_frame = start_text.parse::<u64>().ok()
            .filter(|frame| frame.to_string() == start_text)
            .ok_or("SQLITE_CUE_RANGE_URI_INVALID")?;
        let end_frame = if end_text == "open" { None } else {
            Some(end_text.parse::<u64>().ok()
                .filter(|frame| frame.to_string() == end_text)
                .ok_or("SQLITE_CUE_RANGE_URI_INVALID")?)
        };
        if end_frame.is_some_and(|end| end <= start_frame) {
            return Err("SQLITE_CUE_RANGE_URI_INVALID".into());
        }
        Ok(SqliteCueRangePlayback { path, start_frame, end_frame })
    }

    pub(crate) fn is_sqlite_cue_range_uri(uri: &str) -> bool {
        Self::parse_sqlite_cue_range_uri(uri).is_ok()
    }

    pub(crate) fn sqlite_locator_source_id(uri: &str) -> Result<Option<i64>, String> {
        if !uri.starts_with("sqlite-track://") && !uri.starts_with("sqlite-cue://") {
            return Ok(None);
        }
        if !Self::is_sqlite_track_locator(uri) {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        let payload = uri.split_once("://").map(|(_, payload)| payload).ok_or("SQLITE_TRACK_LOCATOR_INVALID")?;
        let source_id = payload.split('/').next().and_then(|value| value.parse::<i64>().ok())
            .filter(|id| *id > 0).ok_or("SQLITE_TRACK_LOCATOR_INVALID")?;
        Ok(Some(source_id))
    }

    fn ensure_source_view(&self, source_id: i64) -> Result<Arc<SourceLibraryView>, String> {
        if let Some(view) = self
            .inner
            .source_views
            .lock()
            .map_err(|_| "SQLite library view is unavailable".to_owned())?
            .get(&source_id)
            .cloned()
        {
            let catalog = self.catalog()?;
            if !catalog.source_tag_edit_pending(source_id)?
                && catalog.is_source_library_view_current(&view)?
            {
                return Ok(view);
            }
        }
        let view = Arc::new(self.catalog()?.source_library_view(source_id)?);
        self.install_view(view.clone())?;
        Ok(view)
    }

    fn local_browser_albums(&self) -> Result<Vec<LocalBrowserAlbumDto>, String> {
        let sources = self.catalog()?.sources()?;
        let mut albums = Vec::new();
        for source in sources {
            let view = self.ensure_source_view(source.id)?;
            for album in view.albums() {
                let tracks = view
                    .tracks_for_album(&album.id)
                    .map(|tracks| tracks.collect::<Vec<_>>())
                    .unwrap_or_default();
                albums.push(local_browser_album_dto(&source, album, &tracks));
            }
            for cue_group in cue_album_groups(view.cue_tracks()) {
                let mut cue_dto = cue_album_dto(&source, &cue_group);
                let cue_audio_paths = cue_group
                    .tracks
                    .iter()
                    .map(|track| track.audio_path.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let physical_candidates = view
                    .albums()
                    .iter()
                    .filter(|album| album.directory_hint == cue_group.folder)
                    .collect::<Vec<_>>();
                let physical = physical_candidates
                    .iter()
                    .copied()
                    .find(|album| album.title.trim().to_lowercase() == cue_group.normalized_title)
                    .or_else(|| {
                        physical_candidates.iter().copied().find(|album| {
                            view.tracks_for_album(&album.id)
                                .map(|tracks| {
                                    tracks
                                        .map(|track| track.track.relative_path.clone())
                                        .collect::<std::collections::BTreeSet<_>>()
                                        .is_superset(&cue_audio_paths)
                                })
                                .unwrap_or(false)
                        })
                    });
                let Some(physical) = physical else {
                    albums.push(cue_dto);
                    continue;
                };
                let Some(index) = albums.iter().position(|album| album.id == album_id_for_view(physical)) else {
                    albums.push(cue_dto);
                    continue;
                };
                for genre in albums[index].genres.clone() {
                    crate::catalog_genres::add_genre(&mut cue_dto.genres, &genre);
                }
                match (albums[index].added_at.as_ref(), cue_dto.added_at.as_ref()) {
                    (Some(old), Some(new)) if old < new => cue_dto.added_at = Some(old.clone()),
                    (Some(old), None) => cue_dto.added_at = Some(old.clone()),
                    _ => {}
                }
                let cue_sheet_count = cue_group.tracks.iter().map(|track| &track.cue_path)
                    .collect::<std::collections::BTreeSet<_>>().len();
                let relevant_cue_sheets = view
                    .cue_tracks()
                    .iter()
                    .filter(|track| {
                        track.cue_path.parent() == Some(cue_group.folder.as_path())
                            && cue_audio_paths.contains(&track.audio_path)
                    })
                    .map(|track| track.cue_path.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let physical_audio_paths = view
                    .tracks_for_album(&physical.id)
                    .map(|tracks| {
                        tracks
                            .map(|track| track.track.relative_path.clone())
                            .collect::<std::collections::BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                let cue_matches_complete_physical_set = cue_audio_paths == physical_audio_paths;
                let cue_replaces_single_physical = physical.track_count == 1
                    && cue_group.tracks.len() >= 2
                    && cue_sheet_count == 1
                    && relevant_cue_sheets.len() == 1
                    && cue_matches_complete_physical_set;
                let cue_replaces_complete_multifile_album = cue_sheet_count == 1
                    && relevant_cue_sheets.len() == 1
                    && cue_group.tracks.len() >= 2
                    && cue_matches_complete_physical_set;
                if cue_replaces_single_physical || cue_replaces_complete_multifile_album {
                    albums[index] = cue_dto;
                } else {
                    // Keep the physical catalog entry canonical when the CUE
                    // is ambiguous, partial, or one of several competing sheets.
                    // A complete single CUE may define many virtual tracks over
                    // fewer physical files, so completeness is based on exact
                    // file-set coverage rather than track/file count equality.
                    for genre in cue_dto.genres {
                        crate::catalog_genres::add_genre(&mut albums[index].genres, &genre);
                    }
                    if cue_dto.added_at.as_ref().is_some_and(|added| albums[index].added_at.as_ref().is_none_or(|old| added < old)) {
                        albums[index].added_at = cue_dto.added_at;
                    }
                }
            }
        }
        Ok(albums)
    }

    /// Adapt the admitted SQLite album view to the existing Smart Playlist
    /// rule engine. This reads only the catalog and its generation-checked
    /// in-memory views; it never asks the legacy MPD catalog or scans roots.
    pub(crate) fn smart_playlist_albums(
        &self,
    ) -> Result<Vec<crate::smart_playlists::Album>, String> {
        self.local_browser_albums().map(|albums| {
            albums
                .into_iter()
                .map(|album| crate::smart_playlists::Album {
                    id: album.id,
                    title: album.title,
                    artist: album.artist,
                    genres: album.genres,
                    year: album
                        .year
                        .as_deref()
                        .and_then(|date| date.get(..4))
                        .and_then(|year| year.parse().ok())
                        .filter(|year| (1000..=9999).contains(year)),
                    added_at: album.added_at.as_deref().and_then(|value| crate::smart_playlists::local_added(Some(value))),
                    favorite_key: album.folder_path,
                    image: None,
                })
                .collect()
        })
    }

    pub(crate) fn has_indexed_sources(&self) -> Result<bool, String> {
        Ok(!self.catalog()?.sources()?.is_empty())
    }

    /// Confirms the exact configured roots against committed SQLite sources.
    /// The generation guard prevents another connection from removing or
    /// replacing one of them while the first-run preferences are published.
    pub(crate) fn with_initial_sources_guard<T>(
        &self,
        folders: &[String],
        local_enabled: bool,
        publish: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        if local_enabled && folders.is_empty() {
            return Err("SQLITE_WIZARD_SOURCE_REQUIRED".into());
        }
        if !local_enabled && !folders.is_empty() {
            return Err("SQLITE_WIZARD_SOURCE_DISABLED".into());
        }
        let mut catalog = Catalog::open(&self.inner.database_path)?;
        let sources = catalog.sources()?;
        let mut generations = Vec::with_capacity(folders.len());
        let mut seen = BTreeSet::new();
        for folder in folders {
            let canonical = std::fs::canonicalize(folder)
                .map_err(|_| "SQLITE_WIZARD_SOURCE_UNAVAILABLE".to_owned())?;
            if !seen.insert(canonical.clone()) {
                return Err("SQLITE_WIZARD_SOURCE_DUPLICATE".into());
            }
            let source = sources.iter()
                .find(|source| source.canonical_path == canonical && source.generation > 0)
                .ok_or_else(|| "SQLITE_WIZARD_SOURCE_NOT_INDEXED".to_owned())?;
            generations.push((source.id, source.generation));
        }
        catalog.with_source_generation_guard(&generations, publish)
    }

    pub(crate) fn local_browser_album_tracks(
        &self,
        encoded_album_id: &str,
    ) -> Result<Vec<audio::LocalAlbumDiscTracks>, String> {
        if encoded_album_id.starts_with("sqlite-cue-album:") {
            let (source_id, _) = parse_sqlite_cue_album_id(encoded_album_id)?;
            let view = self.current_view(source_id)?;
            let group = cue_album_groups(view.cue_tracks()).into_iter()
                .find(|group| cue_album_id(source_id, group) == encoded_album_id)
                .ok_or_else(|| "CUE_ALBUM_NOT_FOUND: reopen the library view".to_owned())?;
            let mut grouped = std::collections::BTreeMap::<PathBuf, Vec<_>>::new();
            for track in &group.tracks { grouped.entry(track.cue_path.clone()).or_default().push(track); }
            let mut used_numbers = std::collections::BTreeSet::new();
            let mut result = Vec::with_capacity(grouped.len());
            for (index, (cue_path, tracks)) in grouped.into_iter().enumerate() {
                let from_tag = tracks.iter().filter_map(|track| track.disc_number).find(|number| *number > 0);
                let from_folder = disc_number_from_path(&cue_path);
                let preferred = from_tag.or(from_folder).unwrap_or_else(|| u32::try_from(index + 1).unwrap_or(u32::MAX));
                let number = unique_cue_disc_number(preferred, &mut used_numbers);
                let label = cue_path.file_name().map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty()).unwrap_or_else(|| format!("Disc {number}"));
                let mut local_tracks = Vec::with_capacity(tracks.len());
                for track in tracks {
                    local_tracks.push(audio::LocalItem {
                        item_type: "file".into(),
                        path: sqlite_cue_locator(source_id, &track.cue_path, track.track_number)?,
                        name: format!("Track {:02}", track.track_number),
                        title: track.title.clone(),
                        artist: track.performers.first().cloned().or_else(|| track.album_performers.first().cloned()),
                        album: track.album.clone().or_else(|| Some(group.title.clone())),
                        duration: track.duration_ms.map(|duration| duration as f64 / 1000.0),
                    });
                }
                result.push(audio::LocalAlbumDiscTracks {
                    disc: audio::LocalAlbumDisc {
                        number,
                        label,
                        folder_path: format!("sqlite-album://{encoded_album_id}/disc/{number}"),
                        track_count: local_tracks.len(),
                    },
                    tracks: local_tracks,
                });
            }
            return Ok(result);
        }
        let mut parts = encoded_album_id.splitn(3, ':');
        if parts.next() != Some("sqlite") {
            return Err("SQLITE_ALBUM_ID_INVALID".into());
        }
        let source_id = parts
            .next()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
        let identity = parts
            .next()
            .ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
        let identity = URL_SAFE_NO_PAD
            .decode(identity)
            .map_err(|_| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
        let view = self.current_view(source_id)?;
        let album_id = AlbumId {
            source_id,
            identity,
        };
        let tracks = view
            .tracks_for_album(&album_id)
            .ok_or_else(|| "ALBUM_NOT_FOUND: reopen the library view".to_owned())?;
        let mut groups = std::collections::BTreeMap::<u32, Vec<audio::LocalItem>>::new();
        for item in tracks {
            let values = item
                .metadata
                .as_ref()
                .filter(|metadata| metadata.current_for_file)
                .and_then(|metadata| metadata.last_success.as_ref());
            let disc = values
                .and_then(|values| values.disc_number)
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value > 0)
                .or_else(|| disc_number_from_path(&item.track.relative_path))
                .unwrap_or(1);
            let path =
                URL_SAFE_NO_PAD.encode(item.track.relative_path.as_os_str().as_encoded_bytes());
            let name = item
                .track
                .relative_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Audio track".to_owned());
            groups.entry(disc).or_default().push(audio::LocalItem {
                item_type: "file".into(),
                path: format!("sqlite-track://{source_id}/{path}"),
                name,
                title: values.and_then(|values| values.title.clone()),
                artist: values.and_then(|values| values.track_artists.first().cloned()),
                album: values.and_then(|values| values.album.clone()),
                duration: values
                    .and_then(|values| values.duration_ms)
                    .map(|duration| duration as f64 / 1000.0),
            });
        }
        Ok(groups
            .into_iter()
            .map(|(number, tracks)| audio::LocalAlbumDiscTracks {
                disc: audio::LocalAlbumDisc {
                    number,
                    label: format!("Disc {number}"),
                    folder_path: format!("sqlite-album://{encoded_album_id}/disc/{number}"),
                    track_count: tracks.len(),
                },
                tracks,
            })
            .collect())
    }

    pub(crate) fn album_ab_tracks(&self, album_id: &str) -> Result<Vec<audio::LocalItem>, String> {
        let mut tracks = self.local_browser_album_tracks(album_id)?.into_iter()
            .flat_map(|disc| disc.tracks)
            .collect::<Vec<_>>();
        tracks.retain(|track| track.item_type == "file");
        if tracks.is_empty() || tracks.len() > 500 { return Err("AB_TRACKS".into()); }
        Ok(tracks)
    }

    /// Admit a comparison input by the exact album and opaque SQLite locator.
    /// The source view and generation are checked around the descriptor-open;
    /// decoding receives only a clone of that retained descriptor.
    pub(crate) fn open_album_ab_track(&self, album_id: &str, locator: &str) -> Result<SqliteAbTrack, String> {
        let items = self.album_ab_tracks(album_id)?;
        if !items.iter().any(|item| item.path == locator) {
            return Err("AB_CHANGED".into());
        }
        let (source_id, relative_path, cue_path, cue_number) = if locator.starts_with("sqlite-cue://") {
            let (source_id, cue_path, track_number) = parse_sqlite_cue_locator(locator)?;
            (source_id, None, Some(cue_path), Some(track_number))
        } else {
            let (source_id, relative) = parse_sqlite_track_locator(locator)?;
            (source_id, Some(relative), None, None)
        };
        let album_source = if album_id.starts_with("sqlite-cue-album:") {
            parse_sqlite_cue_album_id(album_id)?.0
        } else {
            parse_sqlite_album_id(album_id)?.0
        };
        if source_id != album_source { return Err("AB_SOURCE".into()); }
        let view = self.ensure_source_view(source_id)?;
        if self.catalog()?.source_tag_edit_pending(source_id)? {
            return Err("SOURCE_TAG_EDIT_REQUIRES_RECONCILIATION".into());
        }
        let (file, relative_path, cue) = if let (Some(cue_path), Some(track_number)) = (cue_path, cue_number) {
            let (cue, file) = view.open_verified_cue_audio_file(&cue_path, track_number)?
                .ok_or_else(|| "AB_CHANGED".to_owned())?;
            (file, cue.audio_path.clone(), Some(cue))
        } else {
            let relative = relative_path.ok_or_else(|| "AB_SOURCE".to_owned())?;
            let file = view.open_verified_track_file(&relative)?.ok_or_else(|| "AB_CHANGED".to_owned())?;
            (file, relative, None)
        };
        if !self.catalog()?.is_source_library_view_current(&view)? || !file.identity_is_current() {
            return Err("AB_CHANGED".into());
        }
        Ok(SqliteAbTrack { file: Arc::new(file), view, relative_path, cue })
    }

    pub(crate) fn album_ab_track_is_current(&self, track: &SqliteAbTrack) -> bool {
        if !track.file.identity_is_current()
            || !self.catalog().is_ok_and(|catalog| catalog.is_source_library_view_current(&track.view).unwrap_or(false)) {
            return false;
        }
        if let Some(cue) = &track.cue {
            return track.view.open_verified_cue_audio_file(&cue.cue_path, cue.track_number)
                .is_ok_and(|current| current.is_some_and(|(actual, file)| actual == *cue && track.file.same_object(&file)));
        }
        track.view.open_verified_track_file(&track.relative_path)
            .is_ok_and(|current| current.is_some_and(|file| track.file.same_object(&file)))
    }

    /// Read the finite, filesystem-verified profile used by Album Editions for
    /// a SQLite album. No MPD catalog lookup or audio-content read is involved.
    pub(crate) fn local_edition_profile(
        &self,
        encoded_album_id: &str,
    ) -> Result<Option<crate::album_file_sizes::Profile>, String> {
        let (source_id, identity) = parse_sqlite_album_id(encoded_album_id)?;
        let view = self.ensure_source_view(source_id)?;
        if self.catalog()?.source_tag_edit_pending(source_id)? {
            return Err("SOURCE_TAG_EDIT_REQUIRES_RECONCILIATION".into());
        }
        let album_id = AlbumId { source_id, identity };
        let tracks = view
            .tracks_for_album(&album_id)
            .ok_or_else(|| "EDITIONS_SOURCE".to_owned())?
            .collect::<Vec<_>>();
        if tracks.is_empty() || tracks.len() > crate::album_file_sizes::MAX_FILES {
            return Ok(None);
        }

        let mut sizes = Vec::with_capacity(tracks.len());
        let mut titles = Vec::with_capacity(tracks.len());
        for item in tracks {
            if view
                .verified_track_path(&item.track.relative_path)?
                .is_none()
            {
                return Err("SQLITE_TRACK_UNAVAILABLE_OR_CHANGED".into());
            }
            let size = u64::try_from(item.track.size)
                .ok()
                .filter(|size| *size > 0 && *size <= crate::album_file_sizes::MAX_SAFE_BYTES)
                .ok_or_else(|| "EDITIONS_SOURCE".to_owned())?;
            sizes.push(size);
            let title = item
                .metadata
                .as_ref()
                .filter(|metadata| metadata.current_for_file)
                .and_then(|metadata| metadata.last_success.as_ref())
                .and_then(|values| values.title.clone());
            titles.push(title);
        }
        if self.catalog()?.source_tag_edit_pending(source_id)?
            || !self.catalog()?.is_source_library_view_current(&view)?
        {
            return Err("VIEW_STALE: reopen the source before comparing editions".into());
        }
        Ok(Some(crate::album_file_sizes::Profile::new(Some(sizes), titles)))
    }

    pub(crate) fn sqlite_tag_album_inspection(
        &self,
        encoded_album_id: &str,
    ) -> Result<tags::LocalAlbumTagInspection, String> {
        let (source_id, album_identity) = parse_sqlite_album_id(encoded_album_id)?;
        let view = self.current_view(source_id)?;
        let album_id = AlbumId { source_id, identity: album_identity };
        let tracks = view.tracks_for_album(&album_id)
            .ok_or_else(|| "ALBUM_NOT_FOUND: reopen this library view".to_owned())?
            .map(|entry| (entry.track.relative_path.clone(), entry.track.size, entry.track.mtime_ns))
            .collect::<Vec<_>>();
        let catalog = self.catalog()?;
        if catalog.source_tag_edit_pending(source_id)? {
            return Err("SOURCE_TAG_EDIT_REQUIRES_RECONCILIATION".into());
        }
        let mut inspected = Vec::with_capacity(tracks.len());
        for (relative, _, _) in tracks {
            let uri = sqlite_track_locator(source_id, &relative)?;
            let cue_present = catalog.source_track_directory_has_cue(source_id, view.generation(), &relative)?;
            inspected.push(match tags::inspect_sqlite_track_tags(&catalog, source_id, view.generation(), &relative) {
                Ok(mut tag_values) => {
                    let issue = if cue_present {
                        tag_values.writable = false;
                        tag_values.editable = false;
                        tag_values.unsupported_reason = Some("A folha CUE nesta pasta impede a edição segura pelo editor de álbum SQLite.".into());
                        Some(tags::sqlite_cue_issue())
                    } else { None };
                    tags::LocalAlbumTrackInspection {
                        uri,
                        kind: tags::LocalAlbumTrackKind::Physical,
                        tags: Some(tag_values),
                        issue,
                    }
                }
                Err(issue) => tags::LocalAlbumTrackInspection {
                    uri,
                    kind: tags::LocalAlbumTrackKind::Physical,
                    tags: None,
                    issue: Some(issue),
                },
            });
        }
        if !catalog.is_source_library_view_current(&view)? {
            return Err("VIEW_STALE: reopen this library view".into());
        }
        Ok(tags::inspect_sqlite_album_from_tracks(
            encoded_album_id,
            view.generation(),
            inspected,
        ))
    }

    pub(crate) fn preflight_sqlite_tag_batch(
        &self,
        request: tags::LocalAlbumTagBatchPreflightRequest,
    ) -> Result<tags::LocalAlbumTagBatchPlan, tags::LocalAlbumTagBatchPreflightError> {
        let inspection = self.sqlite_tag_album_inspection(&request.album_id)
            .map_err(|_| tags::LocalAlbumTagBatchPreflightError::album_catalog_failed())?;
        tags::preflight_sqlite_album_tag_batch(request, inspection)
    }

    pub(crate) fn apply_sqlite_tag_batch(
        &self,
        request: tags::LocalAlbumTagBatchPreflightRequest,
    ) -> Result<(tags::LocalAlbumTagBatchApplyResult, bool, HashMap<String, String>), tags::LocalAlbumTagBatchPreflightError> {
        let plan = self.preflight_sqlite_tag_batch(request)?;
        if !plan.applicable {
            let result = tags::execute_sqlite_album_tag_batch_with(plan, |_| {
                Err(tags::sqlite_catalog_write_blocked())
            });
            // A rejected preflight performs no filesystem writes, so there is
            // no pending catalog reconciliation to report.
            return Ok((result, true, HashMap::new()));
        }

        let (source_id, _) = parse_sqlite_album_id(&plan.album_id)
            .map_err(|_| tags::LocalAlbumTagBatchPreflightError::album_not_found())?;
        let view = self.current_view(source_id)
            .map_err(|_| tags::LocalAlbumTagBatchPreflightError::album_catalog_failed())?;
        let generation = view.generation();
        let mut refresh_paths = HashMap::new();
        for track in &plan.tracks {
            if !track.needs_write { continue; }
            let (locator_source, relative) = parse_sqlite_track_locator(&track.uri)
            .map_err(|_| tags::LocalAlbumTagBatchPreflightError::membership_changed())?;
            if locator_source != source_id {
                return Err(tags::LocalAlbumTagBatchPreflightError::membership_changed());
            }
            let path = view.verified_track_path(&relative)
                .map_err(|_| tags::LocalAlbumTagBatchPreflightError::membership_changed())?
                .ok_or_else(tags::LocalAlbumTagBatchPreflightError::membership_changed)?;
            let path = path.to_str().filter(|path| Path::new(path).is_absolute())
                .ok_or_else(tags::LocalAlbumTagBatchPreflightError::membership_changed)?;
            refresh_paths.insert(track.uri.clone(), path.to_owned());
        }

        let mut catalog = self.catalog()
            .map_err(|_| tags::LocalAlbumTagBatchPreflightError::state_unavailable())?;
        catalog.begin_source_tag_edit(source_id, generation)
            .map_err(|_| tags::LocalAlbumTagBatchPreflightError::revision_conflict())?;
        let result = tags::execute_sqlite_album_tag_batch_with(plan, |track| {
            let (locator_source, relative) = parse_sqlite_track_locator(&track.uri)
                .map_err(|_| tags::sqlite_catalog_write_blocked())?;
            if locator_source != source_id {
                return Err(tags::sqlite_catalog_write_blocked());
            }
            tags::update_sqlite_track_tags(
                &mut catalog,
                source_id,
                generation,
                &relative,
                &track.expected_revision,
                &track.update,
            )
        });

        let has_committed_file = result.tracks.iter().any(|track| {
            track.state == tags::LocalAlbumTagBatchTrackOutcomeState::Committed
                || track.error.as_ref().is_some_and(|error| error.committed)
        });
        let catalog_reconciled = if has_committed_file {
            match catalog.reconcile_tag_edit_with_progress(source_id, |_| {}) {
                Ok(_) => {
                    let refreshed = Arc::new(catalog.source_library_view(source_id)
                        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::album_catalog_failed())?);
                    drop(catalog);
                    self.install_view(refreshed)
                        .map_err(|_| tags::LocalAlbumTagBatchPreflightError::state_unavailable())?;
                    true
                }
                Err(error) => {
                    eprintln!("[SQLiteTags] file_write_committed=true catalog_reconciled=false error={}", crate::diagnostics::error_summary(&error));
                    false
                }
            }
        } else {
            catalog.cancel_source_tag_edit(source_id, generation)
                .map_err(|_| tags::LocalAlbumTagBatchPreflightError::state_unavailable())?;
            true
        };
        Ok((result, catalog_reconciled, refresh_paths))
    }

    fn search_local_tracks(&self, query: &str) -> Result<SearchLocalTracksDto, String> {
        let query = query.trim();
        if query.chars().count() < 2
            || query.chars().count() > 160
            || query.contains(['\0', '\r', '\n'])
        {
            return Ok(SearchLocalTracksDto {
                tracks: Vec::new(),
                limited: false,
            });
        }

        const PREVIEW_LIMIT: usize = 20;
        let sources = self.catalog()?.sources()?;
        let mut tracks = Vec::new();
        for source in sources {
            let view = self.ensure_source_view(source.id)?;
            for entry in view.search_tracks(query, PREVIEW_LIMIT + 1) {
                let values = entry
                    .metadata
                    .as_ref()
                    .filter(|metadata| metadata.current_for_file)
                    .and_then(|metadata| metadata.last_success.as_ref());
                let relative = URL_SAFE_NO_PAD
                    .encode(entry.track.relative_path.as_os_str().as_encoded_bytes());
                let name = entry
                    .track
                    .relative_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "Audio track".to_owned());
                tracks.push(audio::LocalItem {
                    item_type: "file".into(),
                    path: format!("sqlite-track://{}/{relative}", source.id),
                    name,
                    title: values.and_then(|values| values.title.clone()),
                    artist: values.and_then(|values| values.track_artists.first().cloned()),
                    album: values.and_then(|values| values.album.clone()),
                    duration: values
                        .and_then(|values| values.duration_ms)
                        .map(|duration| duration as f64 / 1000.0),
                });
                if tracks.len() > PREVIEW_LIMIT {
                    return Ok(SearchLocalTracksDto {
                        tracks: tracks.into_iter().take(PREVIEW_LIMIT).collect(),
                        limited: true,
                    });
                }
            }
            let query_terms = query.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
            for group in cue_album_groups(view.cue_tracks()) {
                let physical = view.albums().iter().find(|album| {
                    album.directory_hint == group.folder
                        && album.title.trim().to_lowercase() == group.normalized_title
                });
                if physical.is_some_and(|album| {
                    let sheet_count = group.tracks.iter().map(|track| &track.cue_path)
                        .collect::<std::collections::BTreeSet<_>>().len();
                    !(album.track_count == 1 && group.tracks.len() >= 2 && sheet_count == 1)
                }) {
                    continue;
                }
                for track in &group.tracks {
                    let title = track.title.as_deref().unwrap_or(&group.title);
                    let artist = track.performers.first().or(track.album_performers.first()).map(String::as_str).unwrap_or("");
                    let album = track.album.as_deref().unwrap_or(&group.title);
                    let searchable = format!("{title} {artist} {album} {}", track.genre.as_deref().unwrap_or("")).to_lowercase();
                    if !query_terms.iter().all(|term| searchable.contains(term)) { continue; }
                    tracks.push(audio::LocalItem {
                        item_type: "file".into(),
                        path: sqlite_cue_locator(source.id, &track.cue_path, track.track_number)?,
                        name: format!("Track {:02}", track.track_number),
                        title: track.title.clone(),
                        artist: track.performers.first().cloned().or_else(|| track.album_performers.first().cloned()),
                        album: track.album.clone().or_else(|| Some(group.title.clone())),
                        duration: track.duration_ms.map(|duration| duration as f64 / 1000.0),
                    });
                    if tracks.len() > PREVIEW_LIMIT {
                        return Ok(SearchLocalTracksDto { tracks: tracks.into_iter().take(PREVIEW_LIMIT).collect(), limited: true });
                    }
                }
            }
        }
        Ok(SearchLocalTracksDto {
            tracks,
            limited: false,
        })
    }

    fn album_cover(&self, encoded_album_id: &str) -> Result<Option<String>, String> {
        let _permit = crate::bounded_artwork_io::COVER_READS.acquire()?;
        if encoded_album_id.starts_with("sqlite-cue-album:") {
            let (source_id, _) = parse_sqlite_cue_album_id(encoded_album_id)?;
            let view = self.current_view(source_id)?;
            let group = cue_album_groups(view.cue_tracks()).into_iter()
                .find(|group| cue_album_id(source_id, group) == encoded_album_id)
                .ok_or_else(|| "CUE_ALBUM_NOT_FOUND: reopen the library view".to_owned())?;
            let mut artwork = view.read_source_artwork_in_directory(
                &group.folder, audio::LOCAL_FOLDER_COVER_NAMES, MAX_SQLITE_COVER_BYTES,
            )?;
            if artwork.is_none() {
                let mut first_error = None;
                let mut seen_audio = std::collections::HashSet::new();
                for track in group.tracks.iter().filter(|track| seen_audio.insert(track.audio_path.clone())).take(8) {
                    match view.read_cue_embedded_artwork(
                        &track.cue_path, track.track_number,
                        MAX_EMBEDDED_SOURCE_BYTES, MAX_SQLITE_COVER_BYTES,
                    ) {
                        Ok(Some(found)) => { artwork = Some(found); break; }
                        Ok(None) => {}
                        Err(error) => { first_error.get_or_insert(error); }
                    }
                }
                if artwork.is_none() {
                    if let Some(error) = first_error { return Err(error); }
                }
            }
            if !self.catalog()?.is_source_library_view_current(&view)? {
                return Err("VIEW_STALE: reopen this source before loading artwork".into());
            }
            return Ok(artwork.map(|artwork| format!(
                "data:{};base64,{}", artwork.mime_type, STANDARD.encode(artwork.bytes),
            )));
        }
        let mut parts = encoded_album_id.splitn(3, ':');
        if parts.next() != Some("sqlite") {
            return Err("SQLITE_ALBUM_ID_INVALID".into());
        }
        let source_id = parts
            .next()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
        let identity = parts
            .next()
            .ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
        let identity = URL_SAFE_NO_PAD
            .decode(identity)
            .map_err(|_| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
        let view = self.current_view(source_id)?;
        let album_id = AlbumId {
            source_id,
            identity,
        };
        let artwork = match view.read_album_artwork(
            &album_id,
            audio::LOCAL_FOLDER_COVER_NAMES,
            MAX_SQLITE_COVER_BYTES,
        )? {
            Some(external) => Some(external),
            None => view.read_album_embedded_artwork(
                &album_id,
                MAX_EMBEDDED_SOURCE_BYTES,
                MAX_SQLITE_COVER_BYTES,
            )?,
        };
        Ok(artwork.map(|artwork| {
            format!(
                "data:{};base64,{}",
                artwork.mime_type,
                STANDARD.encode(artwork.bytes),
            )
        }))
    }

    /// Resolve the exact album associated with an opaque SQLite playback
    /// locator. The source snapshot must still be current and the track must
    /// still pass the scanner's identity/size/mtime validation before the
    /// existing Local artwork resolver can load its cover.
    fn album_for_track(&self, uri: &str) -> Result<Option<String>, String> {
        if !Self::is_sqlite_track_locator(uri) {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        if uri.starts_with("sqlite-cue://") {
            let (source_id, cue_path, track_number) = parse_sqlite_cue_locator(uri)?;
            let view = self.ensure_source_view(source_id)?;
            let virtual_track = view.verified_cue_track(&cue_path, track_number)?
                .ok_or_else(|| "SQLITE_CUE_TRACK_NOT_INDEXED".to_owned())?;
            let group = cue_album_groups(view.cue_tracks()).into_iter().find(|group| {
                group.tracks.iter().any(|track| track.cue_path == cue_path && track.track_number == track_number)
            }).ok_or_else(|| "SQLITE_CUE_ALBUM_UNAVAILABLE".to_owned())?;
            let cue_id = cue_album_id(source_id, &group);
            let visible = self.local_browser_albums()?.iter().any(|album| album.id == cue_id);
            let album_id = if visible {
                Some(cue_id)
            } else {
                view.albums().iter().find(|album| {
                    album.directory_hint == group.folder
                        && album.title.trim().to_lowercase() == group.normalized_title
                }).map(album_id_for_view)
            };
            if !self.catalog()?.is_source_library_view_current(&view)? {
                return Err("VIEW_STALE: reopen this source before loading artwork".into());
            }
            let _ = virtual_track;
            return Ok(album_id);
        }
        let locator = uri.strip_prefix("sqlite-track://").ok_or("SQLITE_TRACK_LOCATOR_INVALID")?;
        let (source_text, encoded_path) = locator
            .split_once('/')
            .ok_or("SQLITE_TRACK_LOCATOR_INVALID")?;
        let source_id = source_text
            .parse::<i64>()
            .ok()
            .filter(|id| *id > 0)
            .ok_or("SQLITE_TRACK_LOCATOR_INVALID")?;
        let encoded = URL_SAFE_NO_PAD
            .decode(encoded_path)
            .map_err(|_| "SQLITE_TRACK_LOCATOR_INVALID")?;
        #[cfg(target_os = "linux")]
        let relative = PathBuf::from(std::ffi::OsString::from_vec(encoded));
        #[cfg(not(target_os = "linux"))]
        return Err("SQLITE_TRACK_PLAYBACK_REQUIRES_LINUX_SECURE_SCANNER".into());

        #[cfg(target_os = "linux")]
        {
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || !relative
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
            }
            let view = self.ensure_source_view(source_id)?;
            if view.verified_track_path(&relative)?.is_none() {
                return Err("SQLITE_TRACK_NOT_INDEXED".into());
            }
            let album = view
                .album_for_track(&relative)
                .ok_or_else(|| "SQLITE_TRACK_ALBUM_UNAVAILABLE".to_owned())?;
            let album_id = album_id_for_view(album);
            if !self.catalog()?.is_source_library_view_current(&view)? {
                return Err("VIEW_STALE: reopen this source before loading artwork".into());
            }
            Ok(Some(album_id))
        }
    }

    /// Resolve queue artwork from the exact opaque playback identity. The
    /// normal locator validation and source-generation checks remain in force;
    /// callers never receive or submit an arbitrary filesystem path.
    pub(crate) fn album_cover_for_track_locator(&self, uri: &str) -> Result<Option<String>, String> {
        if !Self::is_sqlite_track_locator(uri) {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        let Some(album_id) = self.album_for_track(uri)? else { return Ok(None) };
        self.album_cover(&album_id)
    }

    /// Return only parser-confirmed technical facts for the exact queued
    /// SQLite locator. This reads the already-loaded catalog snapshot and
    /// performs the same descriptor-anchored file identity check as playback;
    /// stale metadata and unsupported formats stay unavailable.
    pub(crate) fn player_source_facts(
        &self,
        uri: &str,
    ) -> Result<Option<crate::player_source_info::SourceAudioFacts>, String> {
        let (source_id, relative) = if uri.starts_with("sqlite-cue://") {
            let (source_id, cue_path, track_number) = parse_sqlite_cue_locator(uri)?;
            let view = self.ensure_source_view(source_id)?;
            let cue = view.verified_cue_track(&cue_path, track_number)?
                .ok_or_else(|| "SQLITE_CUE_TRACK_NOT_INDEXED".to_owned())?;
            (source_id, cue.audio_path)
        } else {
            parse_sqlite_track_locator(uri)?
        };
        let view = self.ensure_source_view(source_id)?;
        let Some(album) = view.album_for_track(&relative) else { return Ok(None) };
        if view.verified_track_path(&relative)?.is_none() {
            return Err("SQLITE_TRACK_NOT_INDEXED".into());
        }
        let Some(track) = view.tracks_for_album(&album.id)
            .and_then(|tracks| tracks.into_iter().find(|item| item.track.relative_path == relative))
        else { return Ok(None) };
        let Some(metadata) = track.metadata.as_ref().filter(|metadata| metadata.current_for_file)
        else { return Ok(None) };
        let (codec, compression, sample_type) = match metadata.file_format.as_str() {
            "flac" => ("FLAC", "lossless", "integer"),
            "mp3" => ("MP3", "lossy", "not_applicable"),
            "ogg_vorbis" => ("Vorbis", "lossy", "not_applicable"),
            "ogg_opus" => ("Opus", "lossy", "not_applicable"),
            "mp4_aac" => ("AAC", "lossy", "not_applicable"),
            "mp4_alac" => ("ALAC", "lossless", "integer"),
            "wav_pcm" | "aiff_pcm" => ("PCM", "uncompressed", "integer"),
            _ => return Ok(None),
        };
        let Some(values) = metadata.last_success.as_ref() else { return Ok(None) };
        if !self.catalog()?.is_source_library_view_current(&view)? {
            return Err("VIEW_STALE: reopen this source before reading playback facts".into());
        }
        Ok(Some(crate::player_source_info::SourceAudioFacts {
            provenance: "local",
            codec,
            compression,
            sample_type,
            sample_rate: values.sample_rate.and_then(|value| u32::try_from(value).ok()),
            bit_depth: None,
            dsd_level: None,
            revision: None,
        }))
    }

    /// Read the exact lowercase `.lrc` sibling for an opaque SQLite physical
    /// track locator. CUE locators are rejected by the track-only parser; the
    /// independent catalog connection keeps disk I/O outside shared state
    /// locks, while the catalog fences source generation and file identity.
    pub(crate) fn read_local_track_lrc(
        &self,
        uri: &str,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        let (source_id, relative) = parse_sqlite_track_locator(uri)
            .map_err(|_| "SQLITE_LYRICS_TRACK_NOT_ELIGIBLE".to_owned())?;
        let catalog = Catalog::open(&self.inner.database_path)?;
        let source = catalog
            .sources()?
            .into_iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| "SQLITE_LYRICS_SOURCE_UNAVAILABLE".to_owned())?;
        catalog
            .read_verified_track_lrc(source_id, source.generation, &relative, max_bytes)
            .map_err(|error| {
                if error.contains("exceeds size limit")
                    || error.contains("grew beyond size limit")
                    || error.contains("exceeds the supported maximum")
                {
                    "SQLITE_LYRICS_TOO_LARGE".to_owned()
                } else if error.contains("Permission denied") {
                    "SQLITE_LYRICS_PERMISSION_DENIED".to_owned()
                } else {
                    "SQLITE_LYRICS_SOURCE_CHANGED_OR_UNAVAILABLE".to_owned()
                }
            })
    }

    fn list_sources(&self) -> Result<Vec<SourceDto>, String> {
        let catalog = self.catalog()?;
        catalog
            .sources()?
            .iter()
            .map(|source| source_dto(&catalog, source))
            .collect()
    }

    #[cfg(test)]
    fn add_and_index(&self, folder: &Path) -> Result<IndexResultDto, String> {
        self.add_and_index_with_progress(folder, |_| {})
    }

    fn add_and_index_with_progress<F>(
        &self,
        folder: &Path,
        progress: F,
    ) -> Result<IndexResultDto, String>
    where
        F: FnMut(IndexProgress),
    {
        let display_name = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Local music".into());
        // Filesystem traversal and tag parsing can take a long time on network
        // roots. Use an independent SQLite connection so browsing the last
        // committed catalog is not blocked behind the UI connection's mutex.
        // Catalog commits are atomic and concurrent writers are serialized by
        // SQLite plus the catalog's generation checks.
        let mut catalog = Catalog::open(&self.inner.database_path)?;
        let canonical_root = std::fs::canonicalize(folder)
            .map_err(|_| "LOCAL_SOURCE_UNAVAILABLE: selected folder cannot be opened".to_owned())?;
        if !canonical_root.is_dir() {
            return Err("LOCAL_SOURCE_UNAVAILABLE: selected folder is not a directory".into());
        }
        if let Some(source) = catalog.sources()?.into_iter()
            .find(|source| source.canonical_path == canonical_root)
        {
            let view = Arc::new(catalog.source_library_view(source.id)?);
            let response = IndexResultDto {
                source: source_dto(&catalog, &source)?,
                metrics: metrics_dto(&Metrics::default()),
                view: view_dto(&view),
            };
            drop(catalog);
            self.install_view(view)?;
            return Ok(response);
        }
        let report = catalog.add_source_with_progress(&canonical_root, display_name, progress)?;
        let source = catalog
            .sources()?
            .into_iter()
            .find(|source| source.id == report.source_id)
            .ok_or_else(|| "new source was not found after indexing".to_owned())?;
        let view = Arc::new(catalog.source_library_view(source.id)?);
        let response = IndexResultDto {
            source: source_dto(&catalog, &source)?,
            metrics: metrics_dto(&report.scan.metrics),
            view: view_dto(&view),
        };
        drop(catalog);
        self.install_view(view)?;
        Ok(response)
    }

    #[cfg(test)]
    fn verify(&self, source_id: i64) -> Result<IndexResultDto, String> {
        self.verify_with_progress(source_id, |_, _| {})
    }

    fn verify_with_progress<F>(
        &self,
        source_id: i64,
        mut progress: F,
    ) -> Result<IndexResultDto, String>
    where
        F: FnMut(&str, IndexProgress),
    {
        // Keep reads of the last committed view available during the scan.
        let mut catalog = Catalog::open(&self.inner.database_path)?;
        let source_name = catalog
            .sources()?
            .into_iter()
            .find(|source| source.id == source_id)
            .map(|source| source.display_name)
            .ok_or_else(|| "source not found".to_owned())?;
        let pending_tag_reconcile = catalog.source_tag_edit_pending(source_id)?;
        let report = if pending_tag_reconcile {
            catalog.reconcile_tag_edit_with_progress(source_id, |event| progress(&source_name, event))?
        } else {
            catalog.verify_source_with_progress(source_id, |event| progress(&source_name, event))?
        };
        let source = catalog
            .sources()?
            .into_iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| "source was removed during verification".to_owned())?;
        let view = Arc::new(catalog.source_library_view(source_id)?);
        let response = IndexResultDto {
            source: source_dto(&catalog, &source)?,
            metrics: metrics_dto(&report.metrics),
            view: view_dto(&view),
        };
        drop(catalog);
        self.install_view(view)?;
        Ok(response)
    }

    fn reindex_with_progress<F>(
        &self,
        source_id: i64,
        mut progress: F,
    ) -> Result<IndexResultDto, String>
    where
        F: FnMut(&str, IndexProgress),
    {
        let mut catalog = Catalog::open(&self.inner.database_path)?;
        let source_name = catalog.sources()?.into_iter()
            .find(|source| source.id == source_id)
            .map(|source| source.display_name)
            .ok_or_else(|| "source not found".to_owned())?;
        let report = catalog.rebuild_source_with_progress(source_id, |event| progress(&source_name, event))?;
        let source = catalog.sources()?.into_iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| "source was removed during reindex".to_owned())?;
        let view = Arc::new(catalog.source_library_view(source_id)?);
        let response = IndexResultDto {
            source: source_dto(&catalog, &source)?,
            metrics: metrics_dto(&report.metrics),
            view: view_dto(&view),
        };
        drop(catalog);
        self.install_view(view)?;
        Ok(response)
    }

    fn remove_source(&self, source_id: i64) -> Result<RemoveSourceDto, String> {
        let mut catalog = Catalog::open(&self.inner.database_path)?;
        let source = catalog.sources()?.into_iter()
            .find(|source| source.id == source_id)
            .ok_or_else(|| "source not found".to_owned())?;
        let removed_track_count = catalog.remove_source(source_id)?;
        drop(catalog);

        lock_error(self.inner.source_views.lock(), "SQLite library view")?.remove(&source_id);
        let mut active = lock_error(self.inner.active_view.lock(), "SQLite active library view")?;
        if active.as_ref().is_some_and(|view| view.source_id() == source_id) {
            *active = None;
        }
        Ok(RemoveSourceDto { source_id, display_name: source.display_name, removed_track_count, queue_stopped: false })
    }

    fn open_view(&self, source_id: i64) -> Result<LibraryViewDto, String> {
        let catalog = self.catalog()?;
        let view = Arc::new(catalog.source_library_view(source_id)?);
        drop(catalog);
        let result = view_dto(&view);
        self.install_view(view)?;
        Ok(result)
    }

    fn install_view(&self, view: Arc<SourceLibraryView>) -> Result<(), String> {
        self.inner
            .source_views
            .lock()
            .map_err(|_| "SQLite library view is unavailable".to_owned())?
            .insert(view.source_id(), view.clone());
        *self
            .inner
            .active_view
            .lock()
            .map_err(|_| "SQLite library view is unavailable".to_owned())? = Some(view);
        Ok(())
    }
}

const MAX_SQLITE_COVER_BYTES: usize = 8 * 1024 * 1024;
// Lofty decodes picture blocks into memory. A strict source cap plus the
// shared two-reader artwork semaphore bounds peak work without ever loading
// large lossless/DSD files just to search for an optional cover.
const MAX_EMBEDDED_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalBrowserAlbumDto {
    id: String,
    title: String,
    artist: String,
    artist_names: Vec<String>,
    year: Option<String>,
    added_at: Option<String>,
    genres: Vec<String>,
    folder_path: String,
    source_id: Option<String>,
    source_label: Option<String>,
    track_count: usize,
    discs: Vec<audio::LocalAlbumDisc>,
    tag_presence: Option<audio::LocalAlbumTagPresence>,
    grouping_notes: Vec<String>,
}

#[derive(Clone)]
struct CueAlbumGroup {
    folder: PathBuf,
    title: String,
    normalized_title: String,
    tracks: Vec<sonante_sqlite_catalog::CueVirtualTrack>,
}

fn cue_album_groups(tracks: &[sonante_sqlite_catalog::CueVirtualTrack]) -> Vec<CueAlbumGroup> {
    let mut groups = std::collections::BTreeMap::<(Vec<u8>, String), CueAlbumGroup>::new();
    for track in tracks {
        let folder = track.cue_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let folder_bytes = folder.as_os_str().as_encoded_bytes().to_vec();
        let fallback = folder.file_name().map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty()).unwrap_or_else(|| "Álbum Desconhecido".into());
        let title = track.album.as_deref().map(str::trim).filter(|title| !title.is_empty())
            .unwrap_or(&fallback).to_owned();
        let normalized_title = title.trim().to_lowercase();
        let key = (folder_bytes, normalized_title.clone());
        let group = groups.entry(key).or_insert_with(|| CueAlbumGroup {
            folder: folder.clone(), title: title.clone(), normalized_title: normalized_title.clone(), tracks: Vec::new(),
        });
        if title > group.title { group.title = title; }
        group.tracks.push(track.clone());
    }
    groups.into_values().map(|mut group| {
        group.tracks.sort_by(|left, right| left.cue_path.cmp(&right.cue_path)
            .then_with(|| left.disc_number.cmp(&right.disc_number))
            .then_with(|| left.track_number.cmp(&right.track_number)));
        group
    }).collect()
}

fn cue_album_id(source_id: i64, group: &CueAlbumGroup) -> String {
    let folder = group.folder.as_os_str().as_encoded_bytes();
    let title = group.normalized_title.as_bytes();
    let mut identity = b"sonante-cue-album-v1\0".to_vec();
    identity.extend_from_slice(&(folder.len() as u64).to_be_bytes());
    identity.extend_from_slice(folder);
    identity.extend_from_slice(&(title.len() as u64).to_be_bytes());
    identity.extend_from_slice(title);
    format!("sqlite-cue-album:{source_id}:{}", URL_SAFE_NO_PAD.encode(identity))
}

fn parse_sqlite_cue_album_id(value: &str) -> Result<(i64, Vec<u8>), String> {
    let mut parts = value.splitn(3, ':');
    if parts.next() != Some("sqlite-cue-album") { return Err("SQLITE_ALBUM_ID_INVALID".into()); }
    let source_id = parts.next().and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0).ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
    let encoded = parts.next().ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
    let identity = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
    if identity.is_empty() || URL_SAFE_NO_PAD.encode(&identity) != encoded {
        return Err("SQLITE_ALBUM_ID_INVALID".into());
    }
    Ok((source_id, identity))
}

fn unique_cue_disc_number(preferred: u32, used: &mut std::collections::BTreeSet<u32>) -> u32 {
    if preferred > 0 && used.insert(preferred) {
        return preferred;
    }
    // Search a finite range instead of incrementing a u32 that may saturate
    // forever for a duplicate maximum value from a malformed CUE disc tag.
    for candidate in 1..=used.len().saturating_add(1) {
        if let Ok(candidate) = u32::try_from(candidate) {
            if used.insert(candidate) {
                return candidate;
            }
        }
    }
    // More than u32::MAX sheets cannot be represented by the UI. Keep this
    // conversion finite even in that impossible-in-practice catalog state.
    preferred.max(1)
}

fn normalized_album_year(dates: &[String]) -> Option<String> {
    let years = dates
        .iter()
        .filter_map(|date| {
            let year = date.get(..4)?;
            if !year.bytes().all(|byte| byte.is_ascii_digit())
                || (date.len() > 4 && !matches!(date.as_bytes()[4], b'-' | b'/' | b'.'))
            {
                return None;
            }
            let numeric = year.parse::<u32>().ok()?;
            (1000..=9999).contains(&numeric).then(|| year.to_owned())
        })
        .collect::<std::collections::BTreeSet<_>>();
    (years.len() == 1)
        .then(|| years.into_iter().next())
        .flatten()
}

fn cue_album_dto(source: &Source, group: &CueAlbumGroup) -> LocalBrowserAlbumDto {
    let mut artist_names = Vec::<String>::new();
    let mut album_artists = Vec::<String>::new();
    let mut genres = Vec::<String>::new();
    let mut dates = Vec::<String>::new();
    let mut artists = std::collections::BTreeMap::<String, (String, usize)>::new();
    let mut cue_discs = std::collections::BTreeMap::<PathBuf, Vec<&sonante_sqlite_catalog::CueVirtualTrack>>::new();
    let mut first_seen = Vec::with_capacity(group.tracks.len());
    for track in &group.tracks {
        cue_discs.entry(track.cue_path.clone()).or_default().push(track);
        first_seen.push(track.first_seen_ns);
        for artist in &track.performers {
            if !artist_names.contains(artist) { artist_names.push(artist.clone()); }
            let entry = artists.entry(artist.trim().to_lowercase()).or_insert_with(|| (artist.clone(), 0));
            entry.1 += 1;
            if artist > &entry.0 { entry.0 = artist.clone(); }
        }
        for artist in &track.album_performers {
            if !album_artists.contains(artist) { album_artists.push(artist.clone()); }
            if !artist_names.contains(artist) { artist_names.push(artist.clone()); }
        }
        if let Some(genre) = track.genre.as_deref() { crate::catalog_genres::add_genre(&mut genres, genre); }
        if let Some(date) = track.date.as_ref().filter(|date| !date.trim().is_empty()) {
            if !dates.contains(date) { dates.push(date.clone()); }
        }
    }
    let consistent_album_artist = group.tracks.iter().all(|track| track.album_performers.len() == 1
        && group.tracks.first().is_some_and(|first| first.album_performers.first() == track.album_performers.first()));
    let artist = if consistent_album_artist {
        album_artists.first().cloned().unwrap_or_default()
    } else if artists.len() == 1 {
        artists.values().next().map(|(name, _)| name.clone()).unwrap_or_default()
    } else if let Some((name, count)) = artists.values().max_by_key(|(_, count)| *count) {
        if *count >= group.tracks.len() - group.tracks.len() / 5 { name.clone() } else { "Various Artists".into() }
    } else { "Artista Desconhecido".into() };

    let id = cue_album_id(source.id, group);
    let path = format!("sqlite-album://{id}");
    let all_dates_known = !first_seen.is_empty() && first_seen.iter().all(Option::is_some);
    let added_at = all_dates_known.then(|| first_seen.into_iter().flatten().min()).flatten().and_then(format_added_at);
    let mut used_numbers = std::collections::BTreeSet::new();
    let discs = cue_discs.into_iter().enumerate().map(|(index, (cue_path, tracks))| {
        let from_tag = tracks.iter().filter_map(|track| track.disc_number).find(|number| *number > 0);
        let from_folder = disc_number_from_path(&cue_path);
        let preferred = from_tag.or(from_folder).unwrap_or_else(|| u32::try_from(index + 1).unwrap_or(u32::MAX));
        let number = unique_cue_disc_number(preferred, &mut used_numbers);
        let label = cue_path.file_name().map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty()).unwrap_or_else(|| format!("Disc {number}"));
        audio::LocalAlbumDisc { number, label, folder_path: format!("{path}/disc/{number}"), track_count: tracks.len() }
    }).collect::<Vec<_>>();

    LocalBrowserAlbumDto {
        id,
        title: group.title.clone(),
        artist,
        artist_names,
        year: normalized_album_year(&dates),
        added_at,
        genres,
        folder_path: path,
        source_id: Some(source_key(source.id)),
        source_label: Some(source.display_name.clone()),
        track_count: group.tracks.len(),
        discs,
        tag_presence: None,
        grouping_notes: Vec::new(),
    }
}

fn album_id_for_view(album: &Album) -> String {
    format!(
        "sqlite:{}:{}",
        album.source_id,
        URL_SAFE_NO_PAD.encode(&album.id.identity)
    )
}

fn parse_sqlite_album_id(value: &str) -> Result<(i64, Vec<u8>), String> {
    let mut parts = value.splitn(3, ':');
    if parts.next() != Some("sqlite") { return Err("SQLITE_ALBUM_ID_INVALID".into()); }
    let source_id = parts.next().and_then(|id| id.parse::<i64>().ok())
        .filter(|id| *id > 0).ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
    let encoded = parts.next().ok_or_else(|| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
    let identity = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "SQLITE_ALBUM_ID_INVALID".to_owned())?;
    if identity.is_empty() || URL_SAFE_NO_PAD.encode(&identity) != encoded {
        return Err("SQLITE_ALBUM_ID_INVALID".into());
    }
    Ok((source_id, identity))
}

fn sqlite_track_locator(source_id: i64, relative: &Path) -> Result<String, String> {
    if source_id <= 0 || relative.is_absolute()
        || !relative.components().all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
    }
    Ok(format!("sqlite-track://{source_id}/{}", URL_SAFE_NO_PAD.encode(relative.as_os_str().as_encoded_bytes())))
}

fn sqlite_cue_locator(source_id: i64, cue_path: &Path, track_number: u32) -> Result<String, String> {
    if source_id <= 0 || !(1..=99).contains(&track_number) || cue_path.is_absolute()
        || !cue_path.components().all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err("SQLITE_CUE_LOCATOR_INVALID".into());
    }
    Ok(format!(
        "sqlite-cue://{source_id}/{}/{track_number}",
        URL_SAFE_NO_PAD.encode(cue_path.as_os_str().as_encoded_bytes())
    ))
}

fn parse_sqlite_cue_locator(value: &str) -> Result<(i64, PathBuf, u32), String> {
    let value = value.strip_prefix("sqlite-cue://").ok_or("SQLITE_CUE_LOCATOR_INVALID")?;
    let (source_text, encoded_and_track) = value.split_once('/').ok_or("SQLITE_CUE_LOCATOR_INVALID")?;
    let source_id = source_text.parse::<i64>().ok().filter(|id| *id > 0).ok_or("SQLITE_CUE_LOCATOR_INVALID")?;
    let (encoded, track_text) = encoded_and_track.rsplit_once('/').ok_or("SQLITE_CUE_LOCATOR_INVALID")?;
    if encoded.is_empty() || encoded.contains(['/', '?', '#']) || track_text.is_empty() {
        return Err("SQLITE_CUE_LOCATOR_INVALID".into());
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "SQLITE_CUE_LOCATOR_INVALID")?;
    if bytes.is_empty() || bytes.contains(&0) || URL_SAFE_NO_PAD.encode(&bytes) != encoded {
        return Err("SQLITE_CUE_LOCATOR_INVALID".into());
    }
    let track_number = track_text.parse::<u32>().ok().filter(|number| (1..=99).contains(number))
        .filter(|number| number.to_string() == track_text).ok_or("SQLITE_CUE_LOCATOR_INVALID")?;
    #[cfg(target_os = "linux")]
    let cue_path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    #[cfg(not(target_os = "linux"))]
    return Err("SQLITE_CUE_PLAYBACK_REQUIRES_LINUX_SECURE_SCANNER".into());
    #[cfg(target_os = "linux")]
    {
        if cue_path.is_absolute() || !cue_path.components().all(|part| matches!(part, std::path::Component::Normal(_))) {
            return Err("SQLITE_CUE_LOCATOR_INVALID".into());
        }
        Ok((source_id, cue_path, track_number))
    }
}

fn parse_sqlite_track_locator(value: &str) -> Result<(i64, PathBuf), String> {
    let value = value.strip_prefix("sqlite-track://").ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
    let (source_text, encoded) = value.split_once('/').ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
    let source_id = source_text.parse::<i64>().ok().filter(|id| *id > 0)
        .ok_or_else(|| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
    if encoded.is_empty() || encoded.contains(['/', '?', '#']) {
        return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
    }
    let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "SQLITE_TRACK_LOCATOR_INVALID".to_owned())?;
    if bytes.is_empty() || bytes.contains(&0) || URL_SAFE_NO_PAD.encode(&bytes) != encoded {
        return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
    }
    #[cfg(target_os = "linux")]
    let relative = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    #[cfg(not(target_os = "linux"))]
    return Err("SQLITE_TAG_EDITOR_REQUIRES_LINUX_SECURE_SCANNER".into());
    #[cfg(target_os = "linux")]
    {
        if relative.is_absolute() || !relative.components().all(|part| matches!(part, std::path::Component::Normal(_))) {
            return Err("SQLITE_TRACK_LOCATOR_INVALID".into());
        }
        Ok((source_id, relative))
    }
}

fn source_key(source_id: i64) -> String {
    format!("sqlite-{source_id}")
}

fn virtual_album_path(album: &Album) -> String {
    format!("sqlite-album://{}", album_id_for_view(album))
}

fn local_browser_album_dto(
    source: &Source,
    album: &Album,
    tracks: &[&AlbumTrack],
) -> LocalBrowserAlbumDto {
    let mut artist_names = Vec::<String>::new();
    let mut album_artists = album.album_artists.clone();
    let mut genres = Vec::new();
    let mut dates = album.date_values.clone();
    let mut discs = std::collections::BTreeMap::<u32, usize>::new();
    for item in tracks {
        let values = item
            .metadata
            .as_ref()
            .filter(|metadata| metadata.current_for_file)
            .and_then(|metadata| metadata.last_success.as_ref());
        let number = values
            .and_then(|values| values.disc_number)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .or_else(|| disc_number_from_path(&item.track.relative_path))
            .unwrap_or(1);
        if let Some(values) = values {
            for artist in &values.track_artists {
                if !artist_names.contains(artist) {
                    artist_names.push(artist.clone());
                }
            }
            for artist in &values.album_artists {
                if !album_artists.contains(artist) {
                    album_artists.push(artist.clone());
                }
                if !artist_names.contains(artist) {
                    artist_names.push(artist.clone());
                }
            }
            if let Some(genre) = &values.genre {
                if !genres.contains(genre) {
                    genres.push(genre.clone());
                }
            }
            if let Some(date) = &values.date_text {
                if !dates.contains(date) {
                    dates.push(date.clone());
                }
            }
            *discs.entry(number).or_default() += 1;
        } else {
            *discs.entry(number).or_default() += 1;
        }
    }
    let path = virtual_album_path(album);
    let display_artist = local_album_display_artist(tracks);
    LocalBrowserAlbumDto {
        id: album_id_for_view(album),
        title: album.title.clone(),
        artist: display_artist,
        artist_names,
        year: normalized_album_year(&dates),
        added_at: album.first_seen_ns.and_then(format_added_at),
        genres,
        folder_path: path.clone(),
        source_id: Some(source_key(source.id)),
        source_label: Some(source.display_name.clone()),
        track_count: album.track_count,
        discs: discs
            .into_iter()
            .map(|(number, track_count)| audio::LocalAlbumDisc {
                number,
                label: format!("Disc {number}"),
                folder_path: format!("{path}/disc/{number}"),
                track_count,
            })
            .collect(),
        tag_presence: None,
        grouping_notes: album.grouping_notes.clone(),
    }
}

fn format_added_at(first_seen_ns: i64) -> Option<String> {
    if first_seen_ns < 0 { return None; }
    let seconds = u64::try_from(first_seen_ns.checked_div(1_000_000_000)?).ok()?;
    let days = i64::try_from(seconds / 86_400).ok()?;
    let day_seconds = seconds % 86_400;
    let z = days.checked_add(719_468)?;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    if !(1970..=9999).contains(&year) { return None; }
    Some(format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        day_seconds / 3_600, day_seconds % 3_600 / 60, day_seconds % 60))
}

/// Apply the established Local presentation rule without inferring Album
/// Artist from track artists: use a consistent single Album Artist, otherwise
/// a dominant track artist at the historical 80% threshold, or Various Artists.
fn local_album_display_artist(tracks: &[&AlbumTrack]) -> String {
    let mut album_artist: Option<(String, String)> = None;
    let mut consistent_album_artist = !tracks.is_empty();
    let mut artist_counts = std::collections::BTreeMap::<String, (String, usize)>::new();

    for track in tracks {
        let values = track
            .metadata
            .as_ref()
            .filter(|metadata| metadata.current_for_file)
            .and_then(|metadata| metadata.last_success.as_ref());
        let explicit_album_artists = values
            .map(|values| {
                values
                    .album_artists
                    .iter()
                    .map(|artist| artist.trim())
                    .filter(|artist| !artist.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if explicit_album_artists.len() != 1 {
            consistent_album_artist = false;
        } else {
            let name = explicit_album_artists[0];
            let key = name.to_lowercase();
            match &mut album_artist {
                Some((previous, display)) if *previous == key => {
                    if name > display.as_str() {
                        *display = name.to_owned();
                    }
                }
                Some(_) => consistent_album_artist = false,
                None => album_artist = Some((key, name.to_owned())),
            }
        }

        if let Some(name) = values
            .and_then(|values| values.track_artists.first())
            .map(String::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let key = name.to_lowercase();
            let entry = artist_counts
                .entry(key)
                .or_insert_with(|| (name.to_owned(), 0));
            entry.1 += 1;
            if name > entry.0.as_str() {
                entry.0 = name.to_owned();
            }
        }
    }

    if consistent_album_artist {
        if let Some((_, display)) = album_artist {
            return display;
        }
    }
    let threshold = tracks.len() - tracks.len() / 5;
    if let Some((display, count)) = artist_counts
        .values()
        .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
    {
        if *count >= threshold {
            return display.clone();
        }
        return "Various Artists".to_owned();
    }
    "Artista Desconhecido".to_owned()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderSelectionDto {
    token: String,
    display_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDto {
    id: i64,
    display_name: String,
    generation: i64,
    track_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveSourceDto {
    source_id: i64,
    display_name: String,
    removed_track_count: usize,
    queue_stopped: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexMetricsDto {
    sources_examined: usize,
    entries_visited: usize,
    files_examined: usize,
    files_new: usize,
    files_modified: usize,
    files_unchanged: usize,
    files_removed: usize,
    records_written: usize,
    files_opened_for_extraction: usize,
    files_processed_by_parser: usize,
    successful_extractions: usize,
    files_without_tags: usize,
    unsupported_formats: usize,
    extraction_failures: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteIndexProgressDto {
    operation_id: String,
    operation: &'static str,
    source_name: String,
    phase: &'static str,
    entries_visited: usize,
    files_examined: usize,
    audio_files_found: usize,
    metadata_processed: usize,
    metadata_total: usize,
    identified_albums: usize,
    identified_artists: usize,
    counts_provisional: bool,
}

fn index_progress_dto(
    operation_id: &str,
    operation: &'static str,
    source_name: &str,
    progress: IndexProgress,
) -> SqliteIndexProgressDto {
    let phase = match progress.phase {
        IndexPhase::Scanning => "scanning",
        IndexPhase::ReadingMetadata => "metadata",
        IndexPhase::Grouping => "grouping",
        IndexPhase::Persisting => "persisting",
        IndexPhase::Completed => "completed",
    };
    SqliteIndexProgressDto {
        operation_id: operation_id.to_owned(),
        operation,
        source_name: source_name.to_owned(),
        phase,
        entries_visited: progress.entries_visited,
        files_examined: progress.files_examined,
        audio_files_found: progress.audio_files_found,
        metadata_processed: progress.metadata_processed,
        metadata_total: progress.metadata_total,
        identified_albums: progress.identified_albums,
        identified_artists: progress.identified_artists,
        counts_provisional: progress.counts_provisional,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexResultDto {
    source: SourceDto,
    metrics: IndexMetricsDto,
    view: LibraryViewDto,
}

#[derive(Debug, Serialize)]
pub struct SearchLocalTracksDto {
    pub tracks: Vec<audio::LocalItem>,
    pub limited: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryViewDto {
    source_id: i64,
    generation: i64,
    albums: Vec<AlbumDto>,
    artists: Vec<ArtistDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDto {
    source_id: i64,
    identity: String,
    title: String,
    album_artists: Vec<String>,
    track_count: usize,
    stale_track_count: usize,
    date_values: Vec<String>,
    ambiguous: bool,
    grouping_notes: Vec<String>,
    directory_display: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDto {
    name: String,
    appears_as_track_artist: bool,
    appears_as_album_artist: bool,
    track_count: usize,
    album_count: usize,
    stale_track_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDto {
    relative_path: String,
    metadata_stale: bool,
    title: Option<String>,
    track_artists: Vec<String>,
    album_artists: Vec<String>,
    album: Option<String>,
    genre: Option<String>,
    date_text: Option<String>,
    track_number: Option<i64>,
    disc_number: Option<i64>,
    duration_ms: Option<i64>,
    sample_rate: Option<i64>,
    channels: Option<i64>,
    file_format: Option<String>,
    metadata_status: Option<String>,
}

fn lock_error<T>(result: std::sync::LockResult<T>, name: &str) -> Result<T, String> {
    result.map_err(|_| format!("{name} is unavailable"))
}

fn source_dto(catalog: &Catalog, source: &Source) -> Result<SourceDto, String> {
    Ok(SourceDto {
        id: source.id,
        display_name: source.display_name.clone(),
        generation: source.generation,
        track_count: catalog.track_count_for(source.id)?,
    })
}

fn metrics_dto(metrics: &Metrics) -> IndexMetricsDto {
    IndexMetricsDto {
        sources_examined: metrics.sources_examined,
        entries_visited: metrics.entries_visited,
        files_examined: metrics.files_examined,
        files_new: metrics.files_new,
        files_modified: metrics.files_modified,
        files_unchanged: metrics.files_unchanged,
        files_removed: metrics.files_removed,
        records_written: metrics.records_written,
        files_opened_for_extraction: metrics.files_opened_for_extraction,
        files_processed_by_parser: metrics.files_processed_by_parser,
        successful_extractions: metrics.successful_extractions,
        files_without_tags: metrics.files_without_tags,
        unsupported_formats: metrics.unsupported_formats,
        extraction_failures: metrics.extraction_failures,
    }
}

fn view_dto(view: &SourceLibraryView) -> LibraryViewDto {
    LibraryViewDto {
        source_id: view.source_id(),
        generation: view.generation(),
        albums: view.albums().iter().map(album_dto).collect(),
        artists: view.artists().iter().map(artist_dto).collect(),
    }
}

fn album_dto(album: &Album) -> AlbumDto {
    AlbumDto {
        source_id: album.source_id,
        identity: encode_hex(&album.id.identity),
        title: album.title.clone(),
        album_artists: album.album_artists.clone(),
        track_count: album.track_count,
        stale_track_count: album.stale_track_count,
        date_values: album.date_values.clone(),
        ambiguous: album.ambiguous,
        grouping_notes: album.grouping_notes.clone(),
        directory_display: album.directory_hint.to_string_lossy().into_owned(),
    }
}

fn artist_dto(artist: &Artist) -> ArtistDto {
    ArtistDto {
        name: artist.name.clone(),
        appears_as_track_artist: artist.appears_as_track_artist,
        appears_as_album_artist: artist.appears_as_album_artist,
        track_count: artist.track_count,
        album_count: artist.album_count,
        stale_track_count: artist.stale_track_count,
    }
}

fn track_dto(track: &AlbumTrack) -> TrackDto {
    let metadata = track.metadata.as_ref();
    let values: Option<&TrackMetadataValues> = metadata
        .filter(|metadata| metadata.current_for_file)
        .and_then(|metadata| metadata.last_success.as_ref());
    TrackDto {
        relative_path: track.track.relative_path.to_string_lossy().into_owned(),
        metadata_stale: track.metadata_stale,
        title: values.and_then(|values| values.title.clone()),
        track_artists: values
            .map(|values| values.track_artists.clone())
            .unwrap_or_default(),
        album_artists: values
            .map(|values| values.album_artists.clone())
            .unwrap_or_default(),
        album: values.and_then(|values| values.album.clone()),
        genre: values.and_then(|values| values.genre.clone()),
        date_text: values.and_then(|values| values.date_text.clone()),
        track_number: values.and_then(|values| values.track_number),
        disc_number: values.and_then(|values| values.disc_number),
        duration_ms: values.and_then(|values| values.duration_ms),
        sample_rate: values.and_then(|values| values.sample_rate),
        channels: values.and_then(|values| values.channels),
        file_format: metadata.map(|metadata| metadata.file_format.clone()),
        metadata_status: metadata.map(metadata_status_name),
    }
}

fn metadata_status_name(metadata: &TrackMetadata) -> String {
    use sonante_sqlite_catalog::MetadataStatus;
    match metadata.status {
        MetadataStatus::ParsedWithTags => "parsedWithTags",
        MetadataStatus::ParsedWithoutTags => "parsedWithoutTags",
        MetadataStatus::Unsupported => "unsupported",
        MetadataStatus::Failed => "failed",
    }
    .to_owned()
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if value.len() % 2 != 0 {
        return Err("invalid album identity".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            match (digit(pair[0]), digit(pair[1])) {
                (Some(high), Some(low)) => Ok((high << 4) | low),
                _ => Err("invalid album identity".into()),
            }
        })
        .collect()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| format!("SQLite operation failed: {error}"))?
}

#[tauri::command]
pub async fn sqlite_library_choose_folder(
    state: State<'_, SqliteLibraryState>,
) -> Result<Option<FolderSelectionDto>, String> {
    let Some(folder) = rfd::AsyncFileDialog::new()
        .set_title("Adicionar uma pasta à biblioteca Local")
        .pick_folder()
        .await
    else {
        return Ok(None);
    };
    let path = folder.path().to_path_buf();
    let token = format!(
        "folder-{}",
        state.inner.next_selection.fetch_add(1, Ordering::Relaxed)
    );
    lock_error(state.inner.selected_roots.lock(), "folder selection")?
        .insert(token.clone(), path.clone());
    Ok(Some(FolderSelectionDto {
        token,
        display_path: path.to_string_lossy().into_owned(),
    }))
}

fn revoke_folder_selection(inner: &Inner, token: &str) -> Result<bool, String> {
    Ok(lock_error(inner.selected_roots.lock(), "folder selection")?
        .remove(token)
        .is_some())
}

#[tauri::command]
pub async fn sqlite_library_cancel_folder_selection(
    state: State<'_, SqliteLibraryState>,
    selected_folder_token: String,
) -> Result<(), String> {
    revoke_folder_selection(&state.inner, &selected_folder_token)?;
    Ok(())
}

#[tauri::command]
pub async fn sqlite_library_sources(
    state: State<'_, SqliteLibraryState>,
) -> Result<Vec<SourceDto>, String> {
    let state = state.inner.clone();
    blocking(move || SqliteLibraryState { inner: state }.list_sources()).await
}

fn public_profile_legacy_migration_enabled(debug_build: bool, experimental_package: bool, null_output_test: bool) -> bool {
    !debug_build && !experimental_package && !null_output_test
}

fn authorized_legacy_roots(config: &crate::ConfigState) -> Result<Option<Vec<String>>, String> {
    // Development, experimental-package and null-output profiles deliberately
    // use isolated XDG roots. They must never treat those roots as the public
    // 0.6.3 profile being migrated.
    if !public_profile_legacy_migration_enabled(cfg!(debug_assertions), cfg!(feature = "experimental-package"), crate::null_output_test_mode()) {
        return Ok(None);
    }
    let config = config.0.lock().map_err(|_| "CONFIG_UNAVAILABLE")?.clone();
    if config.first_run || !config.sources.unwrap_or_default().local {
        return Ok(None);
    }
    let roots = crate::config::AppConfig::load_stored_local_folders()?;
    if roots.iter().any(|root| !config.local_folders.iter().any(|current| current == root)) {
        return Err("SQLITE_LEGACY_SOURCE_CONFIG_CHANGED".into());
    }
    Ok(Some(roots))
}

#[tauri::command]
pub async fn sqlite_library_legacy_source_migration_status(
    state: State<'_, SqliteLibraryState>,
    config: State<'_, crate::ConfigState>,
) -> Result<LegacySourceMigrationStatusDto, String> {
    let Some(roots) = authorized_legacy_roots(&config)? else {
        return Ok(LegacySourceMigrationStatusDto { eligible: false, pending_sources: 0 });
    };
    let state = SqliteLibraryState { inner: state.inner.clone() };
    blocking(move || state.legacy_source_migration_status(&roots)).await
}

#[tauri::command]
pub async fn sqlite_library_import_legacy_sources(
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
    config: State<'_, crate::ConfigState>,
    operation_id: String,
) -> Result<LegacySourceImportResultDto, String> {
    if operation_id.is_empty() || operation_id.len() > 64
        || !operation_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("SQLITE_OPERATION_ID_INVALID".into());
    }
    let roots = authorized_legacy_roots(&config)?
        .filter(|roots| !roots.is_empty())
        .ok_or_else(|| "SQLITE_LEGACY_SOURCE_MIGRATION_NOT_AVAILABLE".to_owned())?;
    let state = SqliteLibraryState { inner: state.inner.clone() };
    blocking(move || state.import_legacy_sources_with_progress(&roots, |source_name, progress| {
        let payload = index_progress_dto(&operation_id, "legacyImport", source_name, progress);
        let _ = app.emit("sqlite-library-index-progress", payload);
    })).await
}

#[tauri::command]
pub async fn sqlite_library_local_albums(
    state: State<'_, SqliteLibraryState>,
) -> Result<Vec<LocalBrowserAlbumDto>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || state.local_browser_albums()).await
}

#[tauri::command]
pub async fn sqlite_library_local_album_tracks(
    state: State<'_, SqliteLibraryState>,
    album_id: String,
) -> Result<Vec<audio::LocalAlbumDiscTracks>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || state.local_browser_album_tracks(&album_id)).await
}

#[tauri::command]
pub async fn sqlite_library_search_tracks(
    state: State<'_, SqliteLibraryState>,
    query: String,
) -> Result<SearchLocalTracksDto, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || state.search_local_tracks(&query)).await
}

#[tauri::command]
pub async fn sqlite_library_album_cover(
    state: State<'_, SqliteLibraryState>,
    album_id: String,
) -> Result<Option<String>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || state.album_cover(&album_id)).await
}

#[tauri::command]
pub async fn sqlite_library_album_for_track(
    state: State<'_, SqliteLibraryState>,
    track_locator: String,
) -> Result<Option<String>, String> {
    let state = SqliteLibraryState { inner: state.inner.clone() };
    blocking(move || state.album_for_track(&track_locator)).await
}

#[cfg(test)]
fn index_selected_folder(
    app_state: Arc<Inner>,
    selected_folder_token: String,
) -> Result<IndexResultDto, String> {
    index_selected_folder_with_progress(app_state, selected_folder_token, |_, _| {})
}

fn index_selected_folder_with_progress<F>(
    app_state: Arc<Inner>,
    selected_folder_token: String,
    mut progress: F,
) -> Result<IndexResultDto, String>
where
    F: FnMut(&str, IndexProgress),
{
    let state = SqliteLibraryState {
        inner: app_state.clone(),
    };
    let path = lock_error(app_state.selected_roots.lock(), "folder selection")?
        .get(&selected_folder_token)
        .cloned()
        .ok_or_else(|| "FOLDER_SELECTION_EXPIRED: choose the folder again".to_owned())?;
    let source_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Local music".to_owned());
    let result = state.add_and_index_with_progress(&path, |event| progress(&source_name, event));
    lock_error(app_state.selected_roots.lock(), "folder selection")?.remove(&selected_folder_token);
    result
}

#[tauri::command]
pub async fn sqlite_library_add_and_index(
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
    selected_folder_token: String,
    operation_id: String,
) -> Result<IndexResultDto, String> {
    if operation_id.is_empty()
        || operation_id.len() > 64
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("SQLITE_OPERATION_ID_INVALID".into());
    }
    let app_state = state.inner.clone();
    blocking(move || {
        index_selected_folder_with_progress(
            app_state,
            selected_folder_token,
            |source_name, progress| {
                let payload = index_progress_dto(&operation_id, "add", source_name, progress);
                let _ = app.emit("sqlite-library-index-progress", payload);
            },
        )
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_verify_source(
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
    operation_id: String,
) -> Result<IndexResultDto, String> {
    if operation_id.is_empty()
        || operation_id.len() > 64
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("SQLITE_OPERATION_ID_INVALID".into());
    }
    let app_state = state.inner.clone();
    blocking(move || {
        SqliteLibraryState { inner: app_state }.verify_with_progress(
            source_id,
            |source_name, progress| {
                let payload = index_progress_dto(&operation_id, "verify", source_name, progress);
                let _ = app.emit("sqlite-library-index-progress", payload);
            },
        )
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_reindex_source(
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
    operation_id: String,
) -> Result<IndexResultDto, String> {
    if operation_id.is_empty()
        || operation_id.len() > 64
        || !operation_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("SQLITE_OPERATION_ID_INVALID".into());
    }
    let app_state = state.inner.clone();
    blocking(move || {
        SqliteLibraryState { inner: app_state }.reindex_with_progress(source_id, |source_name, progress| {
            let payload = index_progress_dto(&operation_id, "reindex", source_name, progress);
            let _ = app.emit("sqlite-library-index-progress", payload);
        })
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_remove_source(
    app: AppHandle,
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
) -> Result<RemoveSourceDto, String> {
    let app_state = state.inner.clone();
    blocking(move || {
        let audio_state = app.state::<AudioState>();
        let mut audio = audio_state.0.lock().map_err(|_| "Playback unavailable".to_owned())?;
        let (mut result, queue_stopped) = audio.remove_sqlite_source_with(source_id, || {
            SqliteLibraryState { inner: app_state }.remove_source(source_id)
        })?;
        result.queue_stopped = queue_stopped;
        Ok(result)
    }).await
}

#[tauri::command]
pub async fn sqlite_library_open_view(
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
) -> Result<LibraryViewDto, String> {
    let app_state = state.inner.clone();
    blocking(move || SqliteLibraryState { inner: app_state }.open_view(source_id)).await
}

#[tauri::command]
pub async fn sqlite_library_albums(
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
) -> Result<Vec<AlbumDto>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || {
        Ok(state
            .current_view(source_id)?
            .albums()
            .iter()
            .map(album_dto)
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_artists(
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
) -> Result<Vec<ArtistDto>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || {
        Ok(state
            .current_view(source_id)?
            .artists()
            .iter()
            .map(artist_dto)
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_albums_for_artist(
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
    artist_name: String,
) -> Result<Vec<AlbumDto>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || {
        Ok(state
            .current_view(source_id)?
            .albums_for_artist(&artist_name)
            .map(album_dto)
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_tracks_for_album(
    state: State<'_, SqliteLibraryState>,
    source_id: i64,
    album_identity: String,
) -> Result<Vec<TrackDto>, String> {
    let state = SqliteLibraryState {
        inner: state.inner.clone(),
    };
    blocking(move || {
        let identity = decode_hex(&album_identity)?;
        let view = state.current_view(source_id)?;
        let album_id = AlbumId {
            source_id,
            identity,
        };
        view.tracks_for_album(&album_id)
            .map(|tracks| tracks.map(track_dto).collect())
            .ok_or_else(|| "ALBUM_NOT_FOUND: reopen the library view".into())
    })
    .await
}

#[tauri::command]
pub async fn sqlite_library_album_doctor(
    state: State<'_, SqliteLibraryState>,
    album_id: String,
) -> Result<library_doctor::Report, DoctorError> {
    let state = SqliteLibraryState { inner: state.inner.clone() };
    tauri::async_runtime::spawn_blocking(move || state.album_doctor(&album_id))
        .await
        .map_err(|_| DoctorError::TaskFailed)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, EnabledSources};
    use lofty::file::TaggedFileExt;
    use sonante_sqlite_catalog::MetadataStatus;
    use std::{fs, sync::mpsc, thread, time::Duration};
    use tempfile::tempdir;

    #[test]
    fn local_collection_relational_snapshots_reopen_and_detect_sample_changes() {
        for count in [5, 20, 100, 300] {
            let temp = tempdir().unwrap();
            let root = temp.path().join("música com espaços").join("álbum longo");
            fs::create_dir_all(&root).unwrap();
            for n in 0..count {
                fs::write(root.join(format!("{n:03} faixa com nome longo e unicode 漢字.wav")), synthetic_wav(&format!("Track {n}"), "Album", "Artist")).unwrap();
            }
            let db = temp.path().join("catalog.sqlite");
            let mut catalog = Catalog::open(&db).unwrap();
            let source = catalog.add_source(&root, "Music").unwrap().source_id;
            let view = catalog.source_library_view(source).unwrap();
            assert_eq!(view.albums().len(), 1);
            let album = album_id_for_view(&view.albums()[0]);
            let collection = catalog.collection_create("Test").unwrap();
            let member = collection_local_add(&mut catalog, collection, &album).unwrap();
            assert_eq!(collection_local_add(&mut catalog, collection, &album).unwrap_err(), "COLLECTION_DUPLICATE_MEMBER");
            drop(catalog);
            let mut catalog = Catalog::open(&db).unwrap();
            let saved = catalog.collection_member(collection, member).unwrap();
            assert!(saved.reference.starts_with("local2:"));
            assert!(saved.reference.len() < 100);
            assert_eq!(catalog.collection_local_evidence(collection, member).unwrap().files.len(), count);
            assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Ok(album));
            let path = root.join("000 faixa com nome longo e unicode 漢字.wav");
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            let mut bytes = fs::read(&path).unwrap();
            let last = bytes.len()-1; bytes[last] ^= 1;
            fs::write(&path, bytes).unwrap();
            fs::OpenOptions::new().write(true).open(&path).unwrap().set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
            assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Err("unresolved"));
            assert_eq!(catalog.collection_member(collection, member).unwrap(), saved);
            fs::remove_file(&path).unwrap();
            assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Err("unresolved"));
            fs::rename(&root, temp.path().join("offline")).unwrap();
            assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Err("unavailable"));
            assert_eq!(catalog.collection_member(collection, member).unwrap(), saved);
        }
    }

    #[test]
    fn local_collection_relational_insert_rolls_back_on_bad_evidence() {
        use sonante_sqlite_catalog::collections::{LocalEvidence, LocalEvidenceFile, NewMember};
        let temp = tempdir().unwrap();
        let mut catalog = Catalog::open(temp.path().join("catalog.sqlite")).unwrap();
        let id = catalog.collection_create("Test").unwrap();
        let item = NewMember { source:"local".into(), scope:"local1:1".into(), reference:"local2:fixture".into(), title:"Album".into(), artist:None };
        let evidence = LocalEvidence { album_locator:"sqlite:1:00".into(), files:vec![
            LocalEvidenceFile { path:b"one".to_vec(), size:1, mtime_ns:0, sample_sha256:vec![0;32] },
            LocalEvidenceFile { path:b"two".to_vec(), size:1, mtime_ns:0, sample_sha256:vec![0;2] },
        ], segments:vec![] };
        assert!(catalog.collection_add_local_evidence(id, item, &evidence, None).is_err());
        assert!(catalog.collection_members(id).unwrap().is_empty());
    }

    #[test]
    fn local_collection_distinguishes_homonyms_and_detects_group_changes() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("music");
        for name in ["one", "two"] {
            let directory = root.join(name);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("track.wav"),
                synthetic_wav("Track", "Same Album", "Same Artist"),
            )
            .unwrap();
        }
        let mut catalog = Catalog::open(temp.path().join("catalog.sqlite")).unwrap();
        let source = catalog.add_source(&root, "Music").unwrap().source_id;
        let view = catalog.source_library_view(source).unwrap();
        assert_eq!(view.albums().len(), 2);
        let first_album = view
            .albums()
            .iter()
            .find(|a| a.directory_hint == Path::new("one"))
            .unwrap();
        let second_album = view
            .albums()
            .iter()
            .find(|a| a.directory_hint == Path::new("two"))
            .unwrap();
        let first = collection_local_capture(&catalog, &album_id_for_view(first_album)).unwrap();
        let second = collection_local_capture(&catalog, &album_id_for_view(second_album)).unwrap();
        assert_ne!(first.reference, second.reference);
        catalog.verify_source(source).unwrap();
        assert_eq!(
            collection_local_resolve(&catalog, &first.scope, &first.reference),
            "available"
        );
        fs::write(
            root.join("one").join("another.wav"),
            synthetic_wav("Another", "Same Album", "Same Artist"),
        )
        .unwrap();
        catalog.verify_source(source).unwrap();
        assert_eq!(
            collection_local_resolve(&catalog, &first.scope, &first.reference),
            "unresolved"
        );
        assert_eq!(
            collection_local_resolve(&catalog, &second.scope, &second.reference),
            "available"
        );
    }

    #[test]
    fn local_collection_single_image_cue_detects_sheet_changes() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("music");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("image.wv"), synthetic_wavpack()).unwrap();
        let cue=b"PERFORMER \"Artist\"\nTITLE \"Album\"\nFILE \"image.wv\" WAVE\nTRACK 01 AUDIO\nTITLE \"First\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"Second\"\nINDEX 01 00:00:01\n";
        fs::write(root.join("album.cue"), cue).unwrap();
        let mut catalog = Catalog::open(temp.path().join("catalog.sqlite")).unwrap();
        let source = catalog.add_source(&root, "Music").unwrap().source_id;
        let view = catalog.source_library_view(source).unwrap();
        let groups = cue_album_groups(view.cue_tracks());
        assert_eq!(groups.len(), 1);
        let locator = cue_album_id(source, &groups[0]);
        let item = collection_local_capture(&catalog, &locator).unwrap();
        assert_eq!(collection_local_resolve(&catalog, &item.scope, &item.reference), "available");
        let collection = catalog.collection_create("Cue").unwrap();
        let member = collection_local_add(&mut catalog, collection, &locator).unwrap();
        let saved = catalog.collection_member(collection, member).unwrap();
        let evidence = catalog.collection_local_evidence(collection, member).unwrap();
        assert_eq!(evidence.files.len(), 1);
        assert_eq!(evidence.segments.len(), 2);
        assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Ok(locator.clone()));
        fs::write(root.join("other.cue"), b"TITLE \"Other\"\nFILE \"image.wv\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n").unwrap();
        catalog.verify_source(source).unwrap();
        assert_eq!(collection_local_resolve(&catalog, &item.scope, &item.reference), "unresolved");
        assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Err("unresolved"));
        fs::write(root.join("album.cue"), b"invalid").unwrap();
        assert_eq!(collection_local_resolve(&catalog, &item.scope, &item.reference), "unresolved");
    }

    #[test]
    fn local_collection_snapshot_persists_and_rejects_changed_members() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let db = temp.path().join("catalog.sqlite");
        let mut catalog = Catalog::open(&db).unwrap();
        let source = catalog.add_source(&root, "Test").unwrap().source_id;
        let view = catalog.source_library_view(source).unwrap();
        let album = album_id_for_view(&view.albums()[0]);
        let item = collection_local_capture(&catalog, &album).unwrap();
        let collection = catalog.collection_create("Test").unwrap();
        let member = catalog.collection_add(collection, item).unwrap();
        assert_eq!(collection_local_add(&mut catalog, collection, &album).unwrap_err(), "COLLECTION_DUPLICATE_MEMBER");
        drop(catalog);
        let catalog = Catalog::open(&db).unwrap();
        let saved = catalog.collection_member(collection, member).unwrap();
        assert_eq!(
            collection_local_resolve(&catalog, &saved.scope, &saved.reference),
            "available"
        );
        let first_path = root.join("01-first.wav");
        let modified = fs::metadata(&first_path).unwrap().modified().unwrap();
        let mut bytes = fs::read(&first_path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&first_path, bytes).unwrap();
        fs::OpenOptions::new().write(true).open(&first_path).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
        assert_eq!(collection_local_resolve(&catalog, &saved.scope, &saved.reference), "unresolved");
        fs::remove_file(root.join("01-first.wav")).unwrap();
        assert_eq!(
            collection_local_resolve(&catalog, &saved.scope, &saved.reference),
            "unresolved"
        );
        assert_eq!(
            catalog.collection_member(collection, member).unwrap(),
            saved
        );
        fs::rename(&root, temp.path().join("offline")).unwrap();
        assert_eq!(
            collection_local_resolve(&catalog, &saved.scope, &saved.reference),
            "unavailable"
        );
    }

    #[test]
    fn cue_disc_number_allocation_is_finite_for_duplicate_maximum_values() {
        let mut used = std::collections::BTreeSet::new();
        assert_eq!(unique_cue_disc_number(u32::MAX, &mut used), u32::MAX);
        assert_eq!(unique_cue_disc_number(u32::MAX, &mut used), 1);
        assert_eq!(unique_cue_disc_number(1, &mut used), 2);
    }

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(id);
        value.extend_from_slice(&(data.len() as u32).to_le_bytes());
        value.extend_from_slice(data);
        if data.len() % 2 != 0 {
            value.push(0);
        }
        value
    }

    fn synthetic_wav(title: &str, album: &str, artist: &str) -> Vec<u8> {
        let mut wave = b"WAVE".to_vec();
        let mut format = Vec::new();
        format.extend(1_u16.to_le_bytes());
        format.extend(2_u16.to_le_bytes());
        format.extend(44_100_u32.to_le_bytes());
        format.extend(176_400_u32.to_le_bytes());
        format.extend(4_u16.to_le_bytes());
        format.extend(16_u16.to_le_bytes());
        wave.extend(chunk(b"fmt ", &format));
        wave.extend(chunk(b"data", &vec![0; 4_410 * 4]));
        let mut info = b"INFO".to_vec();
        for (id, text) in [(b"INAM", title), (b"IART", artist), (b"IPRD", album)] {
            let mut text = text.as_bytes().to_vec();
            text.push(0);
            info.extend(chunk(id, &text));
        }
        wave.extend(chunk(b"LIST", &info));
        let mut riff = b"RIFF".to_vec();
        riff.extend((wave.len() as u32).to_le_bytes());
        riff.extend(wave);
        riff
    }

    fn synthetic_wavpack() -> Vec<u8> {
        include_str!("../../experiments/sqlite-catalog/src/fixtures/synthetic-wavpack.wv.hex")
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn fixture_root(parent: &Path) -> PathBuf {
        let root = parent.join("synthetic-library");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("01-first.wav"),
            synthetic_wav("First", "Fixture Album", "Fixture Artist"),
        )
        .unwrap();
        fs::write(
            root.join("02-second.wav"),
            synthetic_wav("Second", "Fixture Album", "Fixture Artist"),
        )
        .unwrap();
        root
    }

    #[test]
    fn legacy_migration_admission_never_reads_an_isolated_profile_as_public() {
        assert!(public_profile_legacy_migration_enabled(false, false, false));
        assert!(!public_profile_legacy_migration_enabled(true, false, false));
        assert!(!public_profile_legacy_migration_enabled(false, true, false));
        assert!(!public_profile_legacy_migration_enabled(false, false, true));
    }

    #[test]
    fn legacy_public_roots_import_idempotently_and_removal_does_not_reimport_them() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("catalog.sqlite3")).unwrap();
        let roots = vec![root.to_string_lossy().into_owned(), root.to_string_lossy().into_owned()];
        assert_eq!(state.legacy_source_migration_status(&roots).unwrap().pending_sources, 1);
        let mut phases = Vec::new();
        let first = state.import_legacy_sources_with_progress(&roots, |name, event| {
            phases.push((name.to_owned(), event.phase));
        }).unwrap();
        assert_eq!(first.imported_sources, 1);
        assert_eq!(first.already_present_sources, 0);
        assert!(first.failed_source_names.is_empty());
        assert!(phases.iter().any(|(_, phase)| *phase == IndexPhase::Completed));
        assert_eq!(state.catalog().unwrap().sources().unwrap().len(), 1);
        assert_eq!(state.legacy_source_migration_status(&roots).unwrap().pending_sources, 0);

        let repeated = state.import_legacy_sources_with_progress(&roots, |_, _| panic!("completed root must not be scanned again")).unwrap();
        assert_eq!(repeated.imported_sources, 0);
        assert!(repeated.failed_source_names.is_empty());
        let source_id = state.catalog().unwrap().sources().unwrap()[0].id;
        state.catalog().unwrap().remove_source(source_id).unwrap();
        assert_eq!(state.legacy_source_migration_status(&roots).unwrap().pending_sources, 0);
        let after_removal = state.import_legacy_sources_with_progress(&roots, |_, _| panic!("removed imported root must not be silently re-added")).unwrap();
        assert_eq!(after_removal.imported_sources, 0);
        assert!(state.catalog().unwrap().sources().unwrap().is_empty());
    }

    #[test]
    fn legacy_import_recovers_catalog_commit_without_rescanning_and_keeps_offline_roots_pending() {
        let temp = tempdir().unwrap();
        let existing_root = fixture_root(temp.path());
        let offline_root = temp.path().join("disconnected-share");
        let state = SqliteLibraryState::open(temp.path().join("catalog.sqlite3")).unwrap();
        let existing = state.add_and_index(&existing_root).unwrap();
        let before_generation = existing.source.generation;
        let roots = vec![existing_root.to_string_lossy().into_owned(), offline_root.to_string_lossy().into_owned()];
        let result = state.import_legacy_sources_with_progress(&roots, |name, _| assert_ne!(name, "disconnected-share")).unwrap();
        assert_eq!(result.imported_sources, 0);
        assert_eq!(result.already_present_sources, 1);
        assert_eq!(result.failed_source_names, vec!["disconnected-share"]);
        let sources = state.catalog().unwrap().sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].generation, before_generation);
        assert_eq!(state.legacy_source_migration_status(&roots).unwrap().pending_sources, 1);
    }

    #[test]
    fn cue_range_transport_is_transient_canonical_and_frame_exact() {
        let path = "/private/source/Disc 01.wav";
        let encoded = sqlite_cue_range_uri(path, 501, Some(1002)).unwrap();
        assert!(encoded.starts_with("sonante-cue-range://"));
        let decoded = SqliteLibraryState::parse_sqlite_cue_range_uri(&encoded).unwrap();
        assert_eq!(decoded.path, path);
        assert_eq!(decoded.start_frame, 501);
        assert_eq!(decoded.end_frame, Some(1002));
        assert!(SqliteLibraryState::is_sqlite_cue_range_uri(&encoded));

        let open = SqliteLibraryState::parse_sqlite_cue_range_uri(
            &sqlite_cue_range_uri(path, 1002, None).unwrap(),
        ).unwrap();
        assert_eq!(open.end_frame, None);
        assert!(sqlite_cue_range_uri("relative.wav", 0, None).is_err());
        assert!(sqlite_cue_range_uri(path, 10, Some(10)).is_err());
        assert!(SqliteLibraryState::parse_sqlite_cue_range_uri("sonante-cue-range://bad/1/open").is_err());
    }

    #[test]
    fn cue_playback_refuses_changed_sheet_and_replaced_physical_audio() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempdir().unwrap();
        let root = temp.path().join("changed-sheet");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("image.wav"), synthetic_wav("Long", "Album", "Artist")).unwrap();
        let sheet = b"TITLE \"Album\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"Part\"\nINDEX 01 00:00:00\n";
        fs::write(root.join("album.cue"), sheet).unwrap();
        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let locator = sqlite_cue_locator(indexed.source.id, Path::new("album.cue"), 1).unwrap();
        assert!(state.resolve_playback_locator_with_roots(&locator, &[]).is_ok());
        let edited_sheet = String::from_utf8(sheet.to_vec()).unwrap().replace("Part", "Edit");
        fs::write(root.join("album.cue"), edited_sheet).unwrap();
        assert!(state.resolve_playback_locator_with_roots(&locator, &[]).is_err(),
            "a saved CUE locator must not authorize a sheet changed after indexing");

        let root = temp.path().join("replaced-audio");
        fs::create_dir_all(&root).unwrap();
        let audio_path = root.join("image.wav");
        fs::write(&audio_path, synthetic_wav("Long", "Album", "Artist")).unwrap();
        fs::write(root.join("album.cue"), sheet).unwrap();
        let replacement_state = SqliteLibraryState::open(temp.path().join("catalog/replacement.sqlite3")).unwrap();
        let indexed = replacement_state.add_and_index(&root).unwrap();
        let locator = sqlite_cue_locator(indexed.source.id, Path::new("album.cue"), 1).unwrap();
        assert!(replacement_state.resolve_playback_locator_with_roots(&locator, &[]).is_ok());
        let original_inode = fs::metadata(&audio_path).unwrap().ino();
        fs::rename(&audio_path, root.join("old-image.wav")).unwrap();
        fs::write(&audio_path, synthetic_wav("Long", "Album", "Artist")).unwrap();
        assert_ne!(fs::metadata(&audio_path).unwrap().ino(), original_inode,
            "fixture must prove replacement by a distinct filesystem object");
        assert!(replacement_state.resolve_playback_locator_with_roots(&locator, &[]).is_err(),
            "a same-path replacement must not inherit the old CUE audio identity");
    }

    #[test]
    fn local_browser_replaces_a_single_image_track_with_its_cue_segments() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("cue-synthetic-library");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("image.wav"), synthetic_wav("Long image", "CUE Album", "CUE Artist")).unwrap();
        fs::write(root.join("cover.jpg"), [0xff, 0xd8, 0xff, 0x00]).unwrap();
        fs::write(root.join("album.cue"), b"TITLE \"CUE Album\"\nPERFORMER \"CUE Artist\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"Part One\"\nPERFORMER \"Singer One\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"Part Two\"\nPERFORMER \"Singer Two\"\nINDEX 01 00:00:02\n").unwrap();
        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let albums = state.local_browser_albums().unwrap();
        assert_eq!(albums.len(), 1, "physical image and virtual CUE must not be shown as duplicate albums");
        let album = &albums[0];
        assert!(album.id.starts_with("sqlite-cue-album:"));
        assert_eq!(album.title, "CUE Album");
        assert_eq!(album.track_count, 2);
        assert_eq!(album.artist, "CUE Artist");
        assert_eq!(album.artist_names, ["Singer One", "CUE Artist", "Singer Two"]);

        let discs = state.local_browser_album_tracks(&album.id).unwrap();
        assert_eq!(discs.len(), 1);
        assert_eq!(discs[0].tracks.len(), 2);
        assert_eq!(discs[0].tracks[0].title.as_deref(), Some("Part One"));
        assert_eq!(discs[0].tracks[0].artist.as_deref(), Some("Singer One"));
        assert!(discs[0].tracks[0].path.starts_with("sqlite-cue://"));
        assert_eq!(state.album_for_track(&discs[0].tracks[1].path).unwrap().as_deref(), Some(album.id.as_str()));
        let ab_tracks = state.album_ab_tracks(&album.id).unwrap();
        assert_eq!(ab_tracks.len(), 2);
        let admitted = state.open_album_ab_track(&album.id, &ab_tracks[1].path).unwrap();
        assert_eq!(admitted.cue.as_ref().map(|cue| cue.start_frame), Some(2));
        assert!(state.album_ab_track_is_current(&admitted));
        assert!(state.search_local_tracks("Part Two").unwrap().tracks.iter().any(|track| track.path == discs[0].tracks[1].path));
        assert_eq!(state.smart_playlist_albums().unwrap()[0].id, album.id);
        assert!(state.album_cover(&album.id).unwrap().unwrap().starts_with("data:image/jpeg;base64,"));
        assert!(root.join("image.wav").is_file(), "catalog browsing must not modify the audio object");
        let current_view = state.current_view(indexed.source.id).unwrap();
        assert!(state.catalog().unwrap().is_source_library_view_current(&current_view).unwrap());
    }

    #[test]
    fn local_browser_uses_complete_multifile_wavpack_cue_without_duplicate_tracks() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("WavPack CUE Café");
        fs::create_dir_all(&root).unwrap();
        let wavpack = synthetic_wavpack();
        fs::write(root.join("disc one.wv"), &wavpack).unwrap();
        fs::write(root.join("disc 二.wv"), &wavpack).unwrap();
        fs::write(
            root.join("album.cue"),
            "TITLE \"CUE Curated Release\"\nPERFORMER \"Fixture Album Artist\"\nFILE \"disc one.wv\" WAVE\nTRACK 01 AUDIO\nTITLE \"Part One\"\nPERFORMER \"Track One Artist\"\nINDEX 01 00:00:00\nFILE \"disc 二.wv\" WAVE\nTRACK 02 AUDIO\nTITLE \"Part Two\"\nPERFORMER \"Track Two Artist\"\nINDEX 01 00:00:30\n".as_bytes(),
        ).unwrap();

        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let albums = state.local_browser_albums().unwrap();
        assert_eq!(albums.len(), 1, "the physical files and virtual CUE must form one album");
        let album = &albums[0];
        assert!(album.id.starts_with("sqlite-cue-album:"));
        assert_eq!(album.track_count, 2);
        assert_eq!(album.title, "CUE Curated Release");

        let discs = state.local_browser_album_tracks(&album.id).unwrap();
        let tracks = discs.into_iter().flat_map(|disc| disc.tracks).collect::<Vec<_>>();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title.as_deref(), Some("Part One"));
        assert_eq!(tracks[1].title.as_deref(), Some("Part Two"));
        assert!(tracks.iter().all(|track| track.path.starts_with("sqlite-cue://")));

        let resolved = state.resolve_playback_locator_with_roots(&tracks[1].path, &[]).unwrap();
        let cue = SqliteLibraryState::parse_sqlite_cue_range_uri(&resolved).unwrap();
        assert_eq!(cue.path, root.canonicalize().unwrap().join("disc 二.wv").to_string_lossy());
        assert_eq!((cue.start_frame, cue.end_frame), (30, None));
        let current_view = state.current_view(indexed.source.id).unwrap();
        assert!(state
            .catalog()
            .unwrap()
            .is_source_library_view_current(&current_view)
            .unwrap());
    }

    #[test]
    fn local_browser_shows_twelve_track_two_file_wavpack_cue_as_one_album() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("Patricia Barber Cafe Blue [1STEP]");
        fs::create_dir_all(&root).unwrap();
        let wavpack = synthetic_wavpack();
        fs::write(root.join("Patricia Barber Cafe Blue [1LP].wv"), &wavpack).unwrap();
        fs::write(root.join("Patricia Barber Cafe Blue [2LP].wv"), &wavpack).unwrap();
        fs::write(root.join("folder.jpg"), [0xff, 0xd8, 0xff, 0x00]).unwrap();

        let mut cue = String::from(
            "REM GENRE Jazz\nREM DATE 2020\nREM BAZAR_VinylRiP\nREM LABEL Impex Records - IMP 6035-1\nPERFORMER \"Patricia Barber\"\nTITLE \"Cafe Blue [1STEP  45rpm IMXLPO6035-45]\"\nFILE \"Patricia Barber Cafe Blue [1LP].wv\" WAVE\n",
        );
        let first = ["00:00:00", "05:24:00", "12:20:00", "17:00:00", "21:33:00", "26:42:00"];
        let second = ["00:00:00", "05:23:00", "13:33:00", "18:06:00", "27:07:00", "28:07:00"];
        for (offset, timestamps) in [(0_u32, first), (6_u32, second)] {
            if offset == 6 {
                cue.push_str("FILE \"Patricia Barber Cafe Blue [2LP].wv\" WAVE\n");
            }
            for (index, timestamp) in timestamps.into_iter().enumerate() {
                let number = offset + index as u32 + 1;
                cue.push_str(&format!(
                    "TRACK {number:02} AUDIO\nTITLE \"Cafe Blue - {number:02}\"\nINDEX 01 {timestamp}\n"
                ));
            }
        }
        fs::write(root.join("album.cue"), cue).unwrap();

        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let albums = state.local_browser_albums().unwrap();
        assert_eq!(albums.len(), 1, "two physical WavPack rows and one CUE album must not be duplicated");
        let album = &albums[0];
        assert!(album.id.starts_with("sqlite-cue-album:"));
        assert_eq!(album.title, "Cafe Blue [1STEP  45rpm IMXLPO6035-45]");
        assert_eq!(album.artist, "Patricia Barber");
        assert_eq!(album.artist_names, ["Patricia Barber"]);
        assert_eq!(album.year.as_deref(), Some("2020"));
        assert_eq!(album.genres, ["Jazz"]);
        assert_eq!(album.track_count, 12);
        {
            let mut catalog = state.catalog().unwrap();
            let item = collection_local_capture(&catalog, &album.id).unwrap();
            assert_eq!(collection_local_resolve(&catalog, &item.scope, &item.reference), "available");
            let collection = catalog.collection_create("Cue").unwrap();
            let member = collection_local_add(&mut catalog, collection, &album.id).unwrap();
            let saved = catalog.collection_member(collection, member).unwrap();
            let evidence = catalog.collection_local_evidence(collection, member).unwrap();
            assert_eq!((evidence.files.len(), evidence.segments.len()), (2, 12));
            assert_eq!(collection_local_resolve_member(&mut catalog, collection, member, &saved.scope, &saved.reference), Ok(album.id.clone()));
        }
        assert_eq!(state.catalog().unwrap().track_count_for(indexed.source.id).unwrap(), 2);

        let tracks = state.local_browser_album_tracks(&album.id).unwrap().into_iter()
            .flat_map(|disc| disc.tracks).collect::<Vec<_>>();
        assert_eq!(tracks.len(), 12);
        assert_eq!(tracks[0].title.as_deref(), Some("Cafe Blue - 01"));
        assert_eq!(tracks[11].title.as_deref(), Some("Cafe Blue - 12"));
        assert!(tracks.iter().all(|track| track.path.starts_with("sqlite-cue://")));
        assert_eq!(state.album_for_track(&tracks[0].path).unwrap().as_deref(), Some(album.id.as_str()));
        assert_eq!(state.album_for_track(&tracks[11].path).unwrap().as_deref(), Some(album.id.as_str()));
        assert!(state.album_cover(&album.id).unwrap().unwrap().starts_with("data:image/jpeg;base64,"));

        let expected = [
            ("Patricia Barber Cafe Blue [1LP].wv", 0, Some(24_300)),
            ("Patricia Barber Cafe Blue [1LP].wv", 120_150, None),
            ("Patricia Barber Cafe Blue [2LP].wv", 0, Some(24_225)),
            ("Patricia Barber Cafe Blue [2LP].wv", 126_525, None),
        ];
        for (index, (file, start, end)) in [0_usize, 5, 6, 11].into_iter().zip(expected) {
            let resolved = state.resolve_playback_locator_with_roots(&tracks[index].path, &[]).unwrap();
            let cue = SqliteLibraryState::parse_sqlite_cue_range_uri(&resolved).unwrap();
            assert_eq!(cue.path, root.canonicalize().unwrap().join(file).to_string_lossy());
            assert_eq!((cue.start_frame, cue.end_frame), (start, end));
        }

        let before_reindex = state.current_view(indexed.source.id).unwrap();
        let generation = before_reindex.generation();
        state.add_and_index(&root).unwrap();
        let after_reindex = state.local_browser_albums().unwrap();
        assert_eq!(after_reindex.len(), 1);
        assert_eq!(after_reindex[0].id, album.id);
        assert_eq!(after_reindex[0].track_count, 12);
        assert_eq!(state.current_view(indexed.source.id).unwrap().generation(), generation);
    }

    #[test]
    fn rejected_known_cue_metadata_keeps_the_physical_album_visible() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("invalid-wavpack-cue");
        fs::create_dir_all(&root).unwrap();
        let wavpack = synthetic_wavpack();
        fs::write(root.join("disc one.wv"), &wavpack).unwrap();
        fs::write(root.join("disc two.wv"), &wavpack).unwrap();
        fs::write(
            root.join("album.cue"),
            b"REM GENRE\nTITLE \"Broken CUE Album\"\nFILE \"disc one.wv\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"disc two.wv\" WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        ).unwrap();
        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let albums = state.local_browser_albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert!(albums[0].id.starts_with("sqlite:"), "invalid CUE must not create a virtual album");
        let sheets = state.catalog().unwrap().cue_sheets_for_source(indexed.source.id).unwrap();
        assert!(matches!(sheets[0].status, sonante_sqlite_catalog::CueSheetStatus::Rejected { ref error_code }
            if error_code == "cue_rem_value_missing"));
        assert!(state.catalog().unwrap().cue_tracks_for_source(indexed.source.id).unwrap().is_empty());
    }

    #[test]
    fn sqlite_doctor_reports_cue_coverage_without_treating_virtual_locators_as_paths() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("cue-doctor-library");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("image.wav"), synthetic_wav("Physical", "CUE Album", "Artist")).unwrap();
        fs::write(
            root.join("album.cue"),
            b"TITLE \"CUE Album\"\nPERFORMER \"Artist\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"One\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"Two\"\nINDEX 01 00:00:01\n",
        ).unwrap();
        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().into_iter()
            .find(|album| album.id.starts_with("sqlite-cue-album:"))
            .expect("single-image CUE album should replace the physical long track");

        let report = state.album_doctor(&album.id).unwrap();
        assert_eq!(report.album_id, album.id);
        assert_eq!(report.inspection_source, library_doctor::InspectionSource::SqliteSnapshot);
        assert_eq!(report.coverage.cue_virtual_count, 2);
        assert!(report.coverage.partial);
        assert!(report.coverage.checks_not_performed.contains(&library_doctor::UnperformedCheck::CueMetadata));
        assert!(report.tracks.iter().all(|track| track.kind == tags::LocalAlbumTrackKind::CueVirtual
            && track.uri.starts_with("sqlite-cue://")));
        assert!(report.failures.is_empty());
    }

    #[test]
    fn cue_grouping_keeps_physical_precedence_for_multiple_sheets_like_063() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("cue-multi-sheet");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("image.wav"), synthetic_wav("Long image", "CUE Album", "CUE Artist")).unwrap();
        for (cue, title) in [("one.cue", "CUE Album"), ("two.cue", "Alternate Title")] {
            fs::write(
                root.join(cue),
                format!("TITLE \"{title}\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"Part One\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"Part Two\"\nINDEX 01 00:01:00\n"),
            ).unwrap();
        }
        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let albums = state.local_browser_albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert!(albums[0].id.starts_with("sqlite:"), "0.6.3 keeps a physical album unless a single CUE sheet replaces its one physical track");
        let cue_locator = sqlite_cue_locator(indexed.source.id, Path::new("one.cue"), 1).unwrap();
        assert_eq!(state.album_for_track(&cue_locator).unwrap().as_deref(), Some(albums[0].id.as_str()));
        let resolved = state.resolve_playback_locator_with_roots(&cue_locator, &[]).unwrap();
        let transport = SqliteLibraryState::parse_sqlite_cue_range_uri(&resolved).unwrap();
        assert_eq!(transport.path, root.canonicalize().unwrap().join("image.wav").to_string_lossy());
        assert_eq!((transport.start_frame, transport.end_frame), (0, Some(75)));
        let second = sqlite_cue_locator(indexed.source.id, Path::new("one.cue"), 2).unwrap();
        let final_track = SqliteLibraryState::parse_sqlite_cue_range_uri(
            &state.resolve_playback_locator_with_roots(&second, &[]).unwrap(),
        ).unwrap();
        assert_eq!((final_track.start_frame, final_track.end_frame), (75, None));
    }

    #[test]
    fn smart_playlist_catalog_uses_sqlite_grouping_and_exposes_first_catalog_confirmation() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("catalog/catalog.sqlite3")).unwrap();

        assert!(!state.has_indexed_sources().unwrap());
        state.add_and_index(&root).unwrap();
        let albums = state.smart_playlist_albums().unwrap();

        assert!(state.has_indexed_sources().unwrap());
        assert_eq!(albums.len(), 1);
        let album = &albums[0];
        assert!(album.id.starts_with("sqlite:"));
        assert_eq!(album.title, "Fixture Album");
        assert_eq!(album.artist, "Fixture Artist");
        assert_eq!(album.genres, Vec::<String>::new());
        assert_eq!(album.year, None);
        assert!(album.added_at.is_some());
        assert!(album.added_at.unwrap() > 0);
        assert!(album.favorite_key.starts_with("sqlite-album://"));

        // The album identity passed into the existing track-expansion service
        // is the same source-scoped identity used by the browser and queue.
        assert_eq!(state.local_browser_albums().unwrap()[0].id, album.id);
        let tracks = state.local_browser_album_tracks(&album.id).unwrap();
        let tracks = tracks
            .into_iter()
            .flat_map(|disc| disc.tracks)
            .collect::<Vec<_>>();
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|track| track.path.starts_with("sqlite-track://")));
    }

    #[test]
    fn added_timestamp_format_is_utc_and_unknown_or_out_of_range_stays_absent() {
        assert_eq!(format_added_at(0), Some("1970-01-01T00:00:00Z".into()));
        assert_eq!(format_added_at(1_798_761_600_000_000_000), Some("2027-01-01T00:00:00Z".into()));
        assert_eq!(format_added_at(-1), None);
    }

    fn synthetic_flac(comments: &[&str]) -> Vec<u8> {
        let mut bytes = b"fLaC".to_vec();
        let mut stream_info = Vec::with_capacity(34);
        stream_info.extend_from_slice(&4096_u16.to_be_bytes());
        stream_info.extend_from_slice(&4096_u16.to_be_bytes());
        stream_info.extend_from_slice(&[0; 6]);
        let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 44_100_u64;
        stream_info.extend_from_slice(&packed.to_be_bytes());
        stream_info.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&[0, 0, 0, stream_info.len() as u8]);
        bytes.extend(stream_info);
        let vendor = b"Sonante synthetic fixture";
        let mut block = Vec::new();
        block.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        block.extend_from_slice(vendor);
        block.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for comment in comments {
            block.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            block.extend_from_slice(comment.as_bytes());
        }
        bytes.push(0x84);
        let length = block.len() as u32;
        bytes.extend_from_slice(&length.to_be_bytes()[1..]);
        bytes.extend(block);
        bytes.extend_from_slice(b"\xff\xf8SONANTE_SYNTHETIC_AUDIO");
        bytes
    }

    fn sqlite_tag_request(
        inspection: &tags::LocalAlbumTagInspection,
        update: tags::LocalAlbumTagBatchUpdate,
    ) -> tags::LocalAlbumTagBatchPreflightRequest {
        tags::LocalAlbumTagBatchPreflightRequest {
            album_id: inspection.album_id.clone(),
            catalog_generation: inspection.catalog_generation,
            tracks: inspection.tracks.iter().map(|track| tags::LocalAlbumTagBatchExpectedTrack {
                uri: track.uri.clone(),
                kind: track.kind,
                revision: track.tags.as_ref().map(|tag| tag.revision.clone()),
            }).collect(),
            update,
        }
    }

    #[test]
    fn sqlite_flac_album_edit_updates_all_three_mixed_tracks_and_doctor() {
        use crate::tags::TagFieldUpdate;
        let temp = tempdir().unwrap();
        let root = temp.path().join("tag-edit-synthetic");
        fs::create_dir_all(&root).unwrap();
        for (index, genre) in ["Reggae", "Reggae", "Teste SONANTE"].iter().enumerate() {
            let comments = vec![
                format!("TITLE=Fixture Track {}", index + 1),
                "ARTIST=Fixture Artist".to_owned(),
                "ALBUM=Before Edit".to_owned(),
                "ALBUMARTIST=Fixture Album Artist".to_owned(),
                "DATE=2024".to_owned(),
                format!("TRACKNUMBER={}", index + 1),
                format!("GENRE={genre}"),
            ];
            fs::write(
                root.join(format!("{:02}-track.flac", index + 1)),
                synthetic_flac(&comments.iter().map(String::as_str).collect::<Vec<_>>()),
            )
            .unwrap();
        }
        let database = temp.path().join("catalog/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let old_album = state.local_browser_albums().unwrap().remove(0);
        let inspection = state.sqlite_tag_album_inspection(&old_album.id).unwrap();
        assert_eq!(inspection.catalog_generation, Some(indexed.source.generation));
        assert_eq!(inspection.tracks.len(), 3);
        assert!(inspection.tracks.iter().all(|track| track.tags.as_ref().unwrap().editable));
        assert_eq!(inspection.fields.genre.state, tags::LocalAlbumFieldState::Mixed);

        let request = sqlite_tag_request(&inspection, tags::LocalAlbumTagBatchUpdate {
            genre: TagFieldUpdate::Set("Reggae".into()),
            ..Default::default()
        });
        let plan = state.preflight_sqlite_tag_batch(request.clone()).unwrap();
        assert!(plan.applicable);
        assert_eq!(plan.tracks.len(), 3);
        assert_eq!(plan.change_count, 1); // The two already-Reggae tracks stay unchanged.
        let (result, reconciled, paths) = state.apply_sqlite_tag_batch(request).unwrap();
        assert!(reconciled);
        assert_eq!(result.committed_count, 1);
        assert_eq!(result.unchanged_count, 2);
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.not_attempted_count, 0);
        assert!(!result.partial_commit);
        assert!(paths.values().all(|path| Path::new(path).is_absolute()));

        // Reopen every FLAC independently with Lofty. This deliberately does
        // not trust the batch result or SQLite's cached metadata.
        for index in 1..=3 {
            let path = root.join(format!("{index:02}-track.flac"));
            let parsed = lofty::probe::Probe::open(&path).unwrap().read().unwrap();
            let genre = parsed.primary_tag().and_then(|tag| {
                use lofty::tag::ItemKey;
                tag.get_string(ItemKey::Genre).map(str::to_owned)
            });
            assert_eq!(genre.as_deref(), Some("Reggae"), "physical FLAC {index}");
        }

        let uniform_album = state.local_browser_albums().unwrap().remove(0);
        let uniform_report = state.album_doctor(&uniform_album.id).unwrap();
        assert_eq!(uniform_report.inspection_source, library_doctor::InspectionSource::SqliteSnapshot);
        assert!(!uniform_report.findings.iter().any(|finding| finding.code == library_doctor::FindingCode::GenreDivergence));

        let rename = sqlite_tag_request(&state.sqlite_tag_album_inspection(&uniform_album.id).unwrap(), tags::LocalAlbumTagBatchUpdate {
            album: TagFieldUpdate::Set("After Edit".into()),
            album_artist: TagFieldUpdate::Set("New Album Artist".into()),
            date: TagFieldUpdate::Set("2025-11".into()),
            ..Default::default()
        });
        let rename_plan = state.preflight_sqlite_tag_batch(rename.clone()).unwrap();
        assert!(rename_plan.applicable);
        assert_eq!(rename_plan.change_count, 3);
        let (renamed, reconciled, _) = state.apply_sqlite_tag_batch(rename).unwrap();
        assert!(reconciled);
        assert_eq!(renamed.committed_count, 3, "batch outcomes: {renamed:?}");
        assert_eq!(renamed.failed_count, 0);
        let new_album = state.local_browser_albums().unwrap().remove(0);
        assert_eq!(new_album.title, "After Edit");
        assert_ne!(new_album.id, old_album.id);
        let after = state.sqlite_tag_album_inspection(&new_album.id).unwrap();
        assert_eq!(after.tracks.len(), 3);
        for track in &after.tracks {
            let values = track.tags.as_ref().unwrap();
            assert_eq!(values.album.as_deref(), Some("After Edit"));
            assert_eq!(values.album_artist.as_deref(), Some("New Album Artist"));
            assert_eq!(values.date.as_deref(), Some("2025-11"));
        }
        for index in 1..=3 {
            let parsed = lofty::probe::Probe::open(root.join(format!("{index:02}-track.flac")))
                .unwrap().read().unwrap();
            let tag = parsed.primary_tag().unwrap();
            use lofty::tag::ItemKey;
            assert_eq!(tag.get_string(ItemKey::AlbumTitle), Some("After Edit"));
            assert_eq!(tag.get_string(ItemKey::AlbumArtist), Some("New Album Artist"));
            assert_eq!(tag.get_string(ItemKey::RecordingDate), Some("2025-11"));
        }

        let remove = sqlite_tag_request(&after, tags::LocalAlbumTagBatchUpdate {
            genre: TagFieldUpdate::Remove,
            ..Default::default()
        });
        let remove_plan = state.preflight_sqlite_tag_batch(remove.clone()).unwrap();
        assert!(remove_plan.applicable);
        assert_eq!(remove_plan.change_count, 3);
        let (removed, reconciled, _) = state.apply_sqlite_tag_batch(remove).unwrap();
        assert!(reconciled);
        assert_eq!(removed.committed_count, 3);
        assert_eq!(removed.failed_count, 0);
        for index in 1..=3 {
            let parsed = lofty::probe::Probe::open(root.join(format!("{index:02}-track.flac")))
                .unwrap().read().unwrap();
            let genre = parsed.primary_tag().and_then(|tag| {
                use lofty::tag::ItemKey;
                tag.get_string(ItemKey::Genre).map(str::to_owned)
            });
            assert_eq!(genre, None, "Genre must be physically removed from FLAC {index}");
        }

        let final_album = state.local_browser_albums().unwrap().remove(0);
        let report = state.album_doctor(&final_album.id).unwrap();
        assert!(!report.findings.iter().any(|finding| finding.code == library_doctor::FindingCode::GenreDivergence));
        assert!(report.tracks.iter().all(|track| track.tags.as_ref().and_then(|value| value.genre.as_ref()).is_none()));
        assert!(state.current_view(indexed.source.id).unwrap().generation() > indexed.source.generation);

        drop(state);
        let reopened = SqliteLibraryState::open(&database).unwrap();
        let reopened_album = reopened.local_browser_albums().unwrap().remove(0);
        assert_eq!(reopened_album.title, "After Edit");
        let persisted = reopened.sqlite_tag_album_inspection(&reopened_album.id).unwrap();
        assert_eq!(persisted.tracks.len(), 3);
        assert!(persisted.tracks.iter().all(|track| track.tags.as_ref().unwrap().genre.is_none()));
        assert!(root.join("01-track.flac").is_file());
    }

    #[test]
    fn sqlite_tag_editor_blocks_cue_folder_and_stale_generation() {
        use crate::tags::TagFieldUpdate;
        let temp = tempdir().unwrap();
        let root = temp.path().join("cue-fixture");
        fs::create_dir_all(&root).unwrap();
        let first = root.join("01-track.flac");
        let second = root.join("02-track.flac");
        let original_first = synthetic_flac(&["ALBUM=Fixture", "ARTIST=Artist", "GENRE=Jazz"]);
        let original_second = synthetic_flac(&["ALBUM=Fixture", "ARTIST=Artist", "GENRE=Rock"]);
        fs::write(&first, &original_first).unwrap();
        fs::write(&second, &original_second).unwrap();
        let database = temp.path().join("catalog/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);
        let inspection = state.sqlite_tag_album_inspection(&album.id).unwrap();
        assert_eq!(inspection.tracks.len(), 2);
        fs::write(root.join("sidecar.cue"), "FILE \"01-track.flac\" WAVE\n").unwrap();
        assert!(state.catalog().unwrap().source_track_directory_has_cue(
            indexed.source.id, indexed.source.generation, Path::new("01-track.flac")
        ).unwrap());
        let with_cue = state.sqlite_tag_album_inspection(&album.id).unwrap();
        assert_eq!(with_cue.tracks.len(), 2);
        assert!(with_cue.tracks.iter().all(|track|
            track.issue.as_ref().is_some_and(|issue| issue.code == tags::LocalTrackTagsErrorCode::CueVirtual)));
        let cue_request = sqlite_tag_request(&with_cue, tags::LocalAlbumTagBatchUpdate {
            album: TagFieldUpdate::Set("Blocked".into()), ..Default::default()
        });
        let cue_plan = state.preflight_sqlite_tag_batch(cue_request).unwrap();
        assert!(!cue_plan.applicable);
        assert_eq!(cue_plan.blocked_reason, Some(tags::LocalAlbumTagBatchBlockedReason::CueVirtual));
        assert_eq!(cue_plan.change_count, 0);
        assert_eq!(fs::read(&first).unwrap(), original_first);
        assert_eq!(fs::read(&second).unwrap(), original_second);

        fs::remove_file(root.join("sidecar.cue")).unwrap();
        state.reindex_with_progress(indexed.source.id, |_, _| {}).unwrap();
        let stale_request = sqlite_tag_request(&inspection, tags::LocalAlbumTagBatchUpdate {
            genre: TagFieldUpdate::Set("Changed".into()), ..Default::default()
        });
        assert_eq!(state.preflight_sqlite_tag_batch(stale_request).unwrap_err().code,
            tags::LocalAlbumTagBatchPreflightErrorCode::RevisionConflict);

        let current_album = state.local_browser_albums().unwrap().remove(0);
        let source_removed_request = sqlite_tag_request(
            &state.sqlite_tag_album_inspection(&current_album.id).unwrap(),
            tags::LocalAlbumTagBatchUpdate {
                genre: TagFieldUpdate::Set("Must Not Write".into()),
                ..Default::default()
            },
        );
        state.remove_source(indexed.source.id).unwrap();
        assert_eq!(
            state.preflight_sqlite_tag_batch(source_removed_request).unwrap_err().code,
            tags::LocalAlbumTagBatchPreflightErrorCode::AlbumCatalogFailure,
        );
        assert_eq!(fs::read(&first).unwrap(), original_first);
        assert_eq!(fs::read(&second).unwrap(), original_second);
    }

    #[test]
    fn pending_tag_edit_survives_restart_until_explicit_verify_reconciles_it() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("pending-edit-fixture");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("track.flac"), synthetic_flac(&["ALBUM=Fixture"])).unwrap();
        let database = temp.path().join("catalog/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        state.catalog().unwrap().begin_source_tag_edit(indexed.source.id, indexed.source.generation).unwrap();
        assert!(state.local_browser_albums().unwrap_err().contains("reconciliation"));
        assert!(state.catalog().unwrap().source_tag_edit_pending(indexed.source.id).unwrap());
        drop(state);

        let reopened = SqliteLibraryState::open(&database).unwrap();
        assert!(reopened.local_browser_albums().unwrap_err().contains("reconciliation"));
        assert!(reopened.verify(indexed.source.id).is_ok());
        assert!(!reopened.catalog().unwrap().source_tag_edit_pending(indexed.source.id).unwrap());
        assert_eq!(reopened.local_browser_albums().unwrap().len(), 1);
    }

    #[test]
    fn add_index_progress_is_operation_scoped_and_contains_no_source_path() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let mut events = Vec::new();
        let result = state
            .add_and_index_with_progress(&root, |event| events.push(event))
            .unwrap();
        assert_eq!(result.source.track_count, 2);
        assert!(events
            .iter()
            .any(|event| event.phase == IndexPhase::Scanning));
        assert!(events
            .iter()
            .any(|event| event.phase == IndexPhase::ReadingMetadata));
        let metadata = events
            .iter()
            .find(|event| event.phase == IndexPhase::ReadingMetadata)
            .unwrap();
        assert_eq!(metadata.identified_albums, 1);
        assert_eq!(metadata.identified_artists, 1);
        assert!(metadata.counts_provisional);
        assert_eq!(events.last().unwrap().phase, IndexPhase::Completed);
        assert_eq!(events.last().unwrap().audio_files_found, 2);
        assert_eq!(events.last().unwrap().identified_albums, 1);
        assert_eq!(events.last().unwrap().identified_artists, 1);
        assert!(!events.last().unwrap().counts_provisional);

        let payload = serde_json::to_string(&index_progress_dto(
            "synthetic-operation",
            "add",
            "Fixture",
            events.last().unwrap().clone(),
        ))
        .unwrap();
        assert!(payload.contains("synthetic-operation"));
        assert!(payload.contains("\"operation\":\"add\""));
        assert!(payload.contains("\"sourceName\":\"Fixture\""));
        assert!(payload.contains("\"phase\":\"completed\""));
        assert!(!payload.contains(root.to_string_lossy().as_ref()));
        assert!(!payload.contains("01-first.wav"));
    }

    #[test]
    fn sqlite_doctor_uses_current_catalog_snapshot_and_never_returns_the_root_path() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);

        let report = state.album_doctor(&album.id).unwrap();
        assert_eq!(report.inspection_source, library_doctor::InspectionSource::SqliteSnapshot);
        assert_eq!(report.coverage.physical_track_count, 2);
        assert_eq!(report.coverage.physical_tracks_read, 2);
        assert!(report.grouping_ambiguous);
        assert!(report.findings.iter().any(|finding| finding.code == library_doctor::FindingCode::AlbumArtistAbsent));
        assert!(report.findings.iter().any(|finding| finding.code == library_doctor::FindingCode::NumberIndeterminate));
        assert_eq!(report.artwork, Some(ArtworkStatus::Missing));
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains(root.to_string_lossy().as_ref()));
        assert!(!json.contains("sqlite-track://"));

        // A newly committed source generation invalidates the former in-memory
        // view; the next report is rebuilt from the latest SQLite snapshot.
        state.verify(indexed.source.id).unwrap();
        let refreshed = state.album_doctor(&album.id).unwrap();
        assert_eq!(refreshed.album_id, album.id);

        fs::write(root.join("01-first.wav"), b"not a WAV anymore").unwrap();
        state.verify(indexed.source.id).unwrap();
        let current_view = state.ensure_source_view(indexed.source.id).unwrap();
        let current_album = current_view.albums().first().unwrap().clone();
        let failed_report = state.album_doctor(&album_id_for_view(&current_album)).unwrap();
        assert!(failed_report.coverage.partial);
        assert_eq!(failed_report.failures.len(), 1);
        let failed_uri = failed_report.failures[0].uri.clone();
        assert!(!failed_report.findings.iter().any(|finding| {
            matches!(finding.code, library_doctor::FindingCode::TrackTitleAbsent | library_doctor::FindingCode::AlbumAbsent)
                && finding.evidence.iter().any(|evidence| evidence.uri == failed_uri)
        }), "a failed current extraction must not be reported as missing tags");
    }

    fn album_track_fixture(
        path: &str,
        artist: Option<&str>,
        album_artist: Option<&str>,
    ) -> AlbumTrack {
        AlbumTrack {
            track: sonante_sqlite_catalog::Track {
                source_id: 1,
                relative_path: PathBuf::from(path),
                size: 1,
                mtime_ns: 1,
                generation: 1,
                first_seen_ns: None,
            },
            metadata: Some(TrackMetadata {
                status: MetadataStatus::ParsedWithTags,
                file_format: "flac".into(),
                parser_version: 1,
                attempted_size: 1,
                attempted_mtime_ns: 1,
                error: None,
                last_success_size: Some(1),
                last_success_mtime_ns: Some(1),
                last_success: Some(TrackMetadataValues {
                    track_artists: artist.into_iter().map(str::to_owned).collect(),
                    album_artists: album_artist.into_iter().map(str::to_owned).collect(),
                    ..TrackMetadataValues::default()
                }),
                current_for_file: true,
            }),
            metadata_stale: false,
        }
    }

    #[test]
    fn normalized_album_year_preserves_only_unambiguous_valid_years() {
        assert_eq!(
            normalized_album_year(&["2024-03-01".into()]),
            Some("2024".into())
        );
        assert_eq!(
            normalized_album_year(&["2025-22-05".into()]),
            Some("2025".into())
        );
        assert_eq!(normalized_album_year(&["2024".into()]), Some("2024".into()));
        assert_eq!(
            normalized_album_year(&["2023-01-01".into(), "2024-01-01".into()]),
            None
        );
        assert_eq!(normalized_album_year(&["99-not-a-year".into()]), None);
        assert_eq!(normalized_album_year(&["2024unknown".into()]), None);
    }

    #[test]
    fn album_artist_display_matches_mature_consistency_and_dominance_rules() {
        let consistent = vec![
            album_track_fixture("CD1/01.flac", Some("Singer A"), Some("The Group")),
            album_track_fixture("CD2/01.flac", Some("Singer B"), Some("The Group")),
        ];
        let refs = consistent.iter().collect::<Vec<_>>();
        assert_eq!(local_album_display_artist(&refs), "The Group");

        let mut dominant = (0..10)
            .map(|index| {
                album_track_fixture(
                    &format!("track-{index}.flac"),
                    Some(if index < 8 { "Singer A" } else { "Guest" }),
                    None,
                )
            })
            .collect::<Vec<_>>();
        let refs = dominant.iter().collect::<Vec<_>>();
        assert_eq!(local_album_display_artist(&refs), "Singer A");
        dominant[7] = album_track_fixture("track-7.flac", Some("Singer B"), None);
        let refs = dominant.iter().collect::<Vec<_>>();
        assert_eq!(local_album_display_artist(&refs), "Various Artists");
    }

    #[test]
    fn album_dto_uses_explicit_disc_tag_then_conventional_folder_number() {
        let tracks = vec![
            album_track_fixture("Release/CD1/01.flac", Some("Singer"), None),
            album_track_fixture("Release/CD2/01.flac", Some("Singer"), None),
        ];
        let refs = tracks.iter().collect::<Vec<_>>();
        let album = Album {
            id: AlbumId {
                source_id: 1,
                identity: b"synthetic-album".to_vec(),
            },
            source_id: 1,
            title: "Release".into(),
            album_artists: Vec::new(),
            track_count: 2,
            first_seen_ns: None,
            stale_track_count: 0,
            date_values: Vec::new(),
            ambiguous: false,
            grouping_notes: Vec::new(),
            directory_hint: PathBuf::from("Release"),
        };
        let source = Source {
            id: 1,
            canonical_path: PathBuf::from("/synthetic"),
            display_name: "Synthetic".into(),
            generation: 1,
            last_checked_ns: 0,
        };
        let dto = local_browser_album_dto(&source, &album, &refs);
        assert_eq!(dto.discs.len(), 2);
        assert_eq!(dto.discs[0].number, 1);
        assert_eq!(dto.discs[1].number, 2);
    }

    #[test]
    fn album_dto_exposes_track_artists_from_persisted_metadata() {
        let mut tracks = vec![album_track_fixture(
            "Release/01.dsf",
            Some("Genesis"),
            Some("Genesis"),
        )];
        tracks[0]
            .metadata
            .as_mut()
            .unwrap()
            .last_success
            .as_mut()
            .unwrap()
            .track_artists
            .push("Guest Voice".into());
        let refs = tracks.iter().collect::<Vec<_>>();
        let album = Album {
            id: AlbumId {
                source_id: 1,
                identity: b"dsd-album".to_vec(),
            },
            source_id: 1,
            title: "Abacab".into(),
            album_artists: vec!["Genesis".into()],
            track_count: 1,
            first_seen_ns: None,
            stale_track_count: 0,
            date_values: vec!["2024-04-03".into()],
            ambiguous: false,
            grouping_notes: Vec::new(),
            directory_hint: PathBuf::from("Release"),
        };
        let source = Source {
            id: 1,
            canonical_path: PathBuf::from("/synthetic"),
            display_name: "Synthetic".into(),
            generation: 1,
            last_checked_ns: 0,
        };

        let dto = local_browser_album_dto(&source, &album, &refs);
        assert_eq!(dto.artist, "Genesis");
        assert_eq!(dto.artist_names, ["Genesis", "Guest Voice"]);
        assert_eq!(dto.year.as_deref(), Some("2024"));
    }

    #[test]
    fn explicit_index_builds_and_reuses_source_view_for_album_artist_and_track_browsing() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();

        let indexed = state.add_and_index(&root).unwrap();
        assert_eq!(indexed.metrics.files_examined, 2);
        assert_eq!(indexed.metrics.files_processed_by_parser, 2);
        assert_eq!(indexed.view.albums.len(), 1);
        assert_eq!(indexed.view.artists.len(), 1);
        let source_id = indexed.source.id;
        let album_identity = indexed.view.albums[0].identity.clone();

        let first = state.current_view(source_id).unwrap();
        let second = state.current_view(source_id).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "navigation should reuse the same view"
        );
        assert_eq!(first.albums()[0].track_count, 2);
        assert_eq!(first.albums_for_artist("fixture artist").count(), 1);
        let id = AlbumId {
            source_id,
            identity: decode_hex(&album_identity).unwrap(),
        };
        assert_eq!(first.tracks_for_album(&id).unwrap().len(), 2);
        assert_eq!(first.album_for_track(Path::new("01-first.wav")).unwrap().id, id);
        assert!(first.album_for_track(Path::new("missing.wav")).is_none());
        let ui_albums = state.local_browser_albums().unwrap();
        assert_eq!(ui_albums.len(), 1);
        assert_eq!(ui_albums[0].title, "Fixture Album");
        assert_eq!(ui_albums[0].artist_names, vec!["Fixture Artist"]);
        assert_eq!(ui_albums[0].grouping_notes, indexed.view.albums[0].grouping_notes);
        assert!(ui_albums[0].folder_path.starts_with("sqlite-album://"));
        let ui_tracks = state.local_browser_album_tracks(&ui_albums[0].id).unwrap();
        assert_eq!(ui_tracks.len(), 1);
        assert_eq!(ui_tracks[0].tracks.len(), 2);
        assert!(ui_tracks[0].tracks[0].path.starts_with("sqlite-track://"));
        assert!(!ui_tracks[0].tracks[0]
            .path
            .contains(root.to_string_lossy().as_ref()));

        let profile = state.local_edition_profile(&ui_albums[0].id).unwrap().unwrap();
        assert_eq!(profile.sizes.as_ref().unwrap().len(), 2);
        assert_eq!(profile.titles.as_ref().unwrap(), &["First", "Second"]);
        fs::write(root.join("01-first.wav"), synthetic_wav("Changed", "Fixture Album", "Fixture Artist")).unwrap();
        assert!(matches!(state.local_edition_profile(&ui_albums[0].id), Err(error) if error.contains("changed") || error.contains("unavailable")));
    }

    #[test]
    fn reindex_refreshes_only_one_source_and_keeps_the_committed_view_during_scan() {
        let temp = tempdir().unwrap();
        let root = fixture_root(&temp.path().join("first"));
        let other_root = fixture_root(&temp.path().join("other"));
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let other = state.add_and_index(&other_root).unwrap();
        let source_id = indexed.source.id;
        let old_view = state.current_view(source_id).unwrap();
        fs::write(root.join("01-first.wav"), synthetic_wav("First updated", "Fixture Album", "Fixture Artist")).unwrap();

        let mut events = Vec::new();
        let refreshed = state.reindex_with_progress(source_id, |_, event| {
            if event.phase == IndexPhase::Scanning {
                assert_eq!(state.current_view(source_id).unwrap().generation(), old_view.generation());
            }
            events.push(event);
        }).unwrap();
        assert!(refreshed.source.generation > indexed.source.generation);
        assert_eq!(refreshed.metrics.files_processed_by_parser, 2, "explicit reindex reparses the selected source only");
        assert_eq!(refreshed.metrics.sources_examined, 1);
        assert_eq!(state.current_view(other.source.id).unwrap().generation(), other.source.generation);
        assert!(events.iter().any(|event| event.phase == IndexPhase::Scanning));
        assert_eq!(events.last().unwrap().phase, IndexPhase::Completed);
        assert_eq!(state.current_view(source_id).unwrap().generation(), refreshed.source.generation);
        assert_ne!(state.current_view(source_id).unwrap().generation(), old_view.generation());
        assert!(root.join("01-first.wav").is_file(), "reindex never changes source files");
    }

    #[test]
    fn removing_source_only_removes_catalog_rows_and_invalidates_its_view() {
        let temp = tempdir().unwrap();
        let first_root = fixture_root(&temp.path().join("first"));
        let second_root = fixture_root(&temp.path().join("second"));
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let first = state.add_and_index(&first_root).unwrap();
        let second = state.add_and_index(&second_root).unwrap();
        let first_locator = state.search_local_tracks("fixture artist").unwrap().tracks.into_iter()
            .find(|track| track.path.starts_with(&format!("sqlite-track://{}/", first.source.id))).unwrap().path;

        let removed = state.remove_source(first.source.id).unwrap();
        assert_eq!(removed.removed_track_count, 2);
        assert_eq!(removed.display_name, "synthetic-library");
        assert!(first_root.join("01-first.wav").is_file(), "catalog removal must not delete music files");
        assert_eq!(state.list_sources().unwrap().iter().map(|source| source.id).collect::<Vec<_>>(), vec![second.source.id]);
        assert!(state.current_view(first.source.id).is_err(), "removed source view is no longer addressable");
        assert!(state.resolve_playback_locator(&first_locator).is_err(), "old queue locator is invalid after catalog removal");
        assert!(state.current_view(second.source.id).is_ok(), "other source remains usable");
    }

    #[test]
    fn adding_an_already_registered_folder_reuses_its_source_without_rescanning() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let first = state.add_and_index(&root).unwrap();
        let repeated = state.add_and_index(&root).unwrap();
        assert_eq!(repeated.source.id, first.source.id);
        assert_eq!(repeated.source.generation, first.source.generation);
        assert_eq!(repeated.source.track_count, first.source.track_count);
        assert_eq!(repeated.metrics.files_examined, 0);
        assert_eq!(repeated.view.albums.len(), first.view.albums.len());
        assert_eq!(state.list_sources().unwrap().len(), 1);
    }

    #[test]
    fn cancel_selected_folder_revokes_the_opaque_token() {
        let temp = tempdir().unwrap();
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let token = "wizard-selection".to_owned();
        state.inner.selected_roots.lock().unwrap().insert(token.clone(), fixture_root(temp.path()));
        assert!(revoke_folder_selection(&state.inner, &token).unwrap());
        assert!(!revoke_folder_selection(&state.inner, &token).unwrap());
    }

    #[test]
    fn selected_folder_index_is_persisted_and_reopened_without_rescanning() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let database = temp.path().join("db/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let token = "wizard-first-run-selection".to_owned();
        state.inner.selected_roots.lock().unwrap().insert(token.clone(), root.clone());
        let indexed = index_selected_folder(state.inner.clone(), token).unwrap();
        assert_eq!(indexed.metrics.files_examined, 2);
        assert_eq!(state.local_browser_albums().unwrap().len(), 1);
        drop(state);

        let reopened = SqliteLibraryState::open(&database).unwrap();
        let sources = reopened.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].track_count, 2);
        let view = reopened.open_view(sources[0].id).unwrap();
        assert_eq!(view.albums.len(), 1);
        assert_eq!(reopened.local_browser_albums().unwrap().len(), 1);
    }

    #[test]
    fn wizard_sqlite_first_run_rejects_premature_save_and_reuses_indexed_source_on_retry() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let database = temp.path().join("db/catalog.sqlite3");
        let config_path = temp.path().join("config/config.json");
        let state = SqliteLibraryState::open(&database).unwrap();
        let current = AppConfig::default();
        current.save_to_path(&config_path).unwrap();
        // A private null-output MPD already creates this file on first run;
        // it is unrelated to the SQLite catalog and must be preserved.
        let mpd_database = config_path.parent().unwrap().join("mpd-v2.db");
        fs::write(&mpd_database, b"synthetic private MPD database marker").unwrap();
        let mut next = current.clone();
        next.first_run = false;
        next.sources = Some(EnabledSources { local: true, plex: false, navidrome: false });
        next.local_folders = vec![root.to_string_lossy().into_owned()];

        let save = || next.save_to_path(&config_path).map_err(|error| error.to_string());
        assert_eq!(crate::confirm_sqlite_initial_preferences(&current, &next, &state, save),
            Err("SQLITE_WIZARD_SOURCE_NOT_INDEXED".into()));
        assert!(AppConfig::load_from_path(&config_path).unwrap().first_run);

        let token = "wizard-first-selection".to_owned();
        state.inner.selected_roots.lock().unwrap().insert(token.clone(), root.clone());
        let indexed = index_selected_folder(state.inner.clone(), token).unwrap();
        assert_eq!(indexed.metrics.files_examined, 2);
        assert_eq!(indexed.view.albums.len(), 1);
        assert_eq!(indexed.source.generation, 1);
        assert_eq!(state.list_sources().unwrap().len(), 1);

        let failed = crate::confirm_sqlite_initial_preferences(&current, &next, &state, || {
            Err::<(), _>("synthetic config write failure".into())
        });
        assert_eq!(failed, Err("synthetic config write failure".into()));
        assert!(AppConfig::load_from_path(&config_path).unwrap().first_run);
        assert_eq!(state.list_sources().unwrap().len(), 1);

        let retry_token = "wizard-retry-selection".to_owned();
        state.inner.selected_roots.lock().unwrap().insert(retry_token.clone(), root.clone());
        let retried = index_selected_folder(state.inner.clone(), retry_token).unwrap();
        assert_eq!(retried.source.id, indexed.source.id);
        assert_eq!(retried.source.generation, indexed.source.generation);
        assert_eq!(retried.metrics.files_examined, 0);
        assert_eq!(state.list_sources().unwrap().len(), 1);

        crate::confirm_sqlite_initial_preferences(&current, &next, &state, || {
            next.save_to_path(&config_path).map_err(|error| error.to_string())
        }).unwrap();
        drop(state);
        let restored = AppConfig::load_from_path(&config_path).unwrap();
        assert!(!restored.first_run);
        assert_eq!(restored.local_folders, next.local_folders);
        let reopened = SqliteLibraryState::open(&database).unwrap();
        let sources = reopened.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].track_count, 2);
        assert_eq!(reopened.open_view(sources[0].id).unwrap().albums.len(), 1);
        assert_eq!(fs::read(&mpd_database).unwrap(), b"synthetic private MPD database marker");
    }

    #[test]
    fn wizard_sqlite_first_run_rejects_unavailable_or_removed_sources() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let current = AppConfig::default();
        let mut next = current.clone();
        next.first_run = false;
        next.local_folders = vec![root.to_string_lossy().into_owned()];
        state.remove_source(indexed.source.id).unwrap();
        assert_eq!(crate::confirm_sqlite_initial_preferences(&current, &next, &state, || -> Result<(), String> {
            panic!("removed source must not publish preferences")
        }), Err("SQLITE_WIZARD_SOURCE_NOT_INDEXED".into()));
        next.local_folders = vec![temp.path().join("unavailable").to_string_lossy().into_owned()];
        assert_eq!(crate::confirm_sqlite_initial_preferences(&current, &next, &state, || -> Result<(), String> {
            panic!("unavailable source must not publish preferences")
        }), Err("SQLITE_WIZARD_SOURCE_UNAVAILABLE".into()));

        let mut remote_only = current.clone();
        remote_only.first_run = false;
        remote_only.sources = Some(EnabledSources { local: false, plex: true, navidrome: false });
        assert_eq!(crate::confirm_sqlite_initial_preferences(&current, &remote_only, &state, || Ok(42)), Ok(42));
        remote_only.local_folders.push(root.to_string_lossy().into_owned());
        assert_eq!(crate::confirm_sqlite_initial_preferences(&current, &remote_only, &state, || -> Result<(), String> {
            panic!("a disabled Local source must not be published")
        }), Err("SQLITE_WIZARD_SOURCE_DISABLED".into()));
    }

    #[test]
    fn selected_folder_command_preserves_existing_catalog_on_failure_and_allows_retry() {
        let temp = tempdir().unwrap();
        let first_root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let first = state.add_and_index(&first_root).unwrap();
        let previous_view = state.current_view(first.source.id).unwrap();

        let failed_token = "failed-selection".to_owned();
        state.inner.selected_roots.lock().unwrap().insert(
            failed_token.clone(),
            temp.path().join("temporarily-unavailable"),
        );
        assert!(index_selected_folder(state.inner.clone(), failed_token.clone()).is_err());
        assert!(!state
            .inner
            .selected_roots
            .lock()
            .unwrap()
            .contains_key(&failed_token));
        assert_eq!(state.list_sources().unwrap().len(), 1);
        assert!(Arc::ptr_eq(
            &state.current_view(first.source.id).unwrap(),
            &previous_view
        ));

        let retry_root = fixture_root(temp.path().join("retry-parent").as_path());
        let retry_token = "retry-selection".to_owned();
        state
            .inner
            .selected_roots
            .lock()
            .unwrap()
            .insert(retry_token.clone(), retry_root);
        let retried = index_selected_folder(state.inner.clone(), retry_token.clone()).unwrap();
        assert!(!state
            .inner
            .selected_roots
            .lock()
            .unwrap()
            .contains_key(&retry_token));
        assert_eq!(retried.metrics.files_examined, 2);
        assert_eq!(state.list_sources().unwrap().len(), 2);
        assert_eq!(previous_view.albums().len(), 1);
    }

    #[test]
    fn adding_a_source_does_not_block_navigation_on_the_existing_catalog_connection() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let database = temp.path().join("db/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let initial = state.add_and_index(&root).unwrap();
        let original_source = initial.source.id;
        let original_view = state.current_view(original_source).unwrap();
        assert!(state
            .add_and_index(&temp.path().join("missing-root"))
            .is_err());
        assert_eq!(state.list_sources().unwrap().len(), 1);
        assert!(Arc::ptr_eq(
            &state.current_view(original_source).unwrap(),
            &original_view
        ));
        let second = temp.path().join("second-library");
        fs::create_dir_all(&second).unwrap();
        fs::write(
            second.join("01-new.wav"),
            synthetic_wav("New", "Second Album", "Second Artist"),
        )
        .unwrap();
        // A malformed audio file is still a physical catalog entry; its
        // per-file metadata failure must not cancel sibling/source indexing.
        fs::write(second.join("02-invalid.wav"), b"malformed synthetic WAV").unwrap();

        // Holding the UI's original read connection simulates a navigation
        // operation. Indexing must complete on its own SQLite connection and
        // leave the previous committed view usable until commit.
        let held_connection = state.inner.catalog.lock().unwrap();
        let worker_state = state.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = worker_state.add_and_index(&second);
            let _ = done_tx.send(result);
        });
        let completed_while_ui_connection_was_held = done_rx.recv_timeout(Duration::from_secs(10));
        let completed_without_ui_connection = completed_while_ui_connection_was_held.is_ok();
        drop(held_connection);
        let indexed = match completed_while_ui_connection_was_held {
            Ok(result) => result.unwrap(),
            Err(_) => done_rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap(),
        };
        worker.join().unwrap();

        assert!(
            completed_without_ui_connection,
            "indexing waited behind the catalog connection used for navigation"
        );
        assert_eq!(indexed.metrics.files_examined, 2);
        assert_eq!(indexed.metrics.extraction_failures, 1);
        assert_eq!(indexed.source.track_count, 2);
        assert_eq!(original_view.albums().len(), 1);
        assert!(Arc::ptr_eq(
            &state.current_view(original_source).unwrap(),
            &original_view
        ));
        let sources = state.list_sources().unwrap();
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources
                .iter()
                .map(|source| source.track_count)
                .sum::<usize>(),
            4
        );
    }

    #[test]
    fn local_search_reads_tracks_from_the_cached_sqlite_view() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();

        let result = state.search_local_tracks("fixture artist").unwrap();
        assert_eq!(result.tracks.len(), 2);
        assert!(!result.limited);
        assert!(result
            .tracks
            .iter()
            .all(|track| track.path.starts_with("sqlite-track://")));
        assert!(result
            .tracks
            .iter()
            .all(|track| !track.path.contains(root.to_string_lossy().as_ref())));
        assert!(Arc::ptr_eq(
            &state.current_view(indexed.source.id).unwrap(),
            &state.current_view(indexed.source.id).unwrap(),
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn playback_locator_resolves_only_a_current_indexed_regular_file() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);
        let track = state
            .local_browser_album_tracks(&album.id)
            .unwrap()
            .remove(0)
            .tracks
            .remove(0);
        let resolved = state.resolve_playback_locator(&track.path).unwrap();
        assert_eq!(PathBuf::from(&resolved), root.canonicalize().unwrap().join("01-first.wav"));

        let traversal = format!(
            "sqlite-track://{}/{}",
            indexed.source.id,
            URL_SAFE_NO_PAD.encode(b"../outside.wav")
        );
        assert_eq!(
            state.resolve_playback_locator(&traversal).unwrap_err(),
            "SQLITE_TRACK_LOCATOR_INVALID"
        );

        fs::write(root.join("01-first.wav"), b"changed synthetic content").unwrap();
        assert_eq!(
            state.resolve_playback_locator(&track.path).unwrap_err(),
            "SQLITE_TRACK_UNAVAILABLE_OR_CHANGED"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn playback_locator_rehydrates_view_after_reopen_without_scanning() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let database = temp.path().join("db/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let item = state.search_local_tracks("fixture artist").unwrap().tracks.remove(0);
        let locator = item.path;
        drop(state);

        let reopened = SqliteLibraryState::open(&database).unwrap();
        let resolved = reopened.resolve_playback_locator(&locator).unwrap();
        assert_eq!(PathBuf::from(&resolved), root.canonicalize().unwrap().join("01-first.wav"));
        assert!(reopened.current_view(indexed.source.id).is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tauri_playback_resolver_revalidates_sqlite_identity_before_returning_absolute_path() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        state.add_and_index(&root).unwrap();
        let item = state.search_local_tracks("fixture artist").unwrap().tracks.remove(0);
        let locator = item.path.clone();
        let facts = state.player_source_facts(&locator).unwrap().unwrap();
        assert_eq!(facts.provenance, "local");
        assert_eq!(facts.codec, "PCM");
        assert_eq!(facts.sample_rate, Some(44_100));
        assert_eq!(facts.bit_depth, None, "the catalog does not persist bit depth");
        let track = audio::TrackMetadata {
            title: item.title.unwrap_or_else(|| item.name.clone()),
            artist: item.artist.unwrap_or_default(),
            album: item.album.unwrap_or_default(),
            thumb: None,
            plex_image: None,
            media_locator: Some(audio::MediaLocator::Local { uri: item.path }),
            uri: String::new(), duration: item.duration,
        };
        let plex = crate::plex::PlexClient::from_config(&crate::config::AppConfig::default());
        let uris = tauri::async_runtime::block_on(crate::resolve_playback_uris(&[track], &plex, Some(&state), &[])).unwrap();
        assert_eq!(uris.len(), 1);
        assert_eq!(PathBuf::from(&uris[0]), root.canonicalize().unwrap().join("01-first.wav"));

        let invalid = audio::TrackMetadata {
            title: "forged".into(), artist: String::new(), album: String::new(), thumb: None,
            plex_image: None,
            media_locator: Some(audio::MediaLocator::Local { uri: format!("sqlite-track://1/{}", URL_SAFE_NO_PAD.encode(b"../escape.wav")) }),
            uri: String::new(), duration: None,
        };
        assert_eq!(
            tauri::async_runtime::block_on(crate::resolve_playback_uris(&[invalid], &plex, Some(&state), &[])).unwrap_err(),
            "SQLITE_TRACK_LOCATOR_INVALID"
        );
    }

    #[test]
    fn sqlite_playback_locator_validator_accepts_only_canonical_opaque_references() {
        let valid = format!("sqlite-track://7/{}", URL_SAFE_NO_PAD.encode(b"Album/track.wav"));
        assert!(SqliteLibraryState::is_sqlite_track_locator(&valid));
        for invalid in [
            "sqlite-track://0/YQ",
            "sqlite-track://7/",
            "sqlite-track://7/../YQ",
            "sqlite-track://7/YQ/extra",
            "sqlite-track://7/YQ?x",
            "sqlite-track://7/YR",
        ] {
            assert!(!SqliteLibraryState::is_sqlite_track_locator(invalid), "{invalid}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sqlite_ab_uses_exact_album_tracks_and_retained_verified_descriptors() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);
        let tracks = state.album_ab_tracks(&album.id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|track| track.path.starts_with("sqlite-track://")));

        let admitted = state.open_album_ab_track(&album.id, &tracks[0].path).unwrap();
        assert!(state.album_ab_track_is_current(&admitted));
        assert!(admitted.file.identity_is_current());

        // An unrelated title or a locator from another album cannot select a
        // path by textual similarity.
        assert!(matches!(state.open_album_ab_track(&album.id, "sqlite-track://999/YQ"), Err(error) if error == "AB_CHANGED"));

        fs::write(root.join("01-first.wav"), b"changed fixture contents").unwrap();
        assert!(!state.album_ab_track_is_current(&admitted));
    }

    #[test]
    fn local_album_artwork_uses_a_bounded_file_under_the_indexed_album_directory() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let cover = b"\xff\xd8\xffsynthetic-jpeg-bytes";
        fs::write(root.join("cover.jpg"), cover).unwrap();
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);

        let identity = decode_hex(&indexed.view.albums[0].identity).unwrap();
        assert_eq!(
            album.id,
            format!(
                "sqlite:{}:{}",
                indexed.source.id,
                URL_SAFE_NO_PAD.encode(identity)
            )
        );
        assert_eq!(
            state.album_cover(&album.id).unwrap(),
            Some(format!("data:image/jpeg;base64,{}", STANDARD.encode(cover)))
        );
    }

    #[test]
    fn sqlite_playback_locator_resolves_its_exact_catalog_album_only_while_track_is_current() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);
        let relative = b"01-first.wav";
        let locator = format!("sqlite-track://{}/{}", indexed.source.id, URL_SAFE_NO_PAD.encode(relative));

        assert_eq!(state.album_for_track(&locator).unwrap().as_deref(), Some(album.id.as_str()));
        let malformed = format!("sqlite-track://{}/{}", indexed.source.id, URL_SAFE_NO_PAD.encode(b"../01-first.wav"));
        assert_eq!(state.album_for_track(&malformed).unwrap_err(), "SQLITE_TRACK_LOCATOR_INVALID");

        fs::remove_file(root.join("01-first.wav")).unwrap();
        assert!(state.album_for_track(&locator).is_err(), "removed or changed media cannot authorize a cover lookup");
        assert!(state.player_source_facts(&locator).is_err(), "removed media cannot supply current Now Playing facts");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn local_album_artwork_rejects_a_symlink_cover_without_reading_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let outside = temp.path().join("outside.jpg");
        fs::write(&outside, b"\xff\xd8\xffoutside").unwrap();
        symlink(&outside, root.join("cover.jpg")).unwrap();
        let state = SqliteLibraryState::open(temp.path().join("db/catalog.sqlite3")).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let album = state.local_browser_albums().unwrap().remove(0);

        let error = state.album_cover(&album.id).unwrap_err();
        assert!(error.contains("symlink"));
        assert!(state.current_view(indexed.source.id).is_ok());
    }

    #[test]
    fn reopening_catalog_restores_sources_and_view_without_automatic_scan() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let database = temp.path().join("db/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let source_id = indexed.source.id;
        let generation = indexed.source.generation;
        drop(state);

        // The files are removed before reopening. A startup/open-view scan would
        // erase them; opening the SQLite snapshot must leave them browsable.
        fs::remove_file(root.join("01-first.wav")).unwrap();
        fs::remove_file(root.join("02-second.wav")).unwrap();
        let reopened = SqliteLibraryState::open(&database).unwrap();
        let sources = reopened.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].generation, generation);
        assert_eq!(sources[0].track_count, 2);
        let view = reopened.open_view(source_id).unwrap();
        assert_eq!(view.generation, generation);
        assert_eq!(view.albums.len(), 1);
        assert_eq!(view.albums[0].track_count, 2);
        assert_eq!(reopened.local_browser_albums().unwrap().len(), 1);

        let report = reopened.verify(source_id).unwrap();
        assert_eq!(report.metrics.files_removed, 2);
        assert_eq!(report.view.albums.len(), 0);
        assert!(report.source.generation > generation);
    }

    #[test]
    fn a_generation_change_from_an_independent_connection_marks_cached_view_stale() {
        let temp = tempdir().unwrap();
        let root = fixture_root(temp.path());
        let database = temp.path().join("db/catalog.sqlite3");
        let state = SqliteLibraryState::open(&database).unwrap();
        let indexed = state.add_and_index(&root).unwrap();
        let source_id = indexed.source.id;

        fs::write(
            root.join("03-third.wav"),
            synthetic_wav("Third", "Fixture Album", "Fixture Artist"),
        )
        .unwrap();
        let mut other_connection = Catalog::open(&database).unwrap();
        other_connection.verify_source(source_id).unwrap();
        assert!(
            matches!(state.current_view(source_id), Err(error) if error.starts_with("VIEW_STALE:"))
        );

        let refreshed = state.open_view(source_id).unwrap();
        assert_eq!(refreshed.albums[0].track_count, 3);
    }

    #[test]
    fn production_build_uses_public_identity_and_development_has_its_own_overlay() {
        let config = include_str!("../tauri.conf.json");
        assert!(config.contains("\"identifier\": \"com.sonante.player\""));
        assert!(!config.contains("sqlite-experimental"));
        let development = include_str!("../tauri.dev.conf.json");
        assert!(development.contains("com.sonante.player.dev064"));
        let html = include_str!("../../index.html");
        assert!(html.contains("/src/main.tsx"));
        assert!(!html.contains("sqlite-library-main.tsx"));
    }
}

// Collection references are opaque snapshots; these commands do not resolve albums.
#[derive(Serialize)]
pub struct PersonalCollectionDto {
    id: i64,
    name: String,
    created_ns: i64,
    updated_ns: i64,
}
#[derive(Serialize)]
pub struct PersonalCollectionMemberDto {
    id: i64,
    collection_id: i64,
    source: String,
    scope: String,
    reference: String,
    title: String,
    artist: Option<String>,
    position: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalCollectionMemberInput {
    source: String,
    scope: String,
    reference: String,
    title: String,
    artist: Option<String>,
}

#[tauri::command]
pub async fn personal_collection_create(
    state: State<'_, SqliteLibraryState>,
    name: String,
) -> Result<i64, String> {
    let path = state.inner.database_path.clone();
    blocking(move || Catalog::open(path)?.collection_create(&name)).await
}
#[tauri::command]
pub async fn personal_collection_rename(
    state: State<'_, SqliteLibraryState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let path = state.inner.database_path.clone();
    blocking(move || Catalog::open(path)?.collection_rename(id, &name)).await
}
#[tauri::command]
pub async fn personal_collection_delete(
    state: State<'_, SqliteLibraryState>,
    id: i64,
) -> Result<(), String> {
    let path = state.inner.database_path.clone();
    blocking(move || Catalog::open(path)?.collection_delete(id)).await
}
#[tauri::command]
pub async fn personal_collections_list(
    state: State<'_, SqliteLibraryState>,
) -> Result<Vec<PersonalCollectionDto>, String> {
    let path = state.inner.database_path.clone();
    blocking(move || {
        Ok(Catalog::open(path)?
            .collections()?
            .into_iter()
            .map(|c| PersonalCollectionDto {
                id: c.id,
                name: c.name,
                created_ns: c.created_ns,
                updated_ns: c.updated_ns,
            })
            .collect())
    })
    .await
}
#[tauri::command]
pub async fn personal_collection_add(
    state: State<'_, SqliteLibraryState>,
    id: i64,
    member: PersonalCollectionMemberInput,
) -> Result<i64, String> {
    let path = state.inner.database_path.clone();
    blocking(move || {
        Catalog::open(path)?.collection_add(
            id,
            sonante_sqlite_catalog::collections::NewMember {
                source: member.source,
                scope: member.scope,
                reference: member.reference,
                title: member.title,
                artist: member.artist,
            },
        )
    })
    .await
}
#[tauri::command]
pub async fn personal_collection_remove(
    state: State<'_, SqliteLibraryState>,
    id: i64,
    member_id: i64,
) -> Result<(), String> {
    let path = state.inner.database_path.clone();
    blocking(move || Catalog::open(path)?.collection_remove(id, member_id)).await
}
#[tauri::command]
pub async fn personal_collection_members(
    state: State<'_, SqliteLibraryState>,
    id: i64,
) -> Result<Vec<PersonalCollectionMemberDto>, String> {
    let path = state.inner.database_path.clone();
    blocking(move || {
        Ok(Catalog::open(path)?
            .collection_members(id)?
            .into_iter()
            .map(|m| PersonalCollectionMemberDto {
                id: m.id,
                collection_id: m.collection_id,
                source: m.source,
                scope: m.scope,
                reference: m.reference,
                title: m.title,
                artist: m.artist,
                position: m.position,
            })
            .collect())
    })
    .await
}

#[tauri::command]
pub async fn personal_collection_add_subsonic(
    state: State<'_, SqliteLibraryState>,
    id: i64,
    connection_id: String,
    account_id: String,
    album_id: String,
) -> Result<i64, String> {
    let member =
        crate::navidrome::collection_capture(&connection_id, &account_id, &album_id).await?;
    let path = state.inner.database_path.clone();
    blocking(move || Catalog::open(path)?.collection_add(id, member)).await
}

#[derive(Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
enum PersonalCollectionTargetDto {
    Local {
        album_id: String,
    },
    Plex {
        server_id: String,
        library_id: String,
        rating_key: String,
    },
    Subsonic {
        account_id: String,
        album_id: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
enum PersonalCollectionArtworkDto {
    Plex { image: crate::plex::PlexImageRef, session: String },
    Subsonic { connection_id: String, cover_id: String },
}

#[derive(Serialize)]
pub struct PersonalCollectionResolutionDto {
    status: String,
    target: Option<PersonalCollectionTargetDto>,
    artwork: Option<PersonalCollectionArtworkDto>,
}

#[cfg(test)]
mod personal_collection_resolution_tests {
    use super::*;

    #[test]
    fn resolution_exposes_only_verified_source_navigation_ids() {
        let value = serde_json::to_value(PersonalCollectionResolutionDto {
            status: "available".into(),
            artwork: None,
            target: Some(PersonalCollectionTargetDto::Plex {
                server_id: "server-fixture".into(),
                library_id: "7".into(),
                rating_key: "42".into(),
            }),
        })
        .unwrap();
        assert_eq!(value["status"], "available");
        assert_eq!(value["target"]["source"], "plex");
        assert_eq!(value["target"]["server_id"], "server-fixture");
        assert!(value.get("scope").is_none());
        assert!(value.get("reference").is_none());
    }
}

#[tauri::command]
pub async fn personal_collection_resolve(
    state: State<'_, SqliteLibraryState>,
    id: i64,
    member_id: i64,
    plex: State<'_, crate::PlexState>,
) -> Result<PersonalCollectionResolutionDto, String> {
    let path = state.inner.database_path.clone();
    let lookup_path = path.clone();
    let member = blocking(move || Catalog::open(lookup_path)?.collection_member(id, member_id)).await?;
    if member.source == "local" {
        return blocking(move || {
            let mut catalog = Catalog::open(path)?;
            let resolved = collection_local_resolve_member(&mut catalog, id, member_id, &member.scope, &member.reference);
            let result = match resolved {
                Ok(album_id) => PersonalCollectionResolutionDto {
                    status: "available".into(),
                    artwork: None,
                    target: Some(PersonalCollectionTargetDto::Local { album_id }),
                },
                Err(status) => PersonalCollectionResolutionDto {
                    status: status.into(),
                    artwork: None,
                    target: None,
                },
            };
            Ok(result)
        })
        .await;
    }
    let (status, target, artwork) = match member.source.as_str() {
        "subsonic" => {
            match crate::navidrome::collection_resolve_target(&member.scope, &member.reference).await {
                Ok(album) => (
                    "available",
                    Some(PersonalCollectionTargetDto::Subsonic {
                        account_id: album.account_id,
                        album_id: album.album_id,
                    }),
                    album.cover_id.map(|cover_id| PersonalCollectionArtworkDto::Subsonic { connection_id: album.connection_id, cover_id }),
                ),
                Err(status) => (status, None, None),
            }
        }
        "plex" => {
            let parts = member
                .scope
                .strip_prefix("v1:")
                .and_then(|s| s.split_once(':'))
                .zip(member.reference.strip_prefix("v1:"));
            if let Some(((server, library), rating)) = parts {
                let client = plex
                    .0
                    .lock()
                    .map_err(|_| "COLLECTION_SOURCE_UNAVAILABLE")?
                    .clone();
                match client.collection_album_with_artwork(server, library, rating).await {
                    Ok((_, image, session)) => (
                        "available",
                        Some(PersonalCollectionTargetDto::Plex {
                            server_id: server.to_owned(),
                            library_id: library.to_owned(),
                            rating_key: rating.to_owned(),
                        }),
                        image.map(|image| PersonalCollectionArtworkDto::Plex { image, session }),
                    ),
                    Err(status) => (status, None, None),
                }
            } else {
                ("unresolved", None, None)
            }
        }
        _ => ("unresolved", None, None),
    };
    Ok(PersonalCollectionResolutionDto { status: status.into(), target, artwork })
}

#[tauri::command]
pub async fn personal_collection_add_plex(
    state: State<'_, SqliteLibraryState>,
    plex: State<'_, crate::PlexState>,
    id: i64,
    server_id: String,
    library_id: String,
    rating_key: String,
) -> Result<i64, String> {
    let client = plex
        .0
        .lock()
        .map_err(|_| "COLLECTION_SOURCE_UNAVAILABLE")?
        .clone();
    let member = client
        .collection_album(&server_id, &library_id, &rating_key)
        .await
        .map_err(|s| format!("COLLECTION_{s}"))?;
    let path = state.inner.database_path.clone();
    blocking(move || Catalog::open(path)?.collection_add(id, member)).await
}

// Hash the first and last 16 KiB of each selected file, plus its length.
// This bounded evidence cannot detect all changes outside those samples.
// Group membership is taken from the last committed source index.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CollectionLocalFile {
    #[serde(rename = "p")]
    path: String,
    #[serde(rename = "s")]
    size: i64,
    #[serde(rename = "m")]
    mtime_ns: i64,
    #[serde(rename = "h")]
    sample_sha256: String,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CollectionCueSegment {
    #[serde(rename = "c")]
    cue: String,
    #[serde(rename = "a")]
    audio: String,
    #[serde(rename = "t")]
    track: u32,
    #[serde(rename = "s")]
    start: u64,
    #[serde(rename = "e")]
    end: Option<u64>,
    #[serde(rename = "z")]
    cue_size: i64,
    #[serde(rename = "m")]
    cue_mtime_ns: i64,
    #[serde(rename = "d")]
    cue_device: u64,
    #[serde(rename = "i")]
    cue_inode: u64,
    #[serde(rename = "n")]
    cue_ctime_ns: i64,
}
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CollectionLocalSnapshot {
    version: u8,
    album_locator: String,
    files: Vec<CollectionLocalFile>,
    segments: Vec<CollectionCueSegment>,
}
fn collection_local_path(path: &Path) -> String {
    URL_SAFE_NO_PAD.encode(path.as_os_str().as_encoded_bytes())
}

fn collection_local_snapshot(
    catalog: &Catalog,
    album_locator: &str,
) -> Result<(CollectionLocalSnapshot, String, Option<String>), String> {
    use std::io::{Read, Seek, SeekFrom};
    let is_cue = album_locator.starts_with("sqlite-cue-album:");
    let (source_id, identity) = if is_cue {
        parse_sqlite_cue_album_id(album_locator)?
    } else {
        parse_sqlite_album_id(album_locator)?
    };
    let source = catalog
        .sources()?
        .into_iter()
        .find(|s| s.id == source_id)
        .ok_or("COLLECTION_LOCAL_SOURCE_MISSING")?;
    if !std::fs::metadata(&source.canonical_path).is_ok_and(|m| m.is_dir())
        || std::fs::read_dir(&source.canonical_path).is_err()
    {
        return Err("COLLECTION_LOCAL_UNAVAILABLE".into());
    }
    let view = catalog.source_library_view(source_id)?;
    let mut segments = Vec::new();
    let (selected, title, artist) = if is_cue {
        let groups = cue_album_groups(view.cue_tracks());
        let group = groups.iter().find(|g| cue_album_id(source_id, g) == album_locator)
            .ok_or("COLLECTION_LOCAL_ALBUM_MISSING")?;
        let audio_paths = group.tracks.iter().map(|t| &t.audio_path).collect::<BTreeSet<_>>();
        if groups.iter().filter(|other| cue_album_id(source_id, other) != album_locator)
            .any(|other| other.tracks.iter().any(|track| audio_paths.contains(&track.audio_path))) {
            return Err("COLLECTION_LOCAL_AMBIGUOUS".into());
        }
        let dto = cue_album_dto(&source, group);
        let mut paths = BTreeSet::new();
        for t in &group.tracks {
            view.verified_cue_track(&t.cue_path, t.track_number)?
                .ok_or("COLLECTION_LOCAL_CUE_CHANGED")?;
            paths.insert(t.audio_path.clone());
            segments.push(CollectionCueSegment {
                cue: collection_local_path(&t.cue_path),
                audio: collection_local_path(&t.audio_path),
                track: t.track_number,
                start: t.start_frame,
                end: t.end_frame,
                cue_size: t.cue_size,
                cue_mtime_ns: t.cue_mtime_ns,
                cue_device: t.cue_device,
                cue_inode: t.cue_inode,
                cue_ctime_ns: t.cue_ctime_ns,
            });
        }
        let selected = catalog
            .tracks(source_id)?
            .into_iter()
            .filter(|t| paths.contains(&t.relative_path))
            .collect::<Vec<_>>();
        if selected.len() != paths.len() {
            return Err("COLLECTION_LOCAL_CUE_CHANGED".into());
        }
        (selected, dto.title, dto.artist)
    } else {
        let album_id = AlbumId {
            source_id,
            identity,
        };
        let album = view
            .albums()
            .iter()
            .find(|a| a.id == album_id)
            .ok_or("COLLECTION_LOCAL_ALBUM_MISSING")?;
        let tracks = view
            .tracks_for_album(&album_id)
            .ok_or("COLLECTION_LOCAL_ALBUM_MISSING")?
            .collect::<Vec<_>>();
        if view.cue_tracks().iter().any(|cue| tracks.iter().any(|track| track.track.relative_path == cue.audio_path)) {
            return Err("COLLECTION_LOCAL_AMBIGUOUS".into());
        }
        let dto = local_browser_album_dto(&source, album, &tracks);
        (
            tracks
                .into_iter()
                .map(|item| item.track.clone())
                .collect::<Vec<_>>(),
            dto.title,
            dto.artist,
        )
    };
    if selected.is_empty() || selected.len() > 100_000 || segments.len() > 100_000 {
        return Err("COLLECTION_LOCAL_MEMBER_LIMIT".into());
    }
    let mut files = Vec::with_capacity(selected.len());
    for track in selected {
        let digest = catalog
            .with_verified_source_track_file(
                source_id,
                view.generation(),
                &track.relative_path,
                |mut reader, length| -> std::io::Result<String> {
                    let mut hash = Sha256::new();
                    hash.update(length.to_le_bytes());
                    let mut bytes = vec![0u8; (length.min(16 * 1024)) as usize];
                    reader.read_exact(&mut bytes)?;
                    hash.update(&bytes);
                    if length > 16 * 1024 {
                        reader.seek(SeekFrom::Start(length - 16 * 1024))?;
                        let mut tail = vec![0u8; 16 * 1024];
                        reader.read_exact(&mut tail)?;
                        hash.update(&tail);
                    }
                    Ok(format!("{:x}", hash.finalize()))
                },
            )?
            .map_err(|_| "COLLECTION_LOCAL_READ_FAILED")?;
        files.push(CollectionLocalFile {
            path: collection_local_path(&track.relative_path),
            size: track.size,
            mtime_ns: track.mtime_ns,
            sample_sha256: digest,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if files.windows(2).any(|w| w[0].path == w[1].path)
        || !catalog.is_source_library_view_current(&view)?
    {
        return Err("COLLECTION_LOCAL_AMBIGUOUS".into());
    }
    Ok((
        CollectionLocalSnapshot {
            version: 1,
            album_locator: album_locator.to_owned(),
            files,
            segments,
        },
        title,
        (!artist.trim().is_empty()).then_some(artist),
    ))
}

fn collection_snapshot_reference(scope: &str, snapshot: &CollectionLocalSnapshot) -> Result<String, String> {
    let bytes = serde_json::to_vec(snapshot).map_err(|_| "COLLECTION_LOCAL_INVALID")?;
    let mut hash = Sha256::new();
    hash.update(b"sonante-local-collection-v2\0");
    hash.update((scope.len() as u64).to_be_bytes());
    hash.update(scope.as_bytes());
    hash.update(bytes);
    Ok(format!("local2:{:x}", hash.finalize()))
}

fn collection_evidence_from_snapshot(snapshot: &CollectionLocalSnapshot) -> Result<sonante_sqlite_catalog::collections::LocalEvidence, String> {
    use sonante_sqlite_catalog::collections::{LocalEvidence, LocalEvidenceFile, LocalEvidenceSegment};
    let decode = |s: &str| URL_SAFE_NO_PAD.decode(s).map_err(|_| "COLLECTION_LOCAL_INVALID".to_owned());
    let files = snapshot.files.iter().map(|f| Ok(LocalEvidenceFile {
        path: decode(&f.path)?, size: f.size, mtime_ns: f.mtime_ns, sample_sha256: decode_hex(&f.sample_sha256)?,
    })).collect::<Result<Vec<_>, String>>()?;
    let segments = snapshot.segments.iter().map(|c| Ok(LocalEvidenceSegment {
        cue_path: decode(&c.cue)?, audio_path: decode(&c.audio)?, track: c.track,
        start_frame: c.start, end_frame: c.end, cue_size: c.cue_size, cue_mtime_ns: c.cue_mtime_ns,
        cue_device: c.cue_device, cue_inode: c.cue_inode, cue_ctime_ns: c.cue_ctime_ns,
    })).collect::<Result<Vec<_>, String>>()?;
    Ok(LocalEvidence { album_locator: snapshot.album_locator.clone(), files, segments })
}

fn collection_snapshot_from_evidence(evidence: sonante_sqlite_catalog::collections::LocalEvidence) -> CollectionLocalSnapshot {
    CollectionLocalSnapshot {
        version: 1, album_locator: evidence.album_locator,
        files: evidence.files.into_iter().map(|f| CollectionLocalFile {
            path: URL_SAFE_NO_PAD.encode(f.path), size: f.size, mtime_ns: f.mtime_ns,
            sample_sha256: f.sample_sha256.iter().map(|b| format!("{b:02x}")).collect(),
        }).collect(),
        segments: evidence.segments.into_iter().map(|c| CollectionCueSegment {
            cue: URL_SAFE_NO_PAD.encode(c.cue_path), audio: URL_SAFE_NO_PAD.encode(c.audio_path), track: c.track,
            start: c.start_frame, end: c.end_frame, cue_size: c.cue_size, cue_mtime_ns: c.cue_mtime_ns,
            cue_device: c.cue_device, cue_inode: c.cue_inode, cue_ctime_ns: c.cue_ctime_ns,
        }).collect(),
    }
}

fn collection_local_add(catalog: &mut Catalog, id: i64, album_locator: &str) -> Result<i64, String> {
    if !crate::music_sources::local() { return Err("COLLECTION_LOCAL_UNAVAILABLE".into()); }
    let (source_id, _) = if album_locator.starts_with("sqlite-cue-album:") {
        parse_sqlite_cue_album_id(album_locator)?
    } else { parse_sqlite_album_id(album_locator)? };
    let (snapshot, title, artist) = collection_local_snapshot(catalog, album_locator)?;
    let scope = format!("local1:{source_id}");
    let reference = collection_snapshot_reference(&scope, &snapshot)?;
    let evidence = collection_evidence_from_snapshot(&snapshot)?;
    let member = sonante_sqlite_catalog::collections::NewMember { source: "local".into(), scope, reference, title, artist };
    let legacy = format!("local1:{}", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&snapshot).map_err(|_| "COLLECTION_LOCAL_INVALID")?));
    let legacy = (legacy.len() <= 4096).then_some(legacy.as_str());
    catalog.collection_add_local_evidence(id, member, &evidence, legacy)
}

fn collection_local_resolve_member(catalog: &mut Catalog, collection: i64, member: i64, scope: &str, reference: &str) -> Result<String, &'static str> {
    if !reference.starts_with("local2:") { return collection_local_resolve_target(catalog, scope, reference); }
    if !crate::music_sources::local() { return Err("unavailable"); }
    let saved = collection_snapshot_from_evidence(catalog.collection_local_evidence(collection, member).map_err(|_| "unresolved")?);
    if collection_snapshot_reference(scope, &saved).ok().as_deref() != Some(reference) { return Err("unresolved"); }
    let source_id = scope.strip_prefix("local1:").and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0).ok_or("unresolved")?;
    let parsed = if saved.album_locator.starts_with("sqlite-cue-album:") { parse_sqlite_cue_album_id(&saved.album_locator) } else { parse_sqlite_album_id(&saved.album_locator) };
    if !parsed.is_ok_and(|(source, _)| source == source_id) { return Err("unresolved"); }
    match collection_local_snapshot(catalog, &saved.album_locator) {
        Ok((actual, _, _)) if actual == saved => Ok(saved.album_locator),
        Err(error) if error == "COLLECTION_LOCAL_UNAVAILABLE" => Err("unavailable"),
        _ => Err("unresolved"),
    }
}

#[cfg(test)]
fn collection_local_capture(
    catalog: &Catalog,
    album_locator: &str,
) -> Result<sonante_sqlite_catalog::collections::NewMember, String> {
    if !crate::music_sources::local() {
        return Err("COLLECTION_LOCAL_UNAVAILABLE".into());
    }
    let (source_id, _) = if album_locator.starts_with("sqlite-cue-album:") {
        parse_sqlite_cue_album_id(album_locator)?
    } else {
        parse_sqlite_album_id(album_locator)?
    };
    let (snapshot, title, artist) = collection_local_snapshot(catalog, album_locator)?;
    let bytes = serde_json::to_vec(&snapshot).map_err(|_| "COLLECTION_LOCAL_INVALID")?;
    let reference = format!("local1:{}", URL_SAFE_NO_PAD.encode(bytes));
    if reference.len() > 4096 {
        return Err("COLLECTION_LOCAL_SNAPSHOT_TOO_LARGE".into());
    }
    Ok(sonante_sqlite_catalog::collections::NewMember {
        source: "local".into(),
        scope: format!("local1:{source_id}"),
        reference,
        title,
        artist,
    })
}

fn collection_local_resolve_target(
    catalog: &Catalog,
    scope: &str,
    reference: &str,
) -> Result<String, &'static str> {
    if !crate::music_sources::local() {
        return Err("unavailable");
    }
    let Some(id) = scope
        .strip_prefix("local1:")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|id| *id > 0)
    else {
        return Err("unresolved");
    };
    let Some(bytes) = reference
        .strip_prefix("local1:")
        .and_then(|s| URL_SAFE_NO_PAD.decode(s).ok())
    else {
        return Err("unresolved");
    };
    let Ok(saved) = serde_json::from_slice::<CollectionLocalSnapshot>(&bytes) else {
        return Err("unresolved");
    };
    if saved.version != 1
        || saved.files.is_empty()
        || saved.files.len() > 256
        || !(if saved.album_locator.starts_with("sqlite-cue-album:") {
            parse_sqlite_cue_album_id(&saved.album_locator)
        } else {
            parse_sqlite_album_id(&saved.album_locator)
        })
        .is_ok_and(|(source, _)| source == id)
    {
        return Err("unresolved");
    }
    match collection_local_snapshot(catalog, &saved.album_locator) {
        Ok((actual, _, _)) if actual == saved => Ok(saved.album_locator),
        Err(error) if error == "COLLECTION_LOCAL_UNAVAILABLE" => Err("unavailable"),
        _ => Err("unresolved"),
    }
}

#[cfg(test)]
fn collection_local_resolve(catalog: &Catalog, scope: &str, reference: &str) -> &'static str {
    match collection_local_resolve_target(catalog, scope, reference) {
        Ok(_) => "available",
        Err(status) => status,
    }
}

#[tauri::command]
pub async fn personal_collection_add_local(
    state: State<'_, SqliteLibraryState>,
    id: i64,
    album_id: String,
) -> Result<i64, String> {
    let path = state.inner.database_path.clone();
    blocking(move || {
        let mut catalog = Catalog::open(path)?;
        collection_local_add(&mut catalog, id, &album_id)
    })
    .await
}
