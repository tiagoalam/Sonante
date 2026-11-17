//! Private, versioned presentation snapshot for the Windows Local catalog.
//! A restored snapshot is cache only; it never carries a live scan signature.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashSet},
    fs::OpenOptions,
    io::Read,
};

const VERSION: u8 = 1;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ALBUMS: usize = 100_000;
const MAX_TEXT: usize = 32_768;
const MAX_WARNINGS_PER_ROOT: usize = 500_000;
const ERROR: &str = "windows_catalog_snapshot_invalid";

#[cfg(test)]
fn snapshot_test_trace(stage: &'static str) {
    if std::env::var_os("SONANTE_SNAPSHOT_TEST_TRACE").is_some() {
        eprintln!("SNAPSHOT_TEST_STAGE={stage}");
    }
}

// Active consolidated Windows catalog format. Version 1 remains readable for
// the one-time rebuild path, but is never rewritten as v1.
const CONTRACT_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RootQualityV2 {
    Confirmed,
    Provisional,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RootStateV2 {
    root_id: String,
    path: String,
    quality: RootQualityV2,
    warning_counts: crate::local_diagnostics::LibraryWarningCounts,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RootOperationKindV2 {
    Add,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RootTransactionV2 {
    transaction_id: String,
    kind: RootOperationKindV2,
    root_id: String,
    root_path: String,
    base_generation: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotV2Envelope {
    version: u8,
    checksum: String,
    payload: SnapshotV2Payload,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotV2Payload {
    generation: u64,
    last_transaction_id: Option<String>,
    roots: Vec<RootStateV2>,
    records: Vec<SnapshotRecordV2>,
    signature: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRecordV2 {
    root_id: String,
    album: SnapshotAlbum,
    folder: String,
    cover: Option<String>,
    search: String,
    tracks: Vec<SnapshotTrack>,
    durations: Vec<SnapshotDuration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootRecoveryV2 {
    Committed,
    PendingAdd,
    PendingRemove,
    Candidate,
    NeedsRecovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotFormatV2 {
    LegacyV1,
    CurrentV2,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryDecisionV2 {
    UseCurrent,
    UsePrevious,
    Pending,
    FinalizeIntent,
    RebuildLegacyOnce,
    NeedsRecovery,
}

fn detect_format_v2(bytes: &[u8]) -> Result<SnapshotFormatV2> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ERROR)?;
    let version = value.get("version").and_then(serde_json::Value::as_u64).ok_or(ERROR)?;
    Ok(match version {
        1 => SnapshotFormatV2::LegacyV1,
        value if value == CONTRACT_VERSION as u64 => SnapshotFormatV2::CurrentV2,
        _ => SnapshotFormatV2::Unsupported,
    })
}

fn encode_v2(payload: SnapshotV2Payload) -> Result<Vec<u8>> {
    validate_v2(&payload)?;
    let payload_bytes = serde_json::to_vec(&payload).map_err(|_| ERROR)?;
    let envelope = SnapshotV2Envelope {
        version: CONTRACT_VERSION,
        checksum: digest_hex(&Sha256::digest(&payload_bytes)),
        payload,
    };
    serde_json::to_vec(&envelope).map_err(|_| ERROR)
}

fn decode_v2(bytes: &[u8]) -> Result<SnapshotV2Payload> {
    if detect_format_v2(bytes)? != SnapshotFormatV2::CurrentV2 {
        return Err(ERROR);
    }
    let envelope: SnapshotV2Envelope = serde_json::from_slice(bytes).map_err(|_| ERROR)?;
    let payload_bytes = serde_json::to_vec(&envelope.payload).map_err(|_| ERROR)?;
    if envelope.checksum != digest_hex(&Sha256::digest(&payload_bytes)) {
        return Err(ERROR);
    }
    validate_v2(&envelope.payload)?;
    Ok(envelope.payload)
}

fn recovery_decision_v2(
    current: Option<&SnapshotV2Payload>,
    previous: Option<&SnapshotV2Payload>,
    intent: Option<&RootTransactionV2>,
) -> RecoveryDecisionV2 {
    if let Some(intent) = intent {
        if current.is_some_and(|snapshot| {
            snapshot.generation == intent.base_generation.saturating_add(1)
                && snapshot.last_transaction_id.as_deref() == Some(intent.transaction_id.as_str())
        }) {
            return RecoveryDecisionV2::FinalizeIntent;
        }
        if current.is_some_and(|snapshot| snapshot.generation == intent.base_generation) {
            return RecoveryDecisionV2::Pending;
        }
        if previous.is_some_and(|snapshot| snapshot.generation == intent.base_generation) {
            return RecoveryDecisionV2::Pending;
        }
        return RecoveryDecisionV2::NeedsRecovery;
    }
    match (current, previous) {
        (Some(_), _) => RecoveryDecisionV2::UseCurrent,
        (None, Some(_)) => RecoveryDecisionV2::UsePrevious,
        (None, None) => RecoveryDecisionV2::NeedsRecovery,
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_warning_counts(value: &crate::local_diagnostics::LibraryWarningCounts) -> bool {
    value.depth_limit <= MAX_WARNINGS_PER_ROOT
        && value.directory_path <= MAX_WARNINGS_PER_ROOT
        && value.directory_read <= MAX_WARNINGS_PER_ROOT
        && value.entry_read <= MAX_WARNINGS_PER_ROOT
        && value.metadata_read <= MAX_WARNINGS_PER_ROOT
        && value.file_containment <= MAX_WARNINGS_PER_ROOT
        && value.metadata_decode <= MAX_WARNINGS_PER_ROOT
}

fn validate_v2(payload: &SnapshotV2Payload) -> Result<()> {
    if payload.generation == 0 || payload.roots.is_empty() || payload.roots.len() > 32
        || payload.records.len() > MAX_ALBUMS
        || payload.last_transaction_id.as_deref().is_some_and(|id| !valid_identifier(id))
    {
        return Err(ERROR);
    }
    let mut roots = std::collections::BTreeMap::new();
    let mut ids = HashSet::new();
    for root in &payload.roots {
        if !valid_identifier(&root.root_id) || root.path.is_empty() || root.path.len() > MAX_TEXT
            || root.root_id != digest_hex(&Sha256::digest(root.path.as_bytes()))
            || roots.insert(root.root_id.as_str(), root.path.as_str()).is_some()
            || !valid_warning_counts(&root.warning_counts)
            || !root.warning_counts.metadata_only()
            || (root.quality == RootQualityV2::Confirmed && root.warning_counts.total() != 0)
            || (root.quality == RootQualityV2::Provisional && root.warning_counts.metadata_decode == 0)
        {
            return Err(ERROR);
        }
    }
    for record in &payload.records {
        if !roots.contains_key(record.root_id.as_str()) {
            return Err(ERROR);
        }
        if !ids.insert(record.album.id.as_str()) {
            return Err(ERROR);
        }
    }
    parse_digest(&payload.signature)?;
    let root_paths: std::collections::BTreeMap<String, PathBuf> = payload.roots.iter()
        .map(|root| (root.path.clone(), PathBuf::from(&root.path))).collect();
    let root_ids: std::collections::BTreeMap<&str, &str> = payload.roots.iter()
        .map(|root| (root.root_id.as_str(), root.path.as_str())).collect();
    let mut track_ids = HashSet::new();
    let mut total_tracks = 0;
    for item in &payload.records {
        let root = root_ids.get(item.root_id.as_str()).copied().ok_or(ERROR)?;
        decode_record(SnapshotRecord { root: root.to_owned(), album: item.album.clone(),
            folder: item.folder.clone(), cover: item.cover.clone(), search: item.search.clone(),
            tracks: item.tracks.clone(), durations: item.durations.clone() },
            &root_paths, &mut track_ids, &mut total_tracks)?;
    }
    Ok(())
}

fn classify_root_v2(
    configured: bool,
    indexed: bool,
    pending: Option<&RootTransactionV2>,
    root_id: &str,
    candidate: bool,
) -> RootRecoveryV2 {
    if let Some(tx) = pending.filter(|tx| tx.root_id == root_id) {
        if !valid_identifier(&tx.transaction_id) || !valid_identifier(&tx.root_id) {
            return RootRecoveryV2::NeedsRecovery;
        }
        return match (tx.kind, configured, indexed, candidate) {
            (RootOperationKindV2::Add, true, false, true) => RootRecoveryV2::Candidate,
            (RootOperationKindV2::Add, true, false, false) => RootRecoveryV2::PendingAdd,
            (RootOperationKindV2::Remove, false, true, false) => RootRecoveryV2::PendingRemove,
            _ => RootRecoveryV2::NeedsRecovery,
        };
    }
    if candidate { return RootRecoveryV2::NeedsRecovery; }
    match (configured, indexed) {
        (true, true) => RootRecoveryV2::Committed,
        (true, false) => RootRecoveryV2::PendingAdd,
        (false, true) => RootRecoveryV2::PendingRemove,
        (false, false) => RootRecoveryV2::NeedsRecovery,
    }
}

fn rejected(reason: crate::local_diagnostics::SnapshotRejection) -> &'static str {
    crate::local_diagnostics::record_library_event(
        crate::local_diagnostics::LibraryEvent::SnapshotRejected { reason },
        Some(ERROR),
    );
    ERROR
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    checksum: String,
    payload: SnapshotPayload,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPayload {
    roots: Vec<String>,
    signature: String,
    records: Vec<SnapshotRecord>,
    #[serde(default, skip_serializing_if = "is_clean")]
    qualification: SnapshotQualification,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum SnapshotQualification {
    #[default]
    Clean,
    Provisional {
        warning_counts: crate::local_diagnostics::LibraryWarningCounts,
    },
}

fn is_clean(value: &SnapshotQualification) -> bool {
    matches!(value, SnapshotQualification::Clean)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotRecord {
    root: String,
    album: SnapshotAlbum,
    folder: String,
    cover: Option<String>,
    search: String,
    tracks: Vec<SnapshotTrack>,
    durations: Vec<SnapshotDuration>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotAlbum {
    id: String,
    title: String,
    artist: String,
    year: Option<u32>,
    genre: Option<String>,
    track_count: usize,
    has_cover: bool,
    added_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotTrack {
    id: String,
    path: String,
    title: String,
    artist: String,
    album: String,
    disc: u32,
    number: u32,
    file_name: String,
    catalog_album_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDuration {
    track_id: String,
    seconds: f64,
}

#[derive(Clone)]
pub(super) struct Restored {
    pub records: HashMap<String, Record>,
    pub signature: [u8; 32],
    pub provisional: bool,
    pub warning_counts: crate::local_diagnostics::LibraryWarningCounts,
    pub generation: u64,
}

pub(super) enum LoadResult {
    Missing,
    LegacyV1,
    Restored(Restored),
    Incompatible,
}

#[derive(Clone, Copy)]
pub(super) enum SaveFailure {
    CatalogShape,
    RootIdentity,
    RecordValidation,
    Serialization,
    SizeLimit,
    Destination,
    Lock,
    AtomicWrite,
}

impl From<SaveFailure> for crate::local_diagnostics::SnapshotWriteFailure {
    fn from(value: SaveFailure) -> Self {
        match value {
            SaveFailure::CatalogShape => Self::CatalogShape,
            SaveFailure::RootIdentity => Self::RootIdentity,
            SaveFailure::RecordValidation => Self::RecordValidation,
            SaveFailure::Serialization => Self::Serialization,
            SaveFailure::SizeLimit => Self::SizeLimit,
            SaveFailure::Destination => Self::Destination,
            SaveFailure::Lock => Self::Lock,
            SaveFailure::AtomicWrite => Self::AtomicWrite,
        }
    }
}

pub(super) fn path(config: &Path) -> Result<PathBuf> {
    Ok(config.parent().ok_or(ERROR)?.join("catalog-v1.json"))
}

fn digest_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_digest(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ERROR);
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| ERROR)?;
    }
    Ok(digest)
}

fn root_key(root: &Path) -> Result<String> {
    root.to_str()
        .filter(|value| !value.is_empty() && value.len() <= MAX_TEXT)
        .map(str::to_owned)
        .ok_or(ERROR)
}

// Diagnostic only: this lexical comparison explains common spelling drift
// without changing the exact root identity required to load a snapshot.
fn root_representation_key(root: &str) -> String {
    root.replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

fn relative(path: &Path, root: &Path, allow_empty: bool) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| ERROR)?;
    let value = relative.to_str().ok_or(ERROR)?;
    validate_relative(value, allow_empty)?;
    Ok(value.to_owned())
}

fn validate_relative(value: &str, allow_empty: bool) -> Result<()> {
    if value.len() > MAX_TEXT || value.contains(['\0', '\r', '\n', ':']) {
        return Err(ERROR);
    }
    if value.is_empty() {
        return if allow_empty { Ok(()) } else { Err(ERROR) };
    }
    if value.starts_with(['/', '\\'])
        || value
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ERROR);
    }
    Ok(())
}

fn restored_path(root: &Path, relative: &str, allow_empty: bool) -> Result<PathBuf> {
    validate_relative(relative, allow_empty)?;
    let path = if relative.is_empty() {
        root.to_owned()
    } else {
        root.join(relative)
    };
    if !path.starts_with(root) {
        return Err(ERROR);
    }
    Ok(path)
}

fn encode_record(record: &Record) -> Result<SnapshotRecord> {
    let root = root_key(&record.root)?;
    if record.tracks.is_empty()
        || record.tracks.len() > MAX_TRACKS
        || record.album.track_count != record.tracks.len()
        || record.album.has_cover != record.cover.is_some()
        || record.tracks.iter().any(|track| {
            track.media_root.as_deref() != Some(record.root.as_path())
                || track.radio.is_some()
                || track.upnp.is_some()
                || track.remote_source.is_some()
                || track.remote_connection_id.is_some()
                || track.plex_image.is_some()
                || track.remote_lease.is_some()
        })
    {
        return Err(ERROR);
    }
    let folder = relative(&record.folder, &record.root, true)?;
    let expected_album = id(&format!(
        "{}\0{}",
        record.folder.to_string_lossy(),
        record.album.title
    ));
    if record.album.id != expected_album {
        return Err(ERROR);
    }
    let mut tracks = Vec::with_capacity(record.tracks.len());
    let mut track_ids = HashSet::new();
    for track in &record.tracks {
        let path = relative(&track.path, &record.root, false)?;
        if track.id != id(&track.path.to_string_lossy())
            || track.album != record.album.title
            || track.catalog_album_id.as_deref() != Some(record.album.id.as_str())
            || !track_ids.insert(track.id.clone())
        {
            return Err(ERROR);
        }
        tracks.push(SnapshotTrack {
            id: track.id.clone(),
            path,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            disc: track.disc,
            number: track.number,
            file_name: track.file_name.clone(),
            catalog_album_id: track.catalog_album_id.clone(),
        });
    }
    tracks.sort_by(|left, right| left.id.cmp(&right.id));
    let mut durations = record
        .durations
        .iter()
        .map(|(track_id, seconds)| {
            if !track_ids.contains(track_id) || !seconds.is_finite() || *seconds < 0.0 {
                return Err(ERROR);
            }
            Ok(SnapshotDuration {
                track_id: track_id.clone(),
                seconds: *seconds,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    durations.sort_by(|left, right| left.track_id.cmp(&right.track_id));
    Ok(SnapshotRecord {
        root,
        album: SnapshotAlbum {
            id: record.album.id.clone(),
            title: record.album.title.clone(),
            artist: record.album.artist.clone(),
            year: record.album.year,
            genre: record.album.genre.clone(),
            track_count: record.album.track_count,
            has_cover: record.album.has_cover,
            added_at: record.album.added_at,
        },
        folder,
        cover: record
            .cover
            .as_deref()
            .map(|path| relative(path, &record.root, false))
            .transpose()?,
        search: record.search.clone(),
        tracks,
        durations,
    })
}

fn decode_record(
    dto: SnapshotRecord,
    roots: &std::collections::BTreeMap<String, PathBuf>,
    ids: &mut HashSet<String>,
    total_tracks: &mut usize,
) -> Result<Record> {
    let root = roots.get(&dto.root).ok_or(ERROR)?.clone();
    if dto.album.id.len() != 64
        || !dto.album.id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || dto.album.title.len() > MAX_TEXT
        || dto.album.artist.len() > MAX_TEXT
        || dto
            .album
            .genre
            .as_ref()
            .is_some_and(|genre| genre.len() > MAX_TEXT)
        || dto.search.len() > MAX_TEXT
        || dto.tracks.is_empty()
        || dto.tracks.len() > MAX_TRACKS
        || dto.album.track_count != dto.tracks.len()
        || dto.durations.len() > dto.tracks.len()
    {
        return Err(ERROR);
    }
    let folder = restored_path(&root, &dto.folder, true)?;
    if dto.album.id
        != id(&format!(
            "{}\0{}",
            folder.to_string_lossy(),
            dto.album.title
        ))
    {
        return Err(ERROR);
    }
    let cover = dto
        .cover
        .as_deref()
        .map(|value| restored_path(&root, value, false))
        .transpose()?;
    if dto.album.has_cover != cover.is_some() {
        return Err(ERROR);
    }
    let mut tracks = Vec::with_capacity(dto.tracks.len());
    for track in dto.tracks {
        let path = restored_path(&root, &track.path, false)?;
        if track.id.len() != 64
            || !track.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || track.id != id(&path.to_string_lossy())
            || track.album != dto.album.title
            || track.catalog_album_id.as_deref() != Some(dto.album.id.as_str())
            || track.file_name.len() > MAX_TEXT
            || track.title.len() > MAX_TEXT
            || track.artist.len() > MAX_TEXT
            || path.file_name().and_then(|name| name.to_str()) != Some(track.file_name.as_str())
            || !ids.insert(track.id.clone())
        {
            return Err(ERROR);
        }
        *total_tracks = total_tracks
            .checked_add(1)
            .filter(|count| *count <= MAX_TRACKS)
            .ok_or(ERROR)?;
        tracks.push(Track {
            id: track.id,
            title: track.title,
            artist: track.artist,
            album: track.album,
            disc: track.disc,
            number: track.number,
            file_name: track.file_name,
            catalog_album_id: track.catalog_album_id,
            path,
            media_root: Some(root.clone()),
            radio: None,
            upnp: None,
            remote_source: None,
            remote_connection_id: None,
            plex_image: None,
            remote_lease: None,
        });
    }
    let mut durations = HashMap::new();
    for duration in dto.durations {
        if !duration.seconds.is_finite()
            || duration.seconds < 0.0
            || !tracks.iter().any(|track| track.id == duration.track_id)
            || durations
                .insert(duration.track_id, duration.seconds)
                .is_some()
        {
            return Err(ERROR);
        }
    }
    let album = Album {
        id: dto.album.id,
        title: dto.album.title,
        artist: dto.album.artist,
        year: dto.album.year,
        genre: dto.album.genre,
        track_count: dto.album.track_count,
        has_cover: dto.album.has_cover,
        added_at: dto.album.added_at,
    };
    Ok(Record {
        root,
        album,
        tracks,
        folder,
        cover,
        search: dto.search,
        durations,
    })
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    #[cfg(test)]
    snapshot_test_trace("read_bytes_prepare");
    crate::persistence::prepare_private_file_for_load_limited(path, ERROR, MAX_BYTES).map_err(|_| ERROR)?;
    #[cfg(test)]
    snapshot_test_trace("read_bytes_prepared");
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ERROR),
    };
    if !metadata.is_file() || linked(&metadata) || metadata.len() > MAX_BYTES {
        return Err(ERROR);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    #[cfg(test)]
    snapshot_test_trace("read_bytes_open");
    let mut bytes = Vec::new();
    options
        .open(path)
        .map_err(|_| ERROR)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes).map_err(|_| ERROR)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ERROR);
    }
    #[cfg(test)]
    snapshot_test_trace("read_bytes_done");
    Ok(Some(bytes))
}

fn decode_v1(bytes: &[u8], configured_roots: &[PathBuf]) -> Result<Option<Restored>> {
    let snapshot: Snapshot = serde_json::from_slice(&bytes)
        .map_err(|_| ERROR)?;
    if snapshot.version != VERSION {
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::SnapshotIncompatible {
                reason: crate::local_diagnostics::SnapshotIncompatibility::Version,
                saved_root_count: snapshot.payload.roots.len(),
                configured_root_count: configured_roots.len(),
            }, None);
        return Ok(None);
    }
    if snapshot.payload.roots.len() != configured_roots.len() || snapshot.payload.roots.len() > 32 {
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::SnapshotIncompatible {
                reason: crate::local_diagnostics::SnapshotIncompatibility::RootCount,
                saved_root_count: snapshot.payload.roots.len(),
                configured_root_count: configured_roots.len(),
            }, None);
        return Ok(None);
    }
    let payload_bytes = serde_json::to_vec(&snapshot.payload)
        .map_err(|_| rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload))?;
    let checksum = digest_hex(&Sha256::digest(&payload_bytes));
    if checksum != snapshot.checksum {
        return Err(rejected(crate::local_diagnostics::SnapshotRejection::Checksum));
    }
    let (provisional, warning_counts) = match snapshot.payload.qualification {
        SnapshotQualification::Clean => (false, crate::local_diagnostics::LibraryWarningCounts::default()),
        SnapshotQualification::Provisional { warning_counts }
            if valid_warning_counts(&warning_counts) && warning_counts.metadata_only() && warning_counts.metadata_decode > 0 =>
        {
            (true, warning_counts)
        }
        SnapshotQualification::Provisional { .. } => {
            return Err(rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload));
        }
    };
    let configured = configured_roots
        .iter()
        .map(|root| root_key(root))
        .collect::<Result<BTreeSet<_>>>()
        .map_err(|_| rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload))?;
    let saved = snapshot
        .payload
        .roots
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if configured != saved {
        let configured_representation = configured_roots
            .iter()
            .filter_map(|root| root_key(root).ok())
            .map(|root| root_representation_key(&root))
            .collect::<BTreeSet<_>>();
        let saved_representation = snapshot.payload.roots
            .iter()
            .map(|root| root_representation_key(root))
            .collect::<BTreeSet<_>>();
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::SnapshotIncompatible {
                reason: if configured_representation == saved_representation {
                    crate::local_diagnostics::SnapshotIncompatibility::RootRepresentation
                } else {
                    crate::local_diagnostics::SnapshotIncompatibility::RootSet
                },
                saved_root_count: saved.len(),
                configured_root_count: configured.len(),
            }, None);
        return Ok(None);
    }
    let signature = parse_digest(&snapshot.payload.signature)
        .map_err(|_| rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload))?;
    if snapshot.payload.records.len() > MAX_ALBUMS {
        return Err(rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload));
    }
    let root_map = configured_roots
        .iter()
        .map(|root| Ok((root_key(root)?, root.clone())))
        .collect::<Result<std::collections::BTreeMap<_, _>>>()
        .map_err(|_| rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload))?;
    let mut records = HashMap::with_capacity(snapshot.payload.records.len());
    let mut ids = HashSet::new();
    let mut total_tracks = 0;
    for record in snapshot.payload.records {
        let record = decode_record(record, &root_map, &mut ids, &mut total_tracks)
            .map_err(|_| rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload))?;
        if records.insert(record.album.id.clone(), record).is_some() {
            return Err(rejected(crate::local_diagnostics::SnapshotRejection::InvalidPayload));
        }
    }
    crate::local_diagnostics::record_library_event(
        crate::local_diagnostics::LibraryEvent::SnapshotRestored {
            root_count: configured_roots.len(),
            album_count: records.len(),
            track_count: total_tracks,
            provisional,
            metadata_warning_count: warning_counts.metadata_decode,
        }, None);
    Ok(Some(Restored { records, signature, provisional, warning_counts, generation: 0 }))
}

fn decode_v2_records(payload: &SnapshotV2Payload, configured_roots: &[PathBuf]) -> Result<Restored> {
    validate_v2(payload)?;
    let configured = configured_roots.iter().map(|r| root_key(r)).collect::<Result<BTreeSet<_>>>()?;
    let saved = payload.roots.iter().map(|r| r.path.clone()).collect::<BTreeSet<_>>();
    if configured != saved { return Err(ERROR); }
    let roots: std::collections::BTreeMap<_, _> = configured_roots.iter()
        .map(|root| Ok((root_key(root)?, root.clone()))).collect::<Result<_>>()?;
    let roots_by_id: std::collections::BTreeMap<_, _> = payload.roots.iter()
        .map(|root| (root.root_id.as_str(), root.path.as_str())).collect();
    let mut records = HashMap::with_capacity(payload.records.len());
    let mut track_ids = HashSet::new();
    let mut total_tracks = 0;
    for item in &payload.records {
        let root = roots_by_id.get(item.root_id.as_str()).ok_or(ERROR)?;
        let dto = SnapshotRecord {
            root: (*root).to_owned(), album: item.album.clone(), folder: item.folder.clone(),
            cover: item.cover.clone(), search: item.search.clone(), tracks: item.tracks.clone(),
            durations: item.durations.clone(),
        };
        let record = decode_record(dto, &roots, &mut track_ids, &mut total_tracks)?;
        if records.insert(record.album.id.clone(), record).is_some() { return Err(ERROR); }
    }
    let mut warning_counts = crate::local_diagnostics::LibraryWarningCounts::default();
    let mut provisional = false;
    for root in &payload.roots {
        provisional |= root.quality == RootQualityV2::Provisional;
        warning_counts.depth_limit += root.warning_counts.depth_limit;
        warning_counts.directory_path += root.warning_counts.directory_path;
        warning_counts.directory_read += root.warning_counts.directory_read;
        warning_counts.entry_read += root.warning_counts.entry_read;
        warning_counts.metadata_read += root.warning_counts.metadata_read;
        warning_counts.file_containment += root.warning_counts.file_containment;
        warning_counts.metadata_decode += root.warning_counts.metadata_decode;
    }
    let signature = parse_digest(&payload.signature)?;
    crate::local_diagnostics::record_library_event(
        crate::local_diagnostics::LibraryEvent::SnapshotRestored {
            root_count: payload.roots.len(), album_count: records.len(), track_count: total_tracks,
            provisional, metadata_warning_count: warning_counts.metadata_decode,
        }, None);
    Ok(Restored { records, signature, provisional, warning_counts, generation: payload.generation })
}

pub(super) fn load(config: &Path, configured_roots: &[PathBuf]) -> Result<LoadResult> {
    load_for_generation(config, configured_roots, None)
}

pub(super) fn load_for_generation(config: &Path, configured_roots: &[PathBuf], expected_generation: Option<u64>) -> Result<LoadResult> {
    #[cfg(test)]
    snapshot_test_trace("load_enter");
    let active = path(config).map_err(|_| ERROR)?;
    let previous = active.with_file_name("catalog-v1.previous.json");
    let active_bytes = read_bytes(&active).map_err(|_| rejected(crate::local_diagnostics::SnapshotRejection::UnsafeFile))?;
    #[cfg(test)]
    snapshot_test_trace("load_active_read");
    let previous_bytes = read_bytes(&previous).ok().flatten();
    #[cfg(test)]
    snapshot_test_trace("load_backup_read");
    if active_bytes.is_none() && previous_bytes.is_none() {
        crate::local_diagnostics::record_library_event(crate::local_diagnostics::LibraryEvent::SnapshotMissing, None);
        return Ok(if expected_generation.is_some() { LoadResult::Incompatible } else { LoadResult::Missing });
    }
    let decode = |bytes: &[u8]| -> Result<(SnapshotFormatV2, Option<Restored>)> {
        match detect_format_v2(bytes)? {
            SnapshotFormatV2::LegacyV1 => {
                if validate_stored(bytes) != Some(SnapshotFormatV2::LegacyV1) { return Err(ERROR); }
                Ok((SnapshotFormatV2::LegacyV1, decode_v1(bytes, configured_roots)?))
            }
            SnapshotFormatV2::CurrentV2 => {
                let payload = decode_v2(bytes)?;
                Ok((SnapshotFormatV2::CurrentV2, Some(decode_v2_records(&payload, configured_roots)?)))
            }
            SnapshotFormatV2::Unsupported => Err(ERROR),
        }
    };
    let current = active_bytes.as_deref().and_then(|b| decode(b).ok());
    #[cfg(test)]
    snapshot_test_trace("load_active_decoded");
    let backup = previous_bytes.as_deref().and_then(|b| decode(b).ok());
    #[cfg(test)]
    snapshot_test_trace("load_backup_decoded");
    if let Some(expected) = expected_generation {
        for candidate in [&current, &backup].into_iter().flatten() {
            if let (SnapshotFormatV2::CurrentV2, Some(restored)) = candidate {
                if restored.generation == expected { return Ok(LoadResult::Restored(restored.clone())); }
            }
        }
        return Ok(LoadResult::Incompatible);
    }
    if let Some((SnapshotFormatV2::CurrentV2, Some(restored))) = current.as_ref() {
        return Ok(LoadResult::Restored(restored.clone()));
    }
    if let Some((SnapshotFormatV2::CurrentV2, Some(restored))) = backup.as_ref() {
        return Ok(LoadResult::Restored(restored.clone()));
    }
    if current.as_ref().is_some_and(|(format, _)| *format == SnapshotFormatV2::LegacyV1)
        || backup.as_ref().is_some_and(|(format, _)| *format == SnapshotFormatV2::LegacyV1) {
        return Ok(LoadResult::LegacyV1);
    }
    if active_bytes.is_some() || previous_bytes.is_some() {
        crate::local_diagnostics::record_library_event(
            crate::local_diagnostics::LibraryEvent::SnapshotIncompatible {
                reason: crate::local_diagnostics::SnapshotIncompatibility::RootSet,
                saved_root_count: 0, configured_root_count: configured_roots.len(),
            }, None);
        return Ok(LoadResult::Incompatible);
    }
    #[cfg(test)]
    snapshot_test_trace("load_missing");
    Ok(LoadResult::Missing)
}

pub(super) fn save(
    config: &Path,
    roots: &[PathBuf],
    signature: [u8; 32],
    records: &HashMap<String, Record>,
    warning_counts: crate::local_diagnostics::LibraryWarningCounts,
) -> std::result::Result<(), SaveFailure> {
    let mut warnings = BTreeMap::new();
    if roots.len() == 1 { warnings.insert(roots[0].clone(), warning_counts); }
    else if warning_counts.total() != 0 { return Err(SaveFailure::RecordValidation); }
    save_v2(config, roots, signature, records, &warnings, warning_counts).map(|_| ())
}

pub(super) fn save_v2(
    config: &Path,
    roots: &[PathBuf],
    signature: [u8; 32],
    records: &HashMap<String, Record>,
    warnings_by_root: &BTreeMap<PathBuf, crate::local_diagnostics::LibraryWarningCounts>,
    expected_warnings: crate::local_diagnostics::LibraryWarningCounts,
) -> std::result::Result<u64, SaveFailure> {
    #[cfg(test)]
    snapshot_test_trace("save_enter");
    let mut total_warnings = crate::local_diagnostics::LibraryWarningCounts::default();
    for warnings in warnings_by_root.values() {
        total_warnings.depth_limit += warnings.depth_limit;
        total_warnings.directory_path += warnings.directory_path;
        total_warnings.directory_read += warnings.directory_read;
        total_warnings.entry_read += warnings.entry_read;
        total_warnings.metadata_read += warnings.metadata_read;
        total_warnings.file_containment += warnings.file_containment;
        total_warnings.metadata_decode += warnings.metadata_decode;
        if !warnings.metadata_only() || (warnings.total() > 0 && warnings.metadata_decode == 0) {
            return Err(SaveFailure::RecordValidation);
        }
    }
    if total_warnings != expected_warnings || warnings_by_root.keys().any(|root| !roots.contains(root)) {
        return Err(SaveFailure::RecordValidation);
    }
    if roots.is_empty() || roots.len() > 32 || records.len() > MAX_ALBUMS {
        return Err(SaveFailure::CatalogShape);
    }
    let path = path(config).map_err(|_| SaveFailure::Destination)?;
    #[cfg(test)]
    snapshot_test_trace("save_before_lock");
    let _lock = crate::persistence::operation_lock(&path.with_extension("lock")).map_err(|_| SaveFailure::Lock)?;
    #[cfg(test)]
    snapshot_test_trace("save_lock_acquired");
    let mut root_states = roots.iter().map(|root| {
        let path = root_key(root).map_err(|_| SaveFailure::RootIdentity)?;
        let warnings = warnings_by_root.get(root).copied().unwrap_or_default();
        Ok(RootStateV2 { root_id: digest_hex(&Sha256::digest(path.as_bytes())), path,
            quality: if warnings.total() == 0 { RootQualityV2::Confirmed } else { RootQualityV2::Provisional },
            warning_counts: warnings })
    }).collect::<std::result::Result<Vec<_>, SaveFailure>>()?;
    root_states.sort_by(|a,b| a.path.cmp(&b.path));
    if root_states.iter().map(|r| &r.path).collect::<BTreeSet<_>>().len() != roots.len() {
        return Err(SaveFailure::CatalogShape);
    }
    let mut encoded = records
        .values()
        .map(|record| {
            let dto = encode_record(record)?;
            Ok(SnapshotRecordV2 { root_id: digest_hex(&Sha256::digest(dto.root.as_bytes())),
                album: dto.album, folder: dto.folder, cover: dto.cover, search: dto.search,
                tracks: dto.tracks, durations: dto.durations })
        })
        .collect::<Result<Vec<_>>>()
        .map_err(|_| SaveFailure::RecordValidation)?;
    encoded.sort_by(|left, right| left.album.id.cmp(&right.album.id));
    let payload = SnapshotV2Payload {
        generation: next_generation(config).map_err(|_| SaveFailure::AtomicWrite)?,
        last_transaction_id: None,
        roots: root_states,
        records: encoded,
        signature: digest_hex(&signature),
    };
    let bytes = encode_v2(payload.clone()).map_err(|_| SaveFailure::RecordValidation)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(SaveFailure::SizeLimit);
    }
    #[cfg(test)]
    snapshot_test_trace("save_before_preserve");
    preserve_current_generation(&path)?;
    #[cfg(test)]
    snapshot_test_trace("save_before_atomic_write");
    if crate::persistence::atomic_write_private_limited(&path, &bytes, ERROR, MAX_BYTES as usize).is_err() {
        let landed = read_bytes(&path).ok().flatten().and_then(|saved| decode_v2(&saved).ok())
            .is_some_and(|saved| saved.generation == payload.generation);
        if !landed { return Err(SaveFailure::AtomicWrite); }
    }
    #[cfg(test)]
    snapshot_test_trace("save_after_atomic_write");
    let verified = read_bytes(&path).map_err(|_| SaveFailure::AtomicWrite)?.ok_or(SaveFailure::AtomicWrite)?;
    let verified = decode_v2(&verified).map_err(|_| SaveFailure::AtomicWrite)?;
    if verified.generation != payload.generation { return Err(SaveFailure::AtomicWrite); }
    #[cfg(test)]
    snapshot_test_trace("save_verified");
    Ok(verified.generation)
}

fn validate_stored(bytes: &[u8]) -> Option<SnapshotFormatV2> {
    match detect_format_v2(bytes).ok()? {
        SnapshotFormatV2::CurrentV2 => decode_v2(bytes).ok().map(|_| SnapshotFormatV2::CurrentV2),
        SnapshotFormatV2::LegacyV1 => {
            let snapshot: Snapshot = serde_json::from_slice(bytes).ok()?;
            let payload = serde_json::to_vec(&snapshot.payload).ok()?;
            if snapshot.version != VERSION || snapshot.checksum != digest_hex(&Sha256::digest(payload))
                || snapshot.payload.roots.is_empty() || snapshot.payload.roots.len() > 32
                || snapshot.payload.records.len() > MAX_ALBUMS || parse_digest(&snapshot.payload.signature).is_err()
            { return None; }
            match snapshot.payload.qualification {
                SnapshotQualification::Clean => {}
                SnapshotQualification::Provisional { warning_counts }
                    if valid_warning_counts(&warning_counts) && warning_counts.metadata_only() && warning_counts.metadata_decode > 0 => {}
                _ => return None,
            }
            let mut roots = BTreeSet::new();
            let mut root_map = std::collections::BTreeMap::new();
            for root in &snapshot.payload.roots {
                if root.is_empty() || root.len() > MAX_TEXT || !roots.insert(root.clone()) { return None; }
                root_map.insert(root.clone(), PathBuf::from(root));
            }
            let mut ids = HashSet::new(); let mut tracks = 0;
            for dto in snapshot.payload.records {
                decode_record(dto, &root_map, &mut ids, &mut tracks).ok()?;
            }
            Some(SnapshotFormatV2::LegacyV1)
        }
        SnapshotFormatV2::Unsupported => None,
    }
}

fn next_generation(config: &Path) -> Result<u64> {
    let active = path(config)?;
    let previous = active.with_file_name("catalog-v1.previous.json");
    let max = [active, previous].into_iter().filter_map(|p| read_bytes(&p).ok().flatten())
        .filter_map(|bytes| decode_v2(&bytes).ok().map(|p| p.generation)).max().unwrap_or(0);
    max.checked_add(1).ok_or(ERROR)
}

fn preserve_current_generation(active: &Path) -> std::result::Result<(), SaveFailure> {
    let current = read_bytes(active).map_err(|_| SaveFailure::AtomicWrite)?;
    let previous = active.with_file_name("catalog-v1.previous.json");
    match current {
        Some(bytes) if validate_stored(&bytes).is_some() => {
            crate::persistence::atomic_write_private_limited(&previous, &bytes, ERROR, MAX_BYTES as usize).map_err(|_| SaveFailure::AtomicWrite)?;
        }
        Some(_) => {
            if !read_bytes(&previous).ok().flatten().is_some_and(|b| validate_stored(&b).is_some()) {
                return Err(SaveFailure::AtomicWrite);
            }
        }
        None => {
            if read_bytes(&previous).ok().flatten().is_some_and(|b| validate_stored(&b).is_none()) {
                return Err(SaveFailure::AtomicWrite);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, Instant},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0u8; 12];
            getrandom::fill(&mut random).unwrap();
            let path =
                std::env::temp_dir().join(format!("sonante-windows-catalog-snapshot-{random:x?}"));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            snapshot_test_trace("fixture_cleanup_begin");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match fs::remove_dir_all(&self.0) {
                    Ok(()) => { snapshot_test_trace("fixture_cleanup_done"); return; },
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => { snapshot_test_trace("fixture_cleanup_missing"); return; },
                    Err(error) => {
                        #[cfg(windows)]
                        snapshot_test_trace(if matches!(error.raw_os_error(), Some(32) | Some(145)) { "fixture_cleanup_retry" } else { "fixture_cleanup_error" });
                        #[cfg(windows)]
                        let transient = matches!(error.raw_os_error(), Some(32) | Some(145));
                        #[cfg(not(windows))]
                        let transient = error.kind() == std::io::ErrorKind::DirectoryNotEmpty;
                        if transient && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        if std::thread::panicking() {
                            eprintln!("snapshot test fixture cleanup failed: {error}");
                        } else {
                            panic!("snapshot test fixture cleanup failed: {error}");
                        }
                        return;
                    }
                }
            }
        }
    }

    struct TestLibrary(Library);
    impl TestLibrary {
        fn new(config: PathBuf) -> Self {
            Self(Library::new(config))
        }
    }
    impl std::ops::Deref for TestLibrary {
        type Target = Library;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl Drop for TestLibrary {
        fn drop(&mut self) {
            snapshot_test_trace("test_library_drop_stop_begin");
            self.0.stop();
            snapshot_test_trace("test_library_drop_stop_done");
        }
    }

    fn wave(path: &Path) {
        let mut body = b"WAVEfmt ".to_vec();
        body.extend(16u32.to_le_bytes());
        body.extend([1, 0, 2, 0]);
        body.extend(44100u32.to_le_bytes());
        body.extend(176400u32.to_le_bytes());
        body.extend([4, 0, 16, 0]);
        body.extend(b"data\0\0\0\0");
        let mut info = b"INFO".to_vec();
        for (name, value) in [
            (b"INAM", "Track"),
            (b"IPRD", "Snapshot Album"),
            (b"IART", "Artist"),
        ] {
            let mut value = value.as_bytes().to_vec();
            value.push(0);
            info.extend(name);
            info.extend((value.len() as u32).to_le_bytes());
            info.extend(&value);
            if value.len() % 2 != 0 {
                info.push(0);
            }
        }
        body.extend(b"LIST");
        body.extend((info.len() as u32).to_le_bytes());
        body.extend(info);
        let mut bytes = b"RIFF".to_vec();
        bytes.extend((body.len() as u32).to_le_bytes());
        bytes.extend(body);
        fs::write(path, bytes).unwrap();
    }
    fn indexed(fixture: &Fixture) -> (TestLibrary, PathBuf, PathBuf) {
        snapshot_test_trace("indexed_before_library_new");
        let root = fixture.0.join("Music");
        fs::create_dir_all(&root).unwrap();
        wave(&root.join("track.wav"));
        let config = fixture.0.join("profile/library-v3/folders.json");
        let library = TestLibrary::new(config.clone());
        snapshot_test_trace("indexed_after_library_new");
        library.select(root.clone()).unwrap();
        snapshot_test_trace("indexed_after_select");
        let deadline = Instant::now() + Duration::from_secs(5);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline, "fixture scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(library.status().unwrap().phase, "ready");
        snapshot_test_trace("indexed_ready");
        assert!(library.data.lock().unwrap().signature.is_some());
        assert!(
            path(&config).unwrap().is_file(),
            "ready was published before the catalog snapshot was written"
        );
        let admitted_root = library.data.lock().unwrap().roots[0].clone();
        snapshot_test_trace("indexed_done");
        (library, config, admitted_root)
    }
    fn record_v2(record: SnapshotRecord, root_id: String) -> SnapshotRecordV2 {
        SnapshotRecordV2 { root_id, album: record.album, folder: record.folder, cover: record.cover,
            search: record.search, tracks: record.tracks, durations: record.durations }
    }
    fn legacy_bytes(roots: &[PathBuf], records: &HashMap<String, Record>, signature: [u8; 32]) -> Vec<u8> {
        let mut roots = roots.iter().map(|root| root_key(root).unwrap()).collect::<Vec<_>>();
        roots.sort();
        let mut records = records.values().map(|record| encode_record(record).unwrap()).collect::<Vec<_>>();
        records.sort_by(|a,b| a.album.id.cmp(&b.album.id));
        let payload = SnapshotPayload { roots, signature: digest_hex(&signature), records,
            qualification: SnapshotQualification::Clean };
        let checksum = digest_hex(&Sha256::digest(serde_json::to_vec(&payload).unwrap()));
        serde_json::to_vec(&Snapshot { version: VERSION, checksum, payload }).unwrap()
    }
    fn wait_scan(library: &Library) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline, "fixture scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn v2_contract_round_trips_with_root_provenance_and_per_root_quality() {
        let fixture = Fixture::new();
        let (library, _config, root) = indexed(&fixture);
        let state = library.data.lock().unwrap();
        let record = state.albums.values().next().unwrap();
        let root_path = root_key(&root).unwrap();
        let root_id = id(&root_path);
        let payload = SnapshotV2Payload {
            generation: 1,
            last_transaction_id: Some("initial-transaction".into()),
            roots: vec![RootStateV2 {
                root_id: root_id.clone(),
                path: root_path,
                quality: RootQualityV2::Confirmed,
                warning_counts: Default::default(),
            }],
            records: vec![record_v2(encode_record(record).unwrap(), root_id)],
            signature: digest_hex(&[7; 32]),
        };
        drop(state);
        let bytes = encode_v2(payload).unwrap();
        let decoded = decode_v2(&bytes).unwrap();
        assert_eq!(decoded.generation, 1);
        assert_eq!(decoded.roots.len(), 1);
        assert_eq!(decoded.records.len(), 1);
        assert_eq!(decoded.records[0].root_id, decoded.roots[0].root_id);
        library.stop();
    }

    #[test]
    fn v2_rejects_checksum_tampering_wrong_provenance_and_structural_warnings() {
        let fixture = Fixture::new();
        let (library, _config, root) = indexed(&fixture);
        let state = library.data.lock().unwrap();
        let record = encode_record(state.albums.values().next().unwrap()).unwrap();
        drop(state);
        let root_path = root_key(&root).unwrap();
        let root_id = id(&root_path);
        let payload = SnapshotV2Payload {
            generation: 1,
            last_transaction_id: None,
            roots: vec![RootStateV2 {
                root_id: root_id.clone(),
                path: root_path,
                quality: RootQualityV2::Confirmed,
                warning_counts: Default::default(),
            }],
            records: vec![record_v2(record, root_id)],
            signature: digest_hex(&[8; 32]),
        };
        let mut bytes = encode_v2(payload.clone()).unwrap();
        let corrupt_at = bytes.len() / 2;
        bytes[corrupt_at] ^= 1;
        assert!(decode_v2(&bytes).is_err());

        let mut wrong_owner = payload.clone();
        wrong_owner.records[0].root_id = "different-root".into();
        assert!(validate_v2(&wrong_owner).is_err());

        let mut structural = payload;
        structural.roots[0].quality = RootQualityV2::Provisional;
        structural.roots[0].warning_counts.directory_read = 1;
        assert!(validate_v2(&structural).is_err());
        library.stop();
    }

    #[test]
    fn v1_is_identified_separately_and_root_intent_recovery_is_deterministic() {
        assert_eq!(detect_format_v2(br#"{"version":1}"#).unwrap(), SnapshotFormatV2::LegacyV1);
        assert_eq!(detect_format_v2(br#"{"version":2}"#).unwrap(), SnapshotFormatV2::CurrentV2);
        assert_eq!(detect_format_v2(br#"{"version":9}"#).unwrap(), SnapshotFormatV2::Unsupported);

        let intent = RootTransactionV2 {
            transaction_id: "tx-1".into(), kind: RootOperationKindV2::Add,
            root_id: "root-1".into(), root_path: "C:\\Music".into(), base_generation: 4,
        };
        let base = SnapshotV2Payload { generation: 4, last_transaction_id: None,
            roots: Vec::new(), records: Vec::new(), signature: digest_hex(&[0; 32]) };
        assert_eq!(recovery_decision_v2(Some(&base), None, Some(&intent)), RecoveryDecisionV2::Pending);
        let committed = SnapshotV2Payload { generation: 5,
            last_transaction_id: Some("tx-1".into()), ..base.clone() };
        assert_eq!(recovery_decision_v2(Some(&committed), Some(&base), Some(&intent)), RecoveryDecisionV2::FinalizeIntent);
        assert_eq!(recovery_decision_v2(None, Some(&base), None), RecoveryDecisionV2::UsePrevious);
        assert_eq!(classify_root_v2(true, false, Some(&intent), "root-1", true), RootRecoveryV2::Candidate);
        assert_eq!(classify_root_v2(true, false, Some(&intent), "root-1", false), RootRecoveryV2::PendingAdd);
    }

    #[test]
    fn v1_is_preserved_as_backup_until_a_valid_v2_rebuild_is_committed() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        let (records, signature) = {
            let state = library.data.lock().unwrap();
            (state.albums.clone(), state.signature.unwrap())
        };
        let legacy = legacy_bytes(&[root.clone()], &records, signature);
        library.stop();
        let active = path(&config).unwrap();
        fs::write(&active, &legacy).unwrap();
        assert!(matches!(load(&config, &[root.clone()]).unwrap(), LoadResult::LegacyV1));

        let migrated = TestLibrary::new(config.clone());
        wait_scan(&migrated);
        assert_eq!(migrated.status().unwrap().phase, "ready");
        let current = read_bytes(&active).unwrap().unwrap();
        assert_eq!(decode_v2(&current).unwrap().generation, 1);
        assert_eq!(fs::read(active.with_file_name("catalog-v1.previous.json")).unwrap(), legacy);
        let settings: super::super::Saved = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        assert!(!settings.catalog_rebuild_attempted);
        migrated.stop();
    }

    #[test]
    fn failed_legacy_rebuild_is_not_repeated_until_explicit_refresh() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        let (records, signature) = {
            let state = library.data.lock().unwrap();
            (state.albums.clone(), state.signature.unwrap())
        };
        let legacy = legacy_bytes(&[root.clone()], &records, signature);
        library.stop();
        let active = path(&config).unwrap();
        fs::write(&active, &legacy).unwrap();
        let offline = fixture.0.join("offline-Music");
        fs::rename(&root, &offline).unwrap();

        let failed = TestLibrary::new(config.clone());
        assert_eq!(failed.status().unwrap().phase, "failed");
        assert!(serde_json::from_slice::<super::super::Saved>(&fs::read(&config).unwrap()).unwrap().catalog_rebuild_attempted);
        assert_eq!(fs::read(&active).unwrap(), legacy);
        failed.stop();

        fs::rename(&offline, &root).unwrap();
        let reopened = TestLibrary::new(config.clone());
        assert_eq!(reopened.status().unwrap().phase, "failed");
        assert_eq!(reopened.status().unwrap().error, Some("windows_catalog_rebuild_requires_manual_retry"));
        assert_eq!(fs::read(&active).unwrap(), legacy);
        reopened.refresh().unwrap();
        wait_scan(&reopened);
        assert_eq!(reopened.status().unwrap().phase, "ready");
        assert_eq!(decode_v2(&fs::read(&active).unwrap()).unwrap().generation, 1);
        assert!(!serde_json::from_slice::<super::super::Saved>(&fs::read(&config).unwrap()).unwrap().catalog_rebuild_attempted);
        reopened.stop();
    }

    #[test]
    fn invalid_active_checksum_and_truncation_recover_only_from_valid_previous_generation() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        let active = path(&config).unwrap();
        let original_generation = decode_v2(&fs::read(&active).unwrap()).unwrap().generation;
        wave(&root.join("second.wav"));
        assert!(library.check_changes(None, None).unwrap());
        let current = decode_v2(&fs::read(&active).unwrap()).unwrap();
        assert!(current.generation > original_generation);
        let backup = active.with_file_name("catalog-v1.previous.json");
        assert_eq!(decode_v2(&fs::read(&backup).unwrap()).unwrap().generation, original_generation);
        let backup_bytes = fs::read(&backup).unwrap();
        library.stop();

        let current_bytes = fs::read(&active).unwrap();
        let mut corrupt: SnapshotV2Envelope = serde_json::from_slice(&current_bytes).unwrap();
        corrupt.checksum = "0".repeat(64);
        fs::write(&active, serde_json::to_vec(&corrupt).unwrap()).unwrap();
        let LoadResult::Restored(recovered) = load(&config, &[root.clone()]).unwrap() else { panic!("valid previous generation was not selected") };
        assert_eq!(recovered.generation, original_generation);
        assert_eq!(recovered.records.values().map(|record| record.tracks.len()).sum::<usize>(), 1);

        fs::write(&active, b"{\"version\":2").unwrap();
        let LoadResult::Restored(recovered) = load(&config, &[root]).unwrap() else { panic!("truncated active snapshot did not recover") };
        assert_eq!(recovered.generation, original_generation);
        assert_eq!(fs::read(&backup).unwrap(), backup_bytes);
    }

    #[test]
    fn configuration_generation_selects_only_a_matching_valid_generation() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        let first = decode_v2(&fs::read(path(&config).unwrap()).unwrap()).unwrap().generation;
        library.refresh().unwrap();
        wait_scan(&library);
        let active = decode_v2(&fs::read(path(&config).unwrap()).unwrap()).unwrap().generation;
        assert!(active > first);
        let LoadResult::Restored(previous) = load_for_generation(&config, &[root.clone()], Some(first)).unwrap() else {
            panic!("saved configuration generation was not recovered from the prior generation")
        };
        assert_eq!(previous.generation, first);
        assert!(matches!(load_for_generation(&config, &[root], Some(active + 10)).unwrap(), LoadResult::Incompatible));
        library.stop();
    }

    #[test]
    fn root_representation_diagnostic_is_lexical_only_and_does_not_claim_other_aliases_equal() {
        assert_eq!(root_representation_key(r"\\NAS\Music\"), root_representation_key(r"\\nas/music"));
        assert_ne!(root_representation_key(r"\\NAS\Music"), root_representation_key(r"\\other\Music"));
    }

    #[test]
    fn equivalent_root_spelling_is_diagnosed_but_not_admitted_as_the_saved_identity() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        library.stop();
        let alternate = PathBuf::from(
            root.to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase(),
        );
        assert_ne!(root, alternate);
        assert!(matches!(load(&config, &[alternate]).unwrap(), LoadResult::Incompatible));
    }

    #[test]
    fn restart_restores_provisional_catalog_without_startup_scan_then_explicit_check_confirms_it() {
        let fixture = Fixture::new();
        let (library, config, _) = indexed(&fixture);
        let snapshot_path = path(&config).unwrap();
        assert!(snapshot_path.is_file());
        let saved = decode_v2(&fs::read(&snapshot_path).unwrap()).unwrap();
        assert_eq!(saved.roots[0].quality, RootQualityV2::Confirmed);
        library.stop();

        let reopened = TestLibrary::new(config);
        let status = reopened.status().unwrap();
        assert_eq!(status.phase, "cached");
        assert_eq!(status.albums, 1);
        assert_eq!(status.tracks, 1);
        let state = reopened.data.lock().unwrap();
        assert!(state.signature.is_none());
        assert!(state.cached_signature.is_some());
        drop(state);
        let albums = reopened.albums_ordered("", 0, 10, "artist").unwrap();
        assert_eq!(albums.total, 1);
        assert_eq!(reopened.tracks(&albums.items[0].id).unwrap().len(), 1);
        assert_eq!(
            reopened.check_album_snapshot("stale-token"),
            Err("library_changed")
        );
        assert!(!reopened.check_changes(None, None).unwrap());
        assert_eq!(reopened.status().unwrap().phase, "ready");
        assert!(reopened.data.lock().unwrap().signature.is_some());
        reopened.stop();
    }

    #[test]
    fn unrelated_configuration_save_does_not_reindex_a_restored_catalog() {
        let fixture = Fixture::new();
        let (library, config, _) = indexed(&fixture);
        library.stop();

        let reopened = TestLibrary::new(config);
        let before = reopened.status().unwrap();
        assert_eq!(before.phase, "cached");
        let after = reopened.configure(None, Sources::default()).unwrap();
        assert_eq!(after.phase, "cached");
        assert_eq!(after.epoch, before.epoch);
        assert_eq!(after.tracks, before.tracks);
        reopened.stop();
    }

    #[test]
    fn metadata_only_warning_writes_a_provisional_snapshot_and_restores_without_scanning() {
        let fixture = Fixture::new();
        let root = fixture.0.join("Music");
        fs::create_dir_all(&root).unwrap();
        wave(&root.join("readable.wav"));
        fs::write(root.join("unreadable.flac"), b"not a FLAC file").unwrap();
        let config = fixture.0.join("profile/library-v3/folders.json");
        let library = TestLibrary::new(config.clone());
        library.select(root).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline, "fixture scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        let status = library.status().unwrap();
        assert_eq!(status.error, Some("windows_library_check_unconfirmed"));
        assert_eq!(status.warnings, 1);
        assert_eq!(status.metadata_warnings, 1);
        let snapshot_path = path(&config).unwrap();
        assert!(snapshot_path.is_file());
        let saved = decode_v2(&fs::read(&snapshot_path).unwrap()).unwrap();
        assert_eq!(saved.roots[0].quality, RootQualityV2::Provisional);
        assert_eq!(saved.roots[0].warning_counts.metadata_decode, 1);
        assert_eq!(saved.roots[0].warning_counts.structural_total(), 0);
        library.stop();

        let reopened = TestLibrary::new(config.clone());
        let restored = reopened.status().unwrap();
        assert_eq!(restored.phase, "cached");
        assert_eq!(restored.warnings, 1);
        assert_eq!(restored.metadata_warnings, 1);
        assert_eq!(restored.error, Some("windows_library_check_unconfirmed"));
        assert_eq!(restored.tracks, status.tracks);
        let saved_before_check = fs::read(&snapshot_path).unwrap();
        assert!(reopened.check_changes(None, None).is_err());
        assert_eq!(reopened.status().unwrap().phase, "cached");
        assert_eq!(fs::read(&snapshot_path).unwrap(), saved_before_check);
        reopened.stop();
    }

    #[test]
    fn three_roots_with_only_metadata_warnings_restore_as_cache_without_startup_scan() {
        let fixture = Fixture::new();
        let roots = ["One", "Two", "Three"].map(|name| {
            let root = fixture.0.join(name);
            fs::create_dir_all(&root).unwrap();
            root
        });
        wave(&roots[0].join("one.wav"));
        wave(&roots[1].join("two.wav"));
        fs::write(roots[2].join("bad.flac"), b"not a FLAC file").unwrap();
        let config = fixture.0.join("profile/library-v3/folders.json");
        let library = TestLibrary::new(config.clone());
        library.select_roots(roots.to_vec()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline, "fixture scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        let completed = library.status().unwrap();
        assert_eq!(completed.phase, "ready");
        assert_eq!(completed.error, Some("windows_library_check_unconfirmed"));
        assert_eq!(completed.metadata_warnings, 1);
        assert!(path(&config).unwrap().is_file());
        library.stop();

        let reopened = TestLibrary::new(config);
        let restored = reopened.status().unwrap();
        assert_eq!(restored.phase, "cached");
        assert_eq!(restored.tracks, completed.tracks);
        assert_eq!(restored.metadata_warnings, 1);
        assert_eq!(reopened.data.lock().unwrap().roots.len(), 3);
        reopened.stop();
    }

    #[test]
    fn structurally_unsafe_warning_snapshot_is_rejected_and_not_rewritten() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        library.stop();
        let snapshot_path = path(&config).unwrap();
        let mut invalid = decode_v2(&fs::read(&snapshot_path).unwrap()).unwrap();
        invalid.roots[0].quality = RootQualityV2::Provisional;
        invalid.roots[0].warning_counts = crate::local_diagnostics::LibraryWarningCounts {
            directory_read: 1, metadata_decode: 1, ..Default::default()
        };
        let LoadResult::Restored(cached) = load(&config, &[root.clone()]).unwrap() else { panic!("snapshot restore failed") };
        assert!(matches!(save(
            &config,
            &[root.clone()],
            cached.signature,
            &cached.records,
            crate::local_diagnostics::LibraryWarningCounts { directory_read: 1, ..Default::default() },
        ), Err(SaveFailure::RecordValidation)));
        let payload_bytes = serde_json::to_vec(&invalid).unwrap();
        let envelope = SnapshotV2Envelope { version: CONTRACT_VERSION,
            checksum: digest_hex(&Sha256::digest(payload_bytes)), payload: invalid };
        fs::write(&snapshot_path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        let bytes = fs::read(&snapshot_path).unwrap();
        assert!(matches!(load(&config, &[root]).unwrap(), LoadResult::Incompatible));
        assert_eq!(fs::read(snapshot_path).unwrap(), bytes);
    }

    #[test]
    fn snapshot_write_failure_keeps_the_previous_valid_snapshot_bytes() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        library.stop();
        let snapshot_path = path(&config).unwrap();
        let original = fs::read(&snapshot_path).unwrap();
        let lock_path = snapshot_path.with_extension("lock");
        match fs::remove_file(&lock_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("fixture lock file could not be prepared: {error}"),
        }
        fs::create_dir(&lock_path).unwrap();
        let LoadResult::Restored(cached) = load(&config, &[root.clone()]).unwrap() else { panic!("snapshot restore failed") };
        assert!(matches!(save(&config, &[root], cached.signature, &cached.records, Default::default()), Err(SaveFailure::Lock)));
        assert_eq!(fs::read(snapshot_path).unwrap(), original);
    }

    #[test]
    fn partial_scan_cannot_replace_a_previous_snapshot() {
        let fixture = Fixture::new();
        let (library, config, _) = indexed(&fixture);
        let snapshot_path = path(&config).unwrap();
        let original = fs::read(&snapshot_path).unwrap();
        let signature = library.data.lock().unwrap().signature;
        library.persist_snapshot_after_scan(
            signature,
            crate::local_diagnostics::LibraryWarningCounts::default(),
            &BTreeMap::new(),
            false,
            true,
        );
        assert_eq!(fs::read(snapshot_path).unwrap(), original);
        library.stop();
    }

    #[test]
    fn removing_a_root_replaces_the_snapshot_with_the_new_root_set() {
        let fixture = Fixture::new();
        let first = fixture.0.join("One");
        let second = fixture.0.join("Two");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        wave(&first.join("one.wav"));
        wave(&second.join("two.wav"));
        let config = fixture.0.join("profile/library-v3/folders.json");
        let library = TestLibrary::new(config.clone());
        library.select_roots(vec![first.clone(), second]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline, "fixture scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(path(&config).unwrap().is_file());
        library.select_roots(vec![first.clone()]).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while library.status().unwrap().phase == "scanning" {
            assert!(Instant::now() < deadline, "fixture scan timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(library.status().unwrap().phase, "ready");
        let admitted_first = first.canonicalize().unwrap();
        assert_eq!(library.data.lock().unwrap().roots, vec![admitted_first.clone()]);
        library.stop();
        let reopened = TestLibrary::new(config);
        assert_eq!(reopened.status().unwrap().phase, "cached");
        assert_eq!(reopened.data.lock().unwrap().roots, vec![admitted_first]);
        reopened.stop();
    }

    #[test]
    fn unavailable_root_keeps_cached_catalog_and_snapshot_bytes_after_explicit_check_fails() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        let snapshot_path = path(&config).unwrap();
        let original = fs::read(&snapshot_path).unwrap();
        library.stop();
        fs::rename(&root, fixture.0.join("offline-Music")).unwrap();

        let reopened = TestLibrary::new(config);
        assert_eq!(reopened.status().unwrap().phase, "cached");
        assert_eq!(
            reopened.albums_ordered("", 0, 10, "artist").unwrap().total,
            1
        );
        assert!(reopened.check_changes(None, None).is_err());
        assert_eq!(reopened.status().unwrap().phase, "cached");
        assert_eq!(
            reopened.albums_ordered("", 0, 10, "artist").unwrap().total,
            1
        );
        assert_eq!(fs::read(snapshot_path).unwrap(), original);
        reopened.stop();
    }

    #[test]
    fn incompatible_roots_version_and_unsafe_relative_paths_are_not_loaded_or_rewritten() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        library.stop();
        let snapshot_path = path(&config).unwrap();
        let original = fs::read(&snapshot_path).unwrap();
        assert!(matches!(load(&config, &[fixture.0.join("different-root")]).unwrap(), LoadResult::Incompatible));

        let mut value: SnapshotV2Envelope = serde_json::from_slice(&original).unwrap();
        value.version = 99;
        fs::write(&snapshot_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let incompatible = fs::read(&snapshot_path).unwrap();
        assert!(matches!(load(&config, &[root.clone()]).unwrap(), LoadResult::Incompatible));
        assert_eq!(fs::read(&snapshot_path).unwrap(), incompatible);

        let mut value: SnapshotV2Envelope = serde_json::from_slice(&original).unwrap();
        value.payload.records[0].tracks[0].path = "../outside.wav".into();
        let payload_bytes = serde_json::to_vec(&value.payload).unwrap();
        value.checksum = digest_hex(&Sha256::digest(&payload_bytes));
        fs::write(&snapshot_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let unsafe_snapshot = fs::read(&snapshot_path).unwrap();
        assert!(matches!(load(&config, &[root]).unwrap(), LoadResult::Incompatible));
        assert_eq!(fs::read(&snapshot_path).unwrap(), unsafe_snapshot);
    }

    #[test]
    fn restored_cache_reconciles_added_and_removed_files_and_rewrites_only_after_success() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        library.stop();

        let cached = TestLibrary::new(config.clone());
        assert_eq!(cached.status().unwrap().phase, "cached");
        assert_eq!(cached.status().unwrap().tracks, 1);

        let added = root.join("second.wav");
        wave(&added);
        assert!(cached.check_changes(None, None).unwrap());
        assert_eq!(cached.status().unwrap().phase, "ready");
        assert_eq!(cached.status().unwrap().tracks, 2);
        let after_add = fs::read(path(&config).unwrap()).unwrap();

        cached.stop();
        let reopened = TestLibrary::new(config.clone());
        assert_eq!(reopened.status().unwrap().phase, "cached");
        assert_eq!(reopened.status().unwrap().tracks, 2);

        fs::remove_file(&added).unwrap();
        assert!(reopened.check_changes(None, None).unwrap());
        assert_eq!(reopened.status().unwrap().phase, "ready");
        assert_eq!(reopened.status().unwrap().tracks, 1);
        let after_remove = fs::read(path(&config).unwrap()).unwrap();
        assert_ne!(after_add, after_remove);

        reopened.stop();
        let final_restore = TestLibrary::new(config);
        assert_eq!(final_restore.status().unwrap().phase, "cached");
        assert_eq!(final_restore.status().unwrap().tracks, 1);
        final_restore.stop();
    }

    #[test]
    fn interrupted_cached_verification_keeps_the_last_snapshot_and_catalog() {
        let fixture = Fixture::new();
        let (library, config, _) = indexed(&fixture);
        library.stop();

        let snapshot_path = path(&config).unwrap();
        let original = fs::read(&snapshot_path).unwrap();
        let reopened = TestLibrary::new(config.clone());
        assert_eq!(reopened.status().unwrap().phase, "cached");
        let epoch = reopened.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        {
            let mut state = reopened.data.lock().unwrap();
            state.status.epoch = epoch;
            state.status.phase = "scanning".into();
            state.status.error = None;
            state.signature = None;
        }

        // Exercise the same retirement callback used when the bounded scan
        // scope expires; it must leave the cached catalog eligible for restart.
        reopened.scan_failed(epoch, "index_timeout");
        let status = reopened.status().unwrap();
        assert_eq!(status.phase, "cached");
        assert_eq!(status.error, Some("index_timeout"));
        assert_eq!(status.tracks, 1);
        assert_eq!(fs::read(&snapshot_path).unwrap(), original);
        reopened.stop();

        let after_interruption = TestLibrary::new(config);
        assert_eq!(after_interruption.status().unwrap().phase, "cached");
        assert_eq!(after_interruption.status().unwrap().tracks, 1);
        after_interruption.stop();
    }

    #[test]
    fn corrupt_snapshot_is_rejected_without_deleting_or_rewriting_it() {
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        library.stop();
        let snapshot_path = path(&config).unwrap();
        fs::write(&snapshot_path, b"{ malformed").unwrap();
        let corrupt = fs::read(&snapshot_path).unwrap();
        assert!(matches!(load(&config, &[root.clone()]).unwrap(), LoadResult::Incompatible));
        assert_eq!(fs::read(snapshot_path).unwrap(), corrupt);
        let reopened = TestLibrary::new(config);
        assert_eq!(reopened.status().unwrap().phase, "failed");
        assert_eq!(reopened.status().unwrap().error, Some("windows_catalog_recovery_required"));
        assert_eq!(reopened.status().unwrap().epoch, 0);
        reopened.stop();
    }

    #[test]
    fn checksum_failure_without_a_valid_backup_is_not_promoted_or_rewritten() {
        snapshot_test_trace("checksum_test_begin");
        let fixture = Fixture::new();
        let (library, config, root) = indexed(&fixture);
        snapshot_test_trace("checksum_test_indexed");
        library.stop();
        snapshot_test_trace("checksum_test_stopped");
        let active = path(&config).unwrap();
        let mut invalid: SnapshotV2Envelope = serde_json::from_slice(&fs::read(&active).unwrap()).unwrap();
        invalid.checksum = "f".repeat(64);
        fs::write(&active, serde_json::to_vec(&invalid).unwrap()).unwrap();
        snapshot_test_trace("checksum_test_corruption_written");
        let bytes = fs::read(&active).unwrap();
        snapshot_test_trace("checksum_test_before_load");
        assert!(matches!(load(&config, &[root]).unwrap(), LoadResult::Incompatible));
        snapshot_test_trace("checksum_test_after_load");
        assert_eq!(fs::read(&active).unwrap(), bytes);
        snapshot_test_trace("checksum_test_bytes_preserved");
        assert!(!active.with_file_name("catalog-v1.previous.json").exists());
        snapshot_test_trace("checksum_test_done");
    }

    #[test]
    fn missing_expected_generation_is_recovery_required_not_a_rebuild_signal() {
        let fixture = Fixture::new();
        let config = fixture.0.join("profile/library-v3/folders.json");
        let root = fixture.0.join("Music");
        assert!(matches!(load_for_generation(&config, &[root], Some(7)).unwrap(), LoadResult::Incompatible));
    }
}
