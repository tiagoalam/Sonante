//! Experimental, standalone local-file catalog with incremental metadata extraction.
use rusqlite::{
    params_from_iter,
    types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef},
    Connection, OpenFlags, Transaction, TransactionBehavior,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{BufReader, Read},
    path::{Component, Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

mod metadata;
mod path_codec;
mod scanner;
mod cue;
pub mod collections;

pub use metadata::{AttemptStatus as MetadataStatus, Values as TrackMetadataValues};

const SCHEMA_V2_VERSION: i64 = 2;
const SCHEMA_V3_VERSION: i64 = 3;
const SCHEMA_V4_VERSION: i64 = 4;
const SCHEMA_V5_VERSION: i64 = 5;
const SCHEMA_V6_VERSION: i64 = 6;
const SCHEMA_V7_VERSION: i64 = 7;
const SCHEMA_V8_VERSION: i64 = 8;
const SCHEMA_V9_VERSION: i64 = 9;
const CURRENT_SCHEMA_VERSION: i64 = 10;
const BASE_SCHEMA_SQL: &str = "CREATE TABLE sources(\
    id INTEGER PRIMARY KEY AUTOINCREMENT, path_format INTEGER NOT NULL CHECK(path_format IN (1,2)),\
    canonical_path BLOB NOT NULL CHECK(typeof(canonical_path)='blob'), display_name TEXT NOT NULL,\
    generation INTEGER NOT NULL CHECK(generation >= 1), last_checked_ns INTEGER NOT NULL);\
    CREATE TABLE tracks(\
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,\
    relative_path BLOB NOT NULL CHECK(typeof(relative_path)='blob' AND length(relative_path)>0),\
    size INTEGER NOT NULL CHECK(size >= 0), mtime_ns INTEGER NOT NULL, generation INTEGER NOT NULL CHECK(generation >= 1),\
    first_seen_ns INTEGER CHECK(first_seen_ns IS NULL OR first_seen_ns >= 0),\
    PRIMARY KEY(source_id, relative_path));\
    CREATE UNIQUE INDEX sources_path_identity ON sources(path_format,canonical_path);\
    CREATE INDEX tracks_source_generation ON tracks(source_id,generation);";
const TRACK_METADATA_SQL: &str = "CREATE TABLE track_metadata(\
    source_id INTEGER NOT NULL, relative_path BLOB NOT NULL,\
    attempt_status INTEGER NOT NULL CHECK(attempt_status IN (1,2,3,4)),\
    attempt_size INTEGER NOT NULL CHECK(attempt_size >= 0), attempt_mtime_ns INTEGER NOT NULL,\
    attempt_parser_version INTEGER NOT NULL CHECK(attempt_parser_version >= 1),\
    file_format TEXT NOT NULL, error TEXT,\
    success_size INTEGER, success_mtime_ns INTEGER,\
    title TEXT, track_artist TEXT, album_artist TEXT, album TEXT, genre TEXT, date_text TEXT,\
    track_number INTEGER, disc_number INTEGER, duration_ms INTEGER, sample_rate INTEGER, channels INTEGER,\
    PRIMARY KEY(source_id, relative_path),\
    FOREIGN KEY(source_id, relative_path) REFERENCES tracks(source_id, relative_path) ON DELETE CASCADE,\
    CHECK((success_size IS NULL AND success_mtime_ns IS NULL) OR\
          (success_size IS NOT NULL AND success_mtime_ns IS NOT NULL)));";
const TRACK_METADATA_ARTISTS_SQL: &str = "CREATE TABLE track_metadata_extra_artists(\
    source_id INTEGER NOT NULL, relative_path BLOB NOT NULL,\
    artist_role INTEGER NOT NULL CHECK(artist_role IN (1,2)),\
    ordinal INTEGER NOT NULL CHECK(ordinal >= 2), value TEXT NOT NULL CHECK(length(value)>0),\
    PRIMARY KEY(source_id,relative_path,artist_role,ordinal),\
    FOREIGN KEY(source_id,relative_path) REFERENCES track_metadata(source_id,relative_path) ON DELETE CASCADE);";
const SOURCE_TAG_EDIT_PENDING_SQL: &str = "CREATE TABLE source_tag_edit_pending(\
    source_id INTEGER PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,\
    generation INTEGER NOT NULL CHECK(generation >= 1),\
    started_ns INTEGER NOT NULL);";
const CUE_SCHEMA_SQL: &str = "CREATE TABLE cue_sheets(\
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,\
    relative_path BLOB NOT NULL CHECK(typeof(relative_path)='blob' AND length(relative_path)>0),\
    size INTEGER NOT NULL CHECK(size >= 0), mtime_ns INTEGER NOT NULL,\
    device BLOB NOT NULL CHECK(typeof(device)='blob' AND length(device)=8),\
    inode BLOB NOT NULL CHECK(typeof(inode)='blob' AND length(inode)=8), ctime_ns INTEGER NOT NULL,\
    parser_version INTEGER NOT NULL CHECK(parser_version >= 1),\
    status INTEGER NOT NULL CHECK(status IN (1,2)), error_code TEXT, track_count INTEGER NOT NULL CHECK(track_count >= 0),\
    PRIMARY KEY(source_id,relative_path),\
    CHECK((status=1 AND error_code IS NULL) OR (status=2 AND error_code IS NOT NULL)));\
    CREATE TABLE cue_tracks(\
    source_id INTEGER NOT NULL, cue_path BLOB NOT NULL, track_number INTEGER NOT NULL CHECK(track_number BETWEEN 1 AND 99),\
    audio_path BLOB NOT NULL CHECK(typeof(audio_path)='blob' AND length(audio_path)>0),\
    start_frame INTEGER NOT NULL CHECK(start_frame >= 0), end_frame INTEGER CHECK(end_frame IS NULL OR end_frame > start_frame),\
    title TEXT, album TEXT, genre TEXT, date_text TEXT, disc_number INTEGER CHECK(disc_number IS NULL OR disc_number > 0),\
    PRIMARY KEY(source_id,cue_path,track_number),\
    FOREIGN KEY(source_id,cue_path) REFERENCES cue_sheets(source_id,relative_path) ON DELETE CASCADE,\
    FOREIGN KEY(source_id,audio_path) REFERENCES tracks(source_id,relative_path) ON DELETE CASCADE);\
    CREATE INDEX cue_tracks_source_audio ON cue_tracks(source_id,audio_path);\
    CREATE TABLE cue_track_artists(\
    source_id INTEGER NOT NULL, cue_path BLOB NOT NULL, track_number INTEGER NOT NULL,\
    artist_role INTEGER NOT NULL CHECK(artist_role IN (1,2)), ordinal INTEGER NOT NULL CHECK(ordinal >= 1),\
    value TEXT NOT NULL CHECK(length(value)>0),\
    PRIMARY KEY(source_id,cue_path,track_number,artist_role,ordinal),\
    FOREIGN KEY(source_id,cue_path,track_number) REFERENCES cue_tracks(source_id,cue_path,track_number) ON DELETE CASCADE);";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: i64,
    pub canonical_path: PathBuf,
    pub display_name: String,
    pub generation: i64,
    pub last_checked_ns: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub source_id: i64,
    pub relative_path: PathBuf,
    pub size: i64,
    pub mtime_ns: i64,
    pub generation: i64,
    /// First catalog confirmation time. `None` means the historical import
    /// date is unknown; it is never inferred from mtime or migration time.
    pub first_seen_ns: Option<i64>,
}

/// A regular file opened through the scanner's descriptor-anchored path
/// resolver. Consumers can duplicate the already-open descriptor, but cannot
/// obtain a path and accidentally reopen a substituted entry.
pub struct VerifiedTrackFile {
    file: File,
    identity: (u64, u64, i64, i64, i64),
}

impl VerifiedTrackFile {
    pub fn try_clone_file(&self) -> std::io::Result<File> { self.file.try_clone() }
    pub fn identity_is_current(&self) -> bool { scanner::identity_matches(&self.file, self.identity) }
    pub fn same_object(&self, other: &Self) -> bool { self.identity == other.identity }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMetadata {
    pub status: MetadataStatus,
    pub file_format: String,
    pub parser_version: i64,
    pub attempted_size: i64,
    pub attempted_mtime_ns: i64,
    pub error: Option<String>,
    pub last_success_size: Option<i64>,
    pub last_success_mtime_ns: Option<i64>,
    pub last_success: Option<TrackMetadataValues>,
    /// True only when the last successful extraction matches the currently
    /// cataloged file signature. Old values remain available but are explicitly
    /// distinguishable after a changed file fails to parse.
    pub current_for_file: bool,
}

/// Persisted status of one source-owned external CUE sheet. Invalid sheets are
/// retained as diagnostics but never expose virtual tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CueSheetStatus {
    Indexed,
    Rejected { error_code: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueSheetRecord {
    pub source_id: i64,
    pub relative_path: PathBuf,
    pub size: i64,
    pub mtime_ns: i64,
    pub device: u64,
    pub inode: u64,
    pub ctime_ns: i64,
    pub status: CueSheetStatus,
    pub track_count: usize,
}

/// A CUE-defined virtual audio segment. The path fields are source-relative
/// catalog data; the application must expose an opaque source-scoped locator
/// and resolve it again immediately before queueing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueVirtualTrack {
    pub source_id: i64,
    pub cue_path: PathBuf,
    pub track_number: u32,
    pub audio_path: PathBuf,
    pub start_frame: u64,
    pub end_frame: Option<u64>,
    pub duration_ms: Option<i64>,
    pub title: Option<String>,
    pub performers: Vec<String>,
    pub album: Option<String>,
    pub album_performers: Vec<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub disc_number: Option<u32>,
    pub cue_size: i64,
    pub cue_mtime_ns: i64,
    pub cue_device: u64,
    pub cue_inode: u64,
    pub cue_ctime_ns: i64,
    pub audio_size: i64,
    pub audio_mtime_ns: i64,
    /// Null only for v7 rows migrated without filesystem identity metadata.
    /// Such legacy rows remain browseable but cannot authorize playback.
    pub audio_device: Option<u64>,
    pub audio_inode: Option<u64>,
    pub audio_ctime_ns: Option<i64>,
    pub first_seen_ns: Option<i64>,
}

/// Derived, source-scoped album identity. It is deterministic for the current
/// metadata and paths, but may change when either changes; it is not a durable
/// database identifier.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AlbumId {
    pub source_id: i64,
    pub identity: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Album {
    pub id: AlbumId,
    pub source_id: i64,
    pub title: String,
    /// Explicit ALBUMARTIST values only. Missing values are not inferred from
    /// the track artist list.
    pub album_artists: Vec<String>,
    pub track_count: usize,
    /// Earliest confirmed track identity time, only when every current track
    /// in the album has known provenance. Migrated historical rows stay unknown.
    pub first_seen_ns: Option<i64>,
    pub stale_track_count: usize,
    pub date_values: Vec<String>,
    /// True when available tags cannot confidently identify one release.
    pub ambiguous: bool,
    /// Stable machine-readable reasons explaining the derived grouping. These
    /// are diagnostic notes, not persisted claims or user-facing prose.
    pub grouping_notes: Vec<String>,
    /// Relative directory evidence used to keep same-name editions apart.
    pub directory_hint: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artist {
    pub source_id: i64,
    /// A deterministic representative spelling; other spellings differing
    /// only by surrounding whitespace or case share this conservative key.
    pub name: String,
    pub appears_as_track_artist: bool,
    pub appears_as_album_artist: bool,
    pub track_count: usize,
    pub album_count: usize,
    pub stale_track_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumTrack {
    pub track: Track,
    pub metadata: Option<TrackMetadata>,
    /// Last successful values are retained after a failed/newer attempt, but
    /// callers can avoid presenting them as current for the file.
    pub metadata_stale: bool,
}

#[cfg(test)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BrowseReadMetrics {
    sql_queries: usize,
    rows_read: usize,
    read_duration_us: u128,
}

struct BrowseBatch {
    tracks: Vec<AlbumTrack>,
    source_generation: Option<i64>,
    #[cfg(test)]
    metrics: BrowseReadMetrics,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metrics {
    pub sources_examined: usize,
    pub entries_visited: usize,
    pub files_examined: usize,
    pub files_new: usize,
    pub files_modified: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
    pub records_written: usize,
    pub files_opened_for_extraction: usize,
    pub files_processed_by_parser: usize,
    pub successful_extractions: usize,
    pub files_without_tags: usize,
    pub unsupported_formats: usize,
    pub extraction_failures: usize,
    pub parsing_duration_us: u128,
    pub metadata_records_written: usize,
    pub duration_us: u128,
}

/// Sanitized progress for a single explicit source import. Paths and file names
/// are intentionally omitted so callers may surface this to a user interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexProgress {
    pub phase: IndexPhase,
    pub entries_visited: usize,
    pub files_examined: usize,
    pub audio_files_found: usize,
    pub metadata_processed: usize,
    pub metadata_total: usize,
    pub identified_albums: usize,
    pub identified_artists: usize,
    pub counts_provisional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPhase {
    Scanning,
    ReadingMetadata,
    Grouping,
    Persisting,
    Completed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub metrics: Metrics,
    pub generation: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddReport {
    pub source_id: i64,
    pub scan: ScanReport,
}
type Result<T> = std::result::Result<T, String>;

#[derive(Debug)]
pub struct TrackFileReplaceFailure {
    pub message: String,
    pub committed: bool,
}

#[derive(Debug)]
pub struct TrackFileReplaceSuccess<T> {
    pub value: T,
    pub metadata: std::fs::Metadata,
}

#[derive(Clone, Debug)]
enum Value {
    Text(String),
    Int(i64),
    Blob(Vec<u8>),
    Null,
}

impl ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(match self {
            Self::Text(value) => rusqlite::types::Value::Text(value.clone()),
            Self::Int(value) => rusqlite::types::Value::Integer(*value),
            Self::Blob(value) => rusqlite::types::Value::Blob(value.clone()),
            Self::Null => rusqlite::types::Value::Null,
        }))
    }
}

impl FromSql for Value {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Integer(value) => Ok(Self::Int(value)),
            ValueRef::Text(value) => std::str::from_utf8(value)
                .map(|value| Self::Text(value.to_owned()))
                .map_err(|error| FromSqlError::Other(Box::new(error))),
            ValueRef::Blob(value) => Ok(Self::Blob(value.to_vec())),
            ValueRef::Null => Ok(Self::Null),
            ValueRef::Real(_) => Err(FromSqlError::InvalidType),
        }
    }
}

struct Db {
    connection: Connection,
}
impl Db {
    fn open(path: &Path) -> Result<Self> {
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )
        .map_err(|error| format!("sqlite open: {error}"))?;
        connection
            .busy_timeout(std::time::Duration::from_millis(5_000))
            .map_err(|error| format!("sqlite busy timeout: {error}"))?;
        // These two settings are connection-local. Persistent journal settings are
        // applied only after the database version and schema have been accepted.
        connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA temp_store=MEMORY;")
            .map_err(|error| format!("sqlite connection pragmas: {error}"))?;
        ensure_schema(&mut connection)?;
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(|error| format!("sqlite persistent pragmas: {error}"))?;
        Ok(Self { connection })
    }

    fn query(&self, sql: &str, args: &[Value]) -> Result<Vec<Vec<Value>>> {
        query(&self.connection, sql, args)
    }

    #[cfg(test)]
    fn close(self) -> Result<()> {
        self.connection
            .close()
            .map_err(|(_, error)| format!("sqlite close: {error}"))
    }
}

fn schema_version(connection: &Connection) -> Result<i64> {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("sqlite schema version: {error}"))
}

fn has_user_schema_objects(connection: &Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master \
             WHERE type IN ('table','index','view','trigger') AND name NOT LIKE 'sqlite_%')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("sqlite schema inspection: {error}"))
}

fn has_cue_schema_objects(connection: &Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name IN ('cue_tracks_source_audio','cue_sheets','cue_track_artists','cue_tracks'))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("sqlite CUE schema inspection: {error}"))
}

fn ensure_schema(connection: &mut Connection) -> Result<()> {
    let mut version = schema_version(connection)?;

    if version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "unsupported future catalog schema version {version}; current version is {CURRENT_SCHEMA_VERSION}"
        ));
    }

    if version > 0 && version < SCHEMA_V7_VERSION && has_cue_schema_objects(connection)? {
        return Err("catalog contains CUE schema objects before schema version 7".into());
    }

    if version == 0 {
        if has_user_schema_objects(connection)? {
            return Err(
                "unversioned legacy catalog has schema objects; explicit migration is required"
                    .into(),
            );
        }
        return initialize_current_schema(connection);
    }

    if version == 1 {
        #[cfg(target_os = "linux")]
        {
            migrate_v1_to_v2(connection)?;
            version = SCHEMA_V2_VERSION;
        }
        #[cfg(not(target_os = "linux"))]
        {
            return Err(
                "schema v1 migration is supported only on Linux; v1 has no stored path platform"
                    .into(),
            );
        }
    }

    if version == SCHEMA_V2_VERSION {
        validate_schema_v2(connection)?;
        validate_path_formats(connection)?;
        migrate_v2_to_v3(connection)?;
        version = SCHEMA_V3_VERSION;
    }

    if version == SCHEMA_V3_VERSION {
        validate_schema_v3(connection)?;
        migrate_v3_to_v4(connection)?;
        version = SCHEMA_V4_VERSION;
    }

    if version == SCHEMA_V4_VERSION {
        validate_schema_v4(connection)?;
        migrate_v4_to_v5(connection)?;
        version = SCHEMA_V5_VERSION;
    }

    if version == SCHEMA_V5_VERSION {
        validate_schema_v5(connection)?;
        migrate_v5_to_v6(connection)?;
        version = SCHEMA_V6_VERSION;
    }

    if version == SCHEMA_V6_VERSION {
        validate_schema_v6(connection)?;
        migrate_v6_to_v7(connection)?;
        version = SCHEMA_V7_VERSION;
    }

    if version == SCHEMA_V7_VERSION {
        validate_schema_v7(connection)?;
        migrate_v7_to_v8(connection)?;
        version = SCHEMA_V8_VERSION;
    }

    if version == SCHEMA_V8_VERSION {
        validate_schema_v8(connection)?;
        apply_schema_change(connection, SCHEMA_V9_VERSION, |tx| {
            tx.execute_batch(collections::SCHEMA).map_err(|e| e.to_string())?;
            collections::validate_schema(tx)
        })?;
        version = SCHEMA_V9_VERSION;
    }

    if version == SCHEMA_V9_VERSION {
        validate_schema_v8(connection)?;
        collections::validate_schema(connection)?;
        apply_schema_change(connection, CURRENT_SCHEMA_VERSION, |tx| {
            tx.execute_batch(collections::EVIDENCE_SCHEMA).map_err(|e| e.to_string())?;
            collections::validate_evidence_schema(tx)
        })?;
        version = CURRENT_SCHEMA_VERSION;
    }

    if version < CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "no catalog schema migration is defined from version {version} to {CURRENT_SCHEMA_VERSION}"
        ));
    }

    validate_schema_v8(connection)?;
    collections::validate_schema(connection)?;
    collections::validate_evidence_schema(connection)?;
    validate_path_formats(connection)
}

fn initialize_current_schema(connection: &mut Connection) -> Result<()> {
    apply_schema_change(connection, CURRENT_SCHEMA_VERSION, |transaction| {
        transaction
            .execute_batch(BASE_SCHEMA_SQL)
            .map_err(|error| format!("create base schema: {error}"))?;
        transaction
            .execute_batch(TRACK_METADATA_SQL)
            .map_err(|error| format!("create metadata schema: {error}"))?;
        transaction
            .execute_batch(TRACK_METADATA_ARTISTS_SQL)
            .map_err(|error| format!("create multi-artist schema: {error}"))
            ?;
        transaction
            .execute_batch(SOURCE_TAG_EDIT_PENDING_SQL)
            .map_err(|error| format!("create pending tag edit schema: {error}"))
            ?;
        transaction
            .execute_batch(CUE_SCHEMA_SQL)
            .map_err(|error| format!("create CUE schema: {error}"))
            ?;
        add_cue_audio_identity_columns(transaction)?;
        transaction.execute_batch(collections::SCHEMA).map_err(|e| e.to_string())?;
        collections::validate_schema(transaction)?;
        transaction.execute_batch(collections::EVIDENCE_SCHEMA).map_err(|e| e.to_string())?;
        collections::validate_evidence_schema(transaction)
    })
    .map_err(|error| format!("sqlite schema initialization: {error}"))
    .and_then(|()| { validate_schema_v8(connection)?; collections::validate_schema(connection)?; collections::validate_evidence_schema(connection) })
}

#[cfg(target_os = "linux")]
fn migrate_v1_to_v2(connection: &mut Connection) -> Result<()> {
    migrate_v1_to_v2_with_hook(connection, |_| Ok(()))
}

#[cfg(target_os = "linux")]
fn migrate_v1_to_v2_with_hook<F>(connection: &mut Connection, after_replace: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    apply_schema_change(connection, SCHEMA_V2_VERSION, |transaction| {
        validate_schema_v1(transaction)?;
        validate_v1_paths_for_migration(transaction)?;
        let old_source_sequence = query(
            transaction,
            "SELECT seq FROM sqlite_sequence WHERE name='sources'",
            &[],
        )?
        .first()
        .map(|row| int(row, 0))
        .unwrap_or(0);

        transaction
            .execute_batch(
                "CREATE TABLE sources_v2(\
                    id INTEGER PRIMARY KEY AUTOINCREMENT,\
                    path_format INTEGER NOT NULL CHECK(path_format IN (1,2)),\
                    canonical_path BLOB NOT NULL CHECK(typeof(canonical_path)='blob'),\
                    display_name TEXT NOT NULL, generation INTEGER NOT NULL CHECK(generation >= 1),\
                    last_checked_ns INTEGER NOT NULL);\
                 CREATE TABLE tracks_v2(\
                    source_id INTEGER NOT NULL REFERENCES sources_v2(id) ON DELETE CASCADE,\
                    relative_path BLOB NOT NULL CHECK(typeof(relative_path)='blob' AND length(relative_path)>0),\
                    size INTEGER NOT NULL CHECK(size >= 0), mtime_ns INTEGER NOT NULL,\
                    generation INTEGER NOT NULL CHECK(generation >= 1),\
                    PRIMARY KEY(source_id,relative_path));",
            )
            .map_err(|error| format!("create v2 migration tables: {error}"))?;

        {
            use std::os::unix::ffi::OsStrExt;
            let mut select = transaction
                .prepare("SELECT id,canonical_path,display_name,generation,last_checked_ns FROM sources ORDER BY id")
                .map_err(|error| format!("read v1 sources: {error}"))?;
            let mut rows = select
                .query([])
                .map_err(|error| format!("read v1 sources: {error}"))?;
            let mut insert = transaction
                .prepare("INSERT INTO sources_v2(id,path_format,canonical_path,display_name,generation,last_checked_ns) VALUES(?1,?2,?3,?4,?5,?6)")
                .map_err(|error| format!("prepare v2 source copy: {error}"))?;
            while let Some(row) = rows
                .next()
                .map_err(|error| format!("read v1 source row: {error}"))?
            {
                let path: String = row
                    .get(1)
                    .map_err(|error| format!("decode v1 source path: {error}"))?;
                insert
                    .execute((
                        row.get::<_, i64>(0)
                            .map_err(|error| format!("read v1 source id: {error}"))?,
                        path_codec::UNIX_BYTES_V1,
                        Path::new(&path).as_os_str().as_bytes(),
                        row.get::<_, String>(2)
                            .map_err(|error| format!("read v1 source name: {error}"))?,
                        row.get::<_, i64>(3)
                            .map_err(|error| format!("read v1 source generation: {error}"))?,
                        row.get::<_, i64>(4)
                            .map_err(|error| format!("read v1 source timestamp: {error}"))?,
                    ))
                    .map_err(|error| format!("copy v1 source: {error}"))?;
            }
        }

        {
            use std::os::unix::ffi::OsStrExt;
            let mut select = transaction
                .prepare("SELECT source_id,relative_path,size,mtime_ns,generation FROM tracks ORDER BY source_id,relative_path")
                .map_err(|error| format!("read v1 tracks: {error}"))?;
            let mut rows = select
                .query([])
                .map_err(|error| format!("read v1 tracks: {error}"))?;
            let mut insert = transaction
                .prepare("INSERT INTO tracks_v2(source_id,relative_path,size,mtime_ns,generation) VALUES(?1,?2,?3,?4,?5)")
                .map_err(|error| format!("prepare v2 track copy: {error}"))?;
            while let Some(row) = rows
                .next()
                .map_err(|error| format!("read v1 track row: {error}"))?
            {
                let relative: String = row
                    .get(1)
                    .map_err(|error| format!("decode v1 relative path: {error}"))?;
                insert
                    .execute((
                        row.get::<_, i64>(0)
                            .map_err(|error| format!("read v1 track source id: {error}"))?,
                        Path::new(&relative).as_os_str().as_bytes(),
                        row.get::<_, i64>(2)
                            .map_err(|error| format!("read v1 track size: {error}"))?,
                        row.get::<_, i64>(3)
                            .map_err(|error| format!("read v1 track mtime: {error}"))?,
                        row.get::<_, i64>(4)
                            .map_err(|error| format!("read v1 track generation: {error}"))?,
                    ))
                    .map_err(|error| format!("copy v1 track: {error}"))?;
            }
        }

        transaction
            .execute_batch(
                "DROP TABLE tracks;\
                 DROP TABLE sources;\
                 ALTER TABLE sources_v2 RENAME TO sources;\
                 ALTER TABLE tracks_v2 RENAME TO tracks;\
                 CREATE UNIQUE INDEX sources_path_identity ON sources(path_format,canonical_path);\
                 CREATE INDEX tracks_source_generation ON tracks(source_id,generation);",
            )
            .map_err(|error| format!("replace v1 tables: {error}"))?;
        if old_source_sequence > 0 {
            let updated = transaction
                .execute(
                    "UPDATE sqlite_sequence SET seq=MAX(seq,?1) WHERE name='sources'",
                    [old_source_sequence],
                )
                .map_err(|error| format!("preserve source ID sequence: {error}"))?;
            if updated == 0 {
                transaction
                    .execute(
                        "INSERT INTO sqlite_sequence(name,seq) VALUES('sources',?1)",
                        [old_source_sequence],
                    )
                    .map_err(|error| format!("restore source ID sequence: {error}"))?;
            }
        }
        after_replace(transaction)?;
        validate_schema_v2(transaction)?;
        let mut foreign_key_check = transaction
            .prepare("PRAGMA foreign_key_check")
            .map_err(|error| format!("prepare migrated foreign key check: {error}"))?;
        let mut foreign_key_rows = foreign_key_check
            .query([])
            .map_err(|error| format!("check migrated foreign keys: {error}"))?;
        if foreign_key_rows
            .next()
            .map_err(|error| format!("check migrated foreign keys: {error}"))?
            .is_some()
        {
            return Err("v1 migration produced a foreign key violation".into());
        }
        Ok(())
    })
    .map_err(|error| format!("catalog v1 to v2 migration failed: {error}"))
}

fn migrate_v2_to_v3(connection: &mut Connection) -> Result<()> {
    migrate_v2_to_v3_with_hook(connection, |_| Ok(()))
}

fn migrate_v2_to_v3_with_hook<F>(connection: &mut Connection, after_ddl: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    apply_schema_change(connection, SCHEMA_V3_VERSION, |transaction| {
        validate_schema_v2(transaction)?;
        transaction
            .execute_batch(TRACK_METADATA_SQL)
            .map_err(|error| format!("create v3 metadata table: {error}"))?;
        after_ddl(transaction)?;
        validate_schema_v3(transaction)
    })
}

#[cfg(test)]
fn migrate_v2_to_v3_failing(connection: &mut Connection) -> Result<()> {
    migrate_v2_to_v3_with_hook(connection, |_| {
        Err("injected v2-to-v3 migration failure".into())
    })
}

fn migrate_v3_to_v4(connection: &mut Connection) -> Result<()> {
    migrate_v3_to_v4_with_hook(connection, |_| Ok(()))
}

fn migrate_v3_to_v4_with_hook<F>(connection: &mut Connection, after_ddl: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    apply_schema_change(connection, SCHEMA_V4_VERSION, |transaction| {
        validate_schema_v3(transaction)?;
        transaction
            .execute_batch(TRACK_METADATA_ARTISTS_SQL)
            .map_err(|error| format!("create multi-artist metadata table: {error}"))?;
        after_ddl(transaction)?;
        validate_schema_v4(transaction)
    })
    .map_err(|error| format!("catalog v3 to v4 migration failed: {error}"))
}

fn migrate_v4_to_v5(connection: &mut Connection) -> Result<()> {
    apply_schema_change(connection, SCHEMA_V5_VERSION, |transaction| {
        validate_schema_v4(transaction)?;
        transaction
            .execute_batch(SOURCE_TAG_EDIT_PENDING_SQL)
            .map_err(|error| format!("create pending tag edit table: {error}"))?;
        validate_schema_v5(transaction)
    })
    .map_err(|error| format!("catalog v4 to v5 migration failed: {error}"))
}

fn migrate_v5_to_v6(connection: &mut Connection) -> Result<()> {
    migrate_v5_to_v6_with_hook(connection, |_| Ok(()))
}

#[cfg(test)]
fn migrate_v5_to_v6_failing(connection: &mut Connection) -> Result<()> {
    migrate_v5_to_v6_with_hook(connection, |_| Err("injected v5-to-v6 migration failure".into()))
}

fn migrate_v5_to_v6_with_hook<F>(connection: &mut Connection, after_ddl: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    apply_schema_change(connection, SCHEMA_V6_VERSION, |transaction| {
        validate_schema_v5(transaction)?;
        transaction
            .execute_batch("ALTER TABLE tracks ADD COLUMN first_seen_ns INTEGER CHECK(first_seen_ns IS NULL OR first_seen_ns >= 0)")
            .map_err(|error| format!("add first-seen timestamp: {error}"))?;
        after_ddl(transaction)?;
        validate_schema_v6(transaction)
    })
    .map_err(|error| format!("catalog v5 to v6 migration failed: {error}"))
}

fn migrate_v6_to_v7(connection: &mut Connection) -> Result<()> {
    migrate_v6_to_v7_with_hook(connection, |_| Ok(()))
}

#[cfg(test)]
fn migrate_v6_to_v7_failing(connection: &mut Connection) -> Result<()> {
    migrate_v6_to_v7_with_hook(connection, |_| Err("injected v6-to-v7 migration failure".into()))
}

fn migrate_v6_to_v7_with_hook<F>(connection: &mut Connection, after_ddl: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    apply_schema_change(connection, SCHEMA_V7_VERSION, |transaction| {
        validate_schema_v6(transaction)?;
        transaction
            .execute_batch(CUE_SCHEMA_SQL)
            .map_err(|error| format!("create CUE schema: {error}"))?;
        after_ddl(transaction)?;
        validate_schema_v7(transaction)
    })
    .map_err(|error| format!("catalog v6 to v7 migration failed: {error}"))
}

fn add_cue_audio_identity_columns(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "ALTER TABLE cue_tracks ADD COLUMN audio_device BLOB CHECK(audio_device IS NULL OR (typeof(audio_device)='blob' AND length(audio_device)=8));\
         ALTER TABLE cue_tracks ADD COLUMN audio_inode BLOB CHECK(audio_inode IS NULL OR (typeof(audio_inode)='blob' AND length(audio_inode)=8));\
         ALTER TABLE cue_tracks ADD COLUMN audio_ctime_ns INTEGER;",
    ).map_err(|error| format!("add CUE audio identity columns: {error}"))
}

fn migrate_v7_to_v8(connection: &mut Connection) -> Result<()> {
    migrate_v7_to_v8_with_hook(connection, |_| Ok(()))
}

#[cfg(test)]
fn migrate_v7_to_v8_failing(connection: &mut Connection) -> Result<()> {
    migrate_v7_to_v8_with_hook(connection, |_| Err("injected v7-to-v8 migration failure".into()))
}

fn migrate_v7_to_v8_with_hook<F>(connection: &mut Connection, after_ddl: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    apply_schema_change(connection, SCHEMA_V8_VERSION, |transaction| {
        validate_schema_v7(transaction)?;
        add_cue_audio_identity_columns(transaction)?;
        after_ddl(transaction)?;
        validate_schema_v8(transaction)
    })
    .map_err(|error| format!("catalog v7 to v8 migration failed: {error}"))
}

#[cfg(test)]
fn migrate_v3_to_v4_failing(connection: &mut Connection) -> Result<()> {
    migrate_v3_to_v4_with_hook(connection, |_| {
        Err("injected v3-to-v4 migration failure".into())
    })
}

#[cfg(target_os = "linux")]
fn validate_v1_paths_for_migration(connection: &Connection) -> Result<()> {
    let mut sources = connection
        .prepare("SELECT canonical_path FROM sources")
        .map_err(|error| format!("inspect v1 root paths: {error}"))?;
    let mut rows = sources
        .query([])
        .map_err(|error| format!("inspect v1 root paths: {error}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("inspect v1 root path: {error}"))?
    {
        let value: String = row
            .get(0)
            .map_err(|error| format!("v1 root identity is not valid UTF-8 text: {error}"))?;
        if value.contains('\u{fffd}') {
            return Err("v1 path contains U+FFFD and has ambiguous lossy provenance".into());
        }
        let path = PathBuf::from(value);
        if !path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err("v1 root path is not a normalized absolute Linux path".into());
        }
        path_codec::encode_root(&path).map_err(|error| format!("unsafe v1 root path: {error}"))?;
    }
    drop(rows);
    drop(sources);

    let mut tracks = connection
        .prepare("SELECT relative_path FROM tracks")
        .map_err(|error| format!("inspect v1 relative paths: {error}"))?;
    let mut rows = tracks
        .query([])
        .map_err(|error| format!("inspect v1 relative paths: {error}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("inspect v1 relative path: {error}"))?
    {
        let value: String = row
            .get(0)
            .map_err(|error| format!("v1 relative identity is not valid UTF-8 text: {error}"))?;
        if value.contains('\u{fffd}') {
            return Err("v1 path contains U+FFFD and has ambiguous lossy provenance".into());
        }
        path_codec::validate_relative(Path::new(&value))
            .map_err(|error| format!("unsafe v1 relative path: {error}"))?;
    }
    Ok(())
}

fn validate_path_formats(connection: &Connection) -> Result<()> {
    let native = path_codec::native_format()?;
    let mut statement = connection
        .prepare("SELECT DISTINCT path_format FROM sources")
        .map_err(|error| format!("inspect catalog path formats: {error}"))?;
    let formats = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("inspect catalog path formats: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("inspect catalog path formats: {error}"))?;
    for format in formats {
        if !path_codec::is_known_format(format) {
            return Err(format!("catalog contains unknown path format {format}"));
        }
        if format != native {
            return Err(format!(
                "catalog path format {format} is not supported by this platform"
            ));
        }
    }
    Ok(())
}

/// Applies DDL and its version marker in one transaction. Future explicit
/// migrations should use this helper so failed changes retain the prior version.
fn apply_schema_change<F>(connection: &mut Connection, target_version: i64, change: F) -> Result<()>
where
    F: FnOnce(&Transaction<'_>) -> Result<()>,
{
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("begin schema transaction: {error}"))?;
    change(&transaction)?;
    transaction
        .pragma_update(None, "user_version", target_version)
        .map_err(|error| format!("write schema version: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("commit schema transaction: {error}"))
}

fn validate_schema_v1(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare(
            "SELECT type,name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let expected_objects = vec![
        ("index".to_owned(), "tracks_source_generation".to_owned()),
        ("table".to_owned(), "sources".to_owned()),
        ("table".to_owned(), "tracks".to_owned()),
    ];
    if objects != expected_objects {
        return Err("catalog schema version 1 has incompatible objects".into());
    }

    validate_table_columns(
        connection,
        "sources",
        &[
            ("id", "INTEGER", 0, 1),
            ("canonical_path", "TEXT", 1, 0),
            ("display_name", "TEXT", 1, 0),
            ("generation", "INTEGER", 1, 0),
            ("last_checked_ns", "INTEGER", 1, 0),
        ],
    )?;
    if !normalize_schema_sql(&schema_table_sql(connection, "sources")?)
        .contains("idintegerprimarykeyautoincrement")
    {
        return Err("catalog schema version 1 has an incompatible source ID definition".into());
    }
    validate_table_columns(
        connection,
        "tracks",
        &[
            ("source_id", "INTEGER", 1, 1),
            ("relative_path", "TEXT", 1, 2),
            ("size", "INTEGER", 1, 0),
            ("mtime_ns", "INTEGER", 1, 0),
            ("generation", "INTEGER", 1, 0),
        ],
    )?;
    validate_source_path_uniqueness(connection)?;
    validate_tracks_schema_constraints(connection)?;
    Ok(())
}

fn validate_schema_v2(connection: &Connection) -> Result<()> {
    validate_schema_base(connection, false, false, false, false)
}

fn validate_schema_base(
    connection: &Connection,
    has_metadata_table: bool,
    has_multi_artist_table: bool,
    has_pending_edit_table: bool,
    has_first_seen: bool,
) -> Result<()> {
    let mut statement = connection
        .prepare(
            "SELECT type,name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
        )
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let mut expected_objects = vec![
        ("index".to_owned(), "sources_path_identity".to_owned()),
        ("index".to_owned(), "tracks_source_generation".to_owned()),
        ("table".to_owned(), "sources".to_owned()),
        ("table".to_owned(), "tracks".to_owned()),
    ];
    if has_metadata_table {
        expected_objects.push(("table".to_owned(), "track_metadata".to_owned()));
    }
    if has_multi_artist_table {
        expected_objects.push((
            "table".to_owned(),
            "track_metadata_extra_artists".to_owned(),
        ));
    }
    if has_pending_edit_table {
        expected_objects.push(("table".to_owned(), "source_tag_edit_pending".to_owned()));
    }
    let cue_objects = [
        ("index", "cue_tracks_source_audio"),
        ("table", "cue_sheets"),
        ("table", "cue_track_artists"),
        ("table", "cue_tracks"),
    ];
    let cue_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name IN ('cue_tracks_source_audio','cue_sheets','cue_track_artists','cue_tracks')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    if cue_count != 0 && cue_count != cue_objects.len() as i64 {
        return Err("catalog has an incomplete CUE schema extension".into());
    }
    if cue_count == cue_objects.len() as i64 {
        expected_objects.extend(cue_objects.iter().map(|(kind, name)| (kind.to_string(), name.to_string())));
    }
    let collection_objects = [("table", "personal_collections"), ("table", "personal_collection_members")];
    let found = collection_objects.iter().filter(|(kind, name)| objects.contains(&(kind.to_string(), name.to_string()))).count();
    if found != 0 && found != collection_objects.len() { return Err("incomplete collections schema".into()); }
    if found != 0 { expected_objects.extend(collection_objects.iter().map(|(kind,name)| (kind.to_string(),name.to_string()))); }
    let evidence_objects = [("table", "collection_local_snapshots"), ("table", "collection_local_files"), ("table", "collection_cue_segments")];
    let found = evidence_objects.iter().filter(|(kind, name)| objects.contains(&(kind.to_string(), name.to_string()))).count();
    if found != 0 && found != evidence_objects.len() { return Err("incomplete collection evidence schema".into()); }
    if found != 0 { expected_objects.extend(evidence_objects.iter().map(|(kind, name)| (kind.to_string(), name.to_string()))); }
    expected_objects.sort();
    if objects != expected_objects {
        return Err(format!(
            "catalog schema version {} has incompatible objects",
            if has_multi_artist_table {
                4
            } else if has_metadata_table {
                3
            } else {
                2
            }
        ));
    }
    validate_table_columns(
        connection,
        "sources",
        &[
            ("id", "INTEGER", 0, 1),
            ("path_format", "INTEGER", 1, 0),
            ("canonical_path", "BLOB", 1, 0),
            ("display_name", "TEXT", 1, 0),
            ("generation", "INTEGER", 1, 0),
            ("last_checked_ns", "INTEGER", 1, 0),
        ],
    )?;
    if !normalize_schema_sql(&schema_table_sql(connection, "sources")?)
        .contains("idintegerprimarykeyautoincrement")
    {
        return Err("catalog schema version 2 has an incompatible source ID definition".into());
    }
    let mut track_columns = vec![
            ("source_id", "INTEGER", 1, 1),
            ("relative_path", "BLOB", 1, 2),
            ("size", "INTEGER", 1, 0),
            ("mtime_ns", "INTEGER", 1, 0),
            ("generation", "INTEGER", 1, 0),
        ];
    if has_first_seen {
        track_columns.push(("first_seen_ns", "INTEGER", 0, 0));
    }
    validate_table_columns(connection, "tracks", &track_columns)?;
    validate_source_path_identity_v2(connection)?;
    validate_tracks_schema_constraints(connection)?;

    let sources_sql = normalize_schema_sql(&schema_table_sql(connection, "sources")?);
    let tracks_sql = normalize_schema_sql(&schema_table_sql(connection, "tracks")?);
    if !sources_sql.contains("check(path_formatin(1,2))")
        || !sources_sql.contains("check(typeof(canonical_path)='blob')")
        || !tracks_sql.contains("check(typeof(relative_path)='blob'andlength(relative_path)>0)")
    {
        return Err("catalog schema is missing path format or blob constraints".into());
    }
    Ok(())
}

fn validate_schema_v3(connection: &Connection) -> Result<()> {
    validate_schema_base(connection, true, false, false, false)?;

    validate_track_metadata_schema(connection)
}

fn validate_schema_v4(connection: &Connection) -> Result<()> {
    validate_schema_v4_core(connection, false, false)
}

fn validate_schema_v4_core(connection: &Connection, has_pending_edit_table: bool, has_first_seen: bool) -> Result<()> {
    validate_schema_base(connection, true, true, has_pending_edit_table, has_first_seen)?;
    validate_track_metadata_schema(connection)?;

    validate_table_columns(
        connection,
        "track_metadata_extra_artists",
        &[
            ("source_id", "INTEGER", 1, 1),
            ("relative_path", "BLOB", 1, 2),
            ("artist_role", "INTEGER", 1, 3),
            ("ordinal", "INTEGER", 1, 4),
            ("value", "TEXT", 1, 0),
        ],
    )?;

    let mut statement = connection
        .prepare("PRAGMA foreign_key_list(track_metadata_extra_artists)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let mut foreign_keys = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    foreign_keys.sort();
    if foreign_keys
        != [
            (
                "track_metadata".to_owned(),
                "relative_path".to_owned(),
                "relative_path".to_owned(),
                "CASCADE".to_owned(),
            ),
            (
                "track_metadata".to_owned(),
                "source_id".to_owned(),
                "source_id".to_owned(),
                "CASCADE".to_owned(),
            ),
        ]
    {
        return Err("catalog artist values are missing the metadata cascade foreign key".into());
    }
    let sql = normalize_schema_sql(&schema_table_sql(
        connection,
        "track_metadata_extra_artists",
    )?);
    if !sql.contains("check(artist_rolein(1,2))")
        || !sql.contains("check(ordinal>=2)")
        || !sql.contains("check(length(value)>0)")
    {
        return Err(
            "catalog artist values table is missing role, order, or value constraints".into(),
        );
    }
    Ok(())
}

fn validate_schema_v5(connection: &Connection) -> Result<()> {
    validate_schema_v5_core(connection, false)
}

fn validate_schema_v5_core(connection: &Connection, has_first_seen: bool) -> Result<()> {
    validate_schema_v4_core(connection, true, has_first_seen)?;
    validate_table_columns(
        connection,
        "source_tag_edit_pending",
        &[
            ("source_id", "INTEGER", 0, 1),
            ("generation", "INTEGER", 1, 0),
            ("started_ns", "INTEGER", 1, 0),
        ],
    )?;
    let mut statement = connection
        .prepare("PRAGMA foreign_key_list(source_tag_edit_pending)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let foreign_keys = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    if foreign_keys
        != [("sources".to_owned(), "source_id".to_owned(), "id".to_owned(), "CASCADE".to_owned())]
    {
        return Err("pending tag edit table is missing its source cascade foreign key".into());
    }
    let sql = normalize_schema_sql(&schema_table_sql(connection, "source_tag_edit_pending")?);
    if !sql.contains("check(generation>=1)") {
        return Err("pending tag edit table is missing its generation constraint".into());
    }
    Ok(())
}

fn validate_schema_v6(connection: &Connection) -> Result<()> {
    validate_schema_v5_core(connection, true)?;
    let sql = normalize_schema_sql(&schema_table_sql(connection, "tracks")?);
    if !sql.contains("check(first_seen_nsisnullorfirst_seen_ns>=0)") {
        return Err("catalog first-seen timestamp is missing its non-negative constraint".into());
    }
    Ok(())
}

fn validate_schema_v7(connection: &Connection) -> Result<()> {
    validate_schema_v7_core(connection, false)
}

fn validate_schema_v7_core(connection: &Connection, has_audio_identity: bool) -> Result<()> {
    validate_schema_v6(connection)?;
    validate_table_columns(
        connection,
        "cue_sheets",
        &[
            ("source_id", "INTEGER", 1, 1),
            ("relative_path", "BLOB", 1, 2),
            ("size", "INTEGER", 1, 0),
            ("mtime_ns", "INTEGER", 1, 0),
            ("device", "BLOB", 1, 0),
            ("inode", "BLOB", 1, 0),
            ("ctime_ns", "INTEGER", 1, 0),
            ("parser_version", "INTEGER", 1, 0),
            ("status", "INTEGER", 1, 0),
            ("error_code", "TEXT", 0, 0),
            ("track_count", "INTEGER", 1, 0),
        ],
    )?;
    let mut cue_track_columns = vec![
            ("source_id", "INTEGER", 1, 1),
            ("cue_path", "BLOB", 1, 2),
            ("track_number", "INTEGER", 1, 3),
            ("audio_path", "BLOB", 1, 0),
            ("start_frame", "INTEGER", 1, 0),
            ("end_frame", "INTEGER", 0, 0),
            ("title", "TEXT", 0, 0),
            ("album", "TEXT", 0, 0),
            ("genre", "TEXT", 0, 0),
            ("date_text", "TEXT", 0, 0),
            ("disc_number", "INTEGER", 0, 0),
        ];
    if has_audio_identity {
        cue_track_columns.extend([
            ("audio_device", "BLOB", 0, 0),
            ("audio_inode", "BLOB", 0, 0),
            ("audio_ctime_ns", "INTEGER", 0, 0),
        ]);
    }
    validate_table_columns(connection, "cue_tracks", &cue_track_columns)?;
    validate_table_columns(
        connection,
        "cue_track_artists",
        &[
            ("source_id", "INTEGER", 1, 1),
            ("cue_path", "BLOB", 1, 2),
            ("track_number", "INTEGER", 1, 3),
            ("artist_role", "INTEGER", 1, 4),
            ("ordinal", "INTEGER", 1, 5),
            ("value", "TEXT", 1, 0),
        ],
    )?;
    let sheet_sql = normalize_schema_sql(&schema_table_sql(connection, "cue_sheets")?);
    let track_sql = normalize_schema_sql(&schema_table_sql(connection, "cue_tracks")?);
    if !sheet_sql.contains("check(typeof(relative_path)='blob'andlength(relative_path)>0)")
        || !sheet_sql.contains("check(typeof(device)='blob'andlength(device)=8)")
        || !sheet_sql.contains("check(typeof(inode)='blob'andlength(inode)=8)")
        || !track_sql.contains("check(end_frameisnullorend_frame>start_frame)")
    {
        return Err("CUE schema is missing path, identity, or segment-bound constraints".into());
    }
    let violations: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| row.get(0))
        .map_err(|error| format!("validate CUE foreign keys: {error}"))?;
    if violations != 0 {
        return Err("catalog contains foreign-key violations".into());
    }
    Ok(())
}

fn validate_schema_v8(connection: &Connection) -> Result<()> {
    validate_schema_v7_core(connection, true)?;
    let sql = normalize_schema_sql(&schema_table_sql(connection, "cue_tracks")?);
    if !sql.contains("check(audio_deviceisnullor(typeof(audio_device)='blob'andlength(audio_device)=8))")
        || !sql.contains("check(audio_inodeisnullor(typeof(audio_inode)='blob'andlength(audio_inode)=8))")
    {
        return Err("CUE audio identity columns are missing their BLOB constraints".into());
    }
    let partial: i64 = connection
        .query_row(
            "SELECT count(*) FROM cue_tracks WHERE NOT ((audio_device IS NULL AND audio_inode IS NULL AND audio_ctime_ns IS NULL) OR (audio_device IS NOT NULL AND audio_inode IS NOT NULL AND audio_ctime_ns IS NOT NULL))",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("validate CUE audio identity state: {error}"))?;
    if partial != 0 {
        return Err("catalog contains partially populated CUE audio identities".into());
    }
    Ok(())
}

fn validate_track_metadata_schema(connection: &Connection) -> Result<()> {
    validate_table_columns(
        connection,
        "track_metadata",
        &[
            ("source_id", "INTEGER", 1, 1),
            ("relative_path", "BLOB", 1, 2),
            ("attempt_status", "INTEGER", 1, 0),
            ("attempt_size", "INTEGER", 1, 0),
            ("attempt_mtime_ns", "INTEGER", 1, 0),
            ("attempt_parser_version", "INTEGER", 1, 0),
            ("file_format", "TEXT", 1, 0),
            ("error", "TEXT", 0, 0),
            ("success_size", "INTEGER", 0, 0),
            ("success_mtime_ns", "INTEGER", 0, 0),
            ("title", "TEXT", 0, 0),
            ("track_artist", "TEXT", 0, 0),
            ("album_artist", "TEXT", 0, 0),
            ("album", "TEXT", 0, 0),
            ("genre", "TEXT", 0, 0),
            ("date_text", "TEXT", 0, 0),
            ("track_number", "INTEGER", 0, 0),
            ("disc_number", "INTEGER", 0, 0),
            ("duration_ms", "INTEGER", 0, 0),
            ("sample_rate", "INTEGER", 0, 0),
            ("channels", "INTEGER", 0, 0),
        ],
    )?;

    let mut statement = connection
        .prepare("PRAGMA foreign_key_list(track_metadata)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let mut foreign_keys = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    foreign_keys.sort();
    if foreign_keys
        != [
            (
                "tracks".to_owned(),
                "relative_path".to_owned(),
                "relative_path".to_owned(),
                "CASCADE".to_owned(),
            ),
            (
                "tracks".to_owned(),
                "source_id".to_owned(),
                "source_id".to_owned(),
                "CASCADE".to_owned(),
            ),
        ]
    {
        return Err(
            "catalog metadata table is missing the composite track cascade foreign key".into(),
        );
    }

    let sql = normalize_schema_sql(&schema_table_sql(connection, "track_metadata")?);
    if !sql.contains("check(attempt_statusin(1,2,3,4))")
        || !sql.contains("check(attempt_size>=0)")
        || !sql.contains("check(attempt_parser_version>=1)")
        || !sql.contains("check((success_sizeisnullandsuccess_mtime_nsisnull)or(success_sizeisnotnullandsuccess_mtime_nsisnotnull))")
    {
        return Err("catalog metadata table is missing required state and provenance constraints".into());
    }
    Ok(())
}

fn schema_table_sql(connection: &Connection, table: &str) -> Result<String> {
    connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| format!("sqlite schema validation: {error}"))
}

fn validate_table_columns(
    connection: &Connection,
    table: &str,
    expected: &[(&str, &str, i64, i64)],
) -> Result<()> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let actual = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let expected = expected
        .iter()
        .map(|(name, kind, not_null, primary_key)| {
            (name.to_string(), kind.to_string(), *not_null, *primary_key)
        })
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(format!(
            "catalog schema table {table} has incompatible columns"
        ));
    }
    Ok(())
}

fn validate_source_path_uniqueness(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare("PRAGMA index_list(sources)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(4)? != 0,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    for (name, unique, partial) in indexes {
        if unique && !partial {
            let quoted_name = name.replace('\"', "\"\"");
            let mut info = connection
                .prepare(&format!("PRAGMA index_info(\"{quoted_name}\")"))
                .map_err(|error| format!("sqlite schema validation: {error}"))?;
            let columns = info
                .query_map([], |row| row.get::<_, String>(2))
                .map_err(|error| format!("sqlite schema validation: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("sqlite schema validation: {error}"))?;
            if columns == ["canonical_path"] {
                return Ok(());
            }
        }
    }
    Err("catalog schema is missing unique source path constraint".into())
}

fn validate_source_path_identity_v2(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare("PRAGMA index_list(sources)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(4)? != 0,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    for (name, unique, partial) in indexes {
        if unique && !partial {
            let quoted_name = name.replace('"', "\"\"");
            let mut info = connection
                .prepare(&format!("PRAGMA index_info(\"{quoted_name}\")"))
                .map_err(|error| format!("sqlite schema validation: {error}"))?;
            let columns = info
                .query_map([], |row| row.get::<_, String>(2))
                .map_err(|error| format!("sqlite schema validation: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("sqlite schema validation: {error}"))?;
            if columns == ["path_format", "canonical_path"] {
                return Ok(());
            }
        }
    }
    Err("catalog schema is missing unique source path format and identity constraint".into())
}

fn validate_tracks_schema_constraints(connection: &Connection) -> Result<()> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_list(tracks)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let foreign_keys = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let expected_foreign_keys = vec![(
        "sources".to_owned(),
        "source_id".to_owned(),
        "id".to_owned(),
        "NO ACTION".to_owned(),
        "CASCADE".to_owned(),
    )];
    if foreign_keys != expected_foreign_keys {
        return Err("catalog schema is missing the source cascade foreign key".into());
    }

    let mut statement = connection
        .prepare("PRAGMA index_info(tracks_source_generation)")
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    let index_columns = statement
        .query_map([], |row| row.get::<_, String>(2))
        .map_err(|error| format!("sqlite schema validation: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite schema validation: {error}"))?;
    if index_columns != ["source_id", "generation"] {
        return Err("catalog schema has an incompatible source generation index".into());
    }

    let table_sql = |table: &str| -> Result<String> {
        connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| format!("sqlite schema validation: {error}"))
    };
    let sources_sql = normalize_schema_sql(&table_sql("sources")?);
    let tracks_sql = normalize_schema_sql(&table_sql("tracks")?);
    if !sources_sql.contains("check(generation>=1)")
        || !tracks_sql.contains("check(size>=0)")
        || !tracks_sql.contains("check(generation>=1)")
    {
        return Err("catalog schema is missing required value constraints".into());
    }
    Ok(())
}

fn normalize_schema_sql(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn execute(connection: &Connection, sql: &str, args: &[Value]) -> Result<()> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("sqlite prepare: {error}"))?;
    statement
        .execute(params_from_iter(args.iter()))
        .map(|_| ())
        .map_err(|error| format!("sqlite step: {error}"))
}

fn query(connection: &Connection, sql: &str, args: &[Value]) -> Result<Vec<Vec<Value>>> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("sqlite prepare: {error}"))?;
    let columns = statement.column_count();
    let rows = statement
        .query_map(params_from_iter(args.iter()), |row| {
            (0..columns)
                .map(|column| row.get(column))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .map_err(|error| format!("sqlite query: {error}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("sqlite query: {error}"))
}

struct Tx<'a> {
    transaction: Transaction<'a>,
}
impl<'a> Tx<'a> {
    fn begin(db: &'a mut Db) -> Result<Self> {
        let transaction = db
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("sqlite begin immediate: {error}"))?;
        Ok(Self { transaction })
    }

    fn execute(&self, sql: &str, args: &[Value]) -> Result<()> {
        execute(&self.transaction, sql, args)
    }

    fn query(&self, sql: &str, args: &[Value]) -> Result<Vec<Vec<Value>>> {
        query(&self.transaction, sql, args)
    }

    fn last_insert_rowid(&self) -> i64 {
        self.transaction.last_insert_rowid()
    }

    fn commit(self) -> Result<()> {
        self.transaction
            .commit()
            .map_err(|error| format!("sqlite commit: {error}"))
    }
}
fn int(row: &[Value], n: usize) -> i64 {
    if let Value::Int(v) = row[n] {
        v
    } else {
        0
    }
}
fn text(row: &[Value], n: usize) -> String {
    if let Value::Text(ref v) = row[n] {
        v.clone()
    } else {
        String::new()
    }
}
fn blob(row: &[Value], n: usize) -> Vec<u8> {
    if let Value::Blob(ref value) = row[n] {
        value.clone()
    } else {
        Vec::new()
    }
}
fn optional_int(row: &[Value], n: usize) -> Option<i64> {
    if let Value::Int(value) = row.get(n)? {
        Some(*value)
    } else {
        None
    }
}
fn optional_text(row: &[Value], n: usize) -> Option<String> {
    if let Value::Text(value) = row.get(n)? {
        Some(value.clone())
    } else {
        None
    }
}
fn val_i(n: i64) -> Value {
    Value::Int(n)
}
fn val_t(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}
fn val_b(bytes: impl Into<Vec<u8>>) -> Value {
    Value::Blob(bytes.into())
}
fn val_opt_i(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::Int)
}
fn val_opt_t(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::Text(value.to_owned()))
}
fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

#[derive(Clone, Debug)]
pub(crate) struct DiskFile {
    pub(crate) relative: PathBuf,
    pub(crate) size: i64,
    pub(crate) mtime_ns: i64,
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) ctime_ns: i64,
}
#[derive(Default)]
pub(crate) struct WalkResult {
    pub(crate) files: Vec<DiskFile>,
    pub(crate) cue_files: Vec<DiskFile>,
    pub(crate) entries: usize,
    pub(crate) files_examined: usize,
}
fn is_cue(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("cue"))
}
fn is_audio(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "aac"
            | "aif"
            | "aiff"
            | "aifc"
            | "ape"
            | "dsf"
            | "dff"
            | "flac"
            | "m4a"
            | "mp3"
            | "mp4"
            | "oga"
            | "ogg"
            | "opus"
            | "wav"
            | "wave"
            | "wv"
            | "wma"
    )
}
fn canonical_dir(p: &Path) -> Result<PathBuf> {
    let c = fs::canonicalize(p).map_err(|e| format!("cannot resolve source: {e}"))?;
    if !c.is_dir() {
        return Err("source is not a directory".into());
    }
    Ok(c)
}
fn validate_non_overlap(connection: &Connection, path: &Path) -> Result<()> {
    let candidate = path_codec::encode_root(path)?;
    let format = path_codec::native_format()?;
    for row in query(
        connection,
        "SELECT path_format,canonical_path FROM sources",
        &[],
    )? {
        let existing_format = int(&row, 0);
        let existing = path_codec::decode_root(existing_format, &blob(&row, 1))?;
        if existing == path || existing.starts_with(path) || path.starts_with(&existing) {
            return Err(format!(
                "overlapping source roots are not supported: {candidate:02x?} (format {format})"
            ));
        }
    }
    Ok(())
}
fn source_row(connection: &Connection, id: i64) -> Result<Source> {
    let rows = query(
        connection,
        "SELECT id,path_format,canonical_path,display_name,generation,last_checked_ns FROM sources WHERE id=?",
        &[val_i(id)],
    )?;
    let r = rows.first().ok_or("source not found")?;
    Ok(Source {
        id: int(r, 0),
        canonical_path: path_codec::decode_root(int(r, 1), &blob(r, 2))?,
        display_name: text(r, 3),
        generation: int(r, 4),
        last_checked_ns: int(r, 5),
    })
}

fn source_track_row(connection: &Connection, source_id: i64, relative: &Path) -> Result<Track> {
    let encoded = path_codec::encode_relative(relative)?;
    let rows = query(
        connection,
        "SELECT tracks.source_id,sources.path_format,tracks.relative_path,tracks.size,tracks.mtime_ns,tracks.generation,tracks.first_seen_ns FROM tracks JOIN sources ON sources.id=tracks.source_id WHERE tracks.source_id=? AND tracks.relative_path=?",
        &[val_i(source_id), val_b(encoded)],
    )?;
    let row = rows.first().ok_or_else(|| "track is not indexed in this source".to_owned())?;
    Ok(Track {
        source_id: int(row, 0),
        relative_path: path_codec::decode_relative(int(row, 1), &blob(row, 2))?,
        size: int(row, 3),
        mtime_ns: int(row, 4),
        generation: int(row, 5),
        first_seen_ns: optional_int(row, 6),
    })
}

fn validate_observed_generation(connection: &Connection, id: i64, observed: i64) -> Result<()> {
    let rows = query(
        connection,
        "SELECT generation FROM sources WHERE id=?",
        &[val_i(id)],
    )?;
    let Some(row) = rows.first() else {
        return Err("stale verification discarded: source was removed".into());
    };
    let current = int(row, 0);
    if current != observed {
        return Err(format!(
            "stale verification discarded: source generation advanced from {observed} to {current}"
        ));
    }
    Ok(())
}

fn validate_tag_edit_pending(connection: &Connection, id: i64, expected: bool) -> Result<()> {
    let pending = !query(
        connection,
        "SELECT 1 FROM source_tag_edit_pending WHERE source_id=?",
        &[val_i(id)],
    )?
    .is_empty();
    if pending != expected {
        return Err("source tag edit state changed while verification was running".into());
    }
    Ok(())
}
#[derive(Clone, Debug)]
struct ExistingTrack {
    size: i64,
    mtime_ns: i64,
    attempt_size: Option<i64>,
    attempt_mtime_ns: Option<i64>,
    attempt_parser_version: Option<i64>,
    attempt_error: Option<String>,
}

fn existing_tracks(connection: &Connection, id: i64) -> Result<HashMap<PathBuf, ExistingTrack>> {
    let mut statement = connection
        .prepare(
            "SELECT sources.path_format,tracks.relative_path,tracks.size,tracks.mtime_ns,\
                    track_metadata.attempt_size,track_metadata.attempt_mtime_ns,\
                    track_metadata.attempt_parser_version,track_metadata.error \
             FROM tracks JOIN sources ON sources.id=tracks.source_id \
             LEFT JOIN track_metadata ON track_metadata.source_id=tracks.source_id \
                 AND track_metadata.relative_path=tracks.relative_path \
             WHERE tracks.source_id=?1",
        )
        .map_err(|error| format!("read existing tracks: {error}"))?;
    let rows = statement
        .query_map([id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                ExistingTrack {
                    size: row.get(2)?,
                    mtime_ns: row.get(3)?,
                    attempt_size: row.get(4)?,
                    attempt_mtime_ns: row.get(5)?,
                    attempt_parser_version: row.get(6)?,
                    attempt_error: row.get(7)?,
                },
            ))
        })
        .map_err(|error| format!("read existing tracks: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("read existing tracks: {error}"))?;
    rows.into_iter()
        .map(|(format, relative, state)| {
            Ok((path_codec::decode_relative(format, &relative)?, state))
        })
        .collect()
}

#[derive(Debug)]
struct MetadataUpdate {
    relative: PathBuf,
    size: i64,
    mtime_ns: i64,
    parser_version: i64,
    attempt: metadata::Attempt,
}

#[derive(Default)]
struct MetadataWork {
    updates: Vec<MetadataUpdate>,
    metrics: Metrics,
}

// Version 4 rebinds sheets whose FILE references became cataloged audio when
// WavPack support was added. Version 5 accepts unknown valueless REM flags.
// Previously rejected sheets are refreshed when this parser version changes.
const CUE_PARSER_VERSION: i64 = 5;

#[derive(Clone)]
struct ExistingCueSheet {
    file: DiskFile,
    parser_version: i64,
    status: i64,
    error_code: Option<String>,
    track_count: usize,
    audio_files: Vec<ExistingCueAudio>,
}

#[derive(Clone)]
struct ExistingCueAudio {
    relative: PathBuf,
    size: i64,
    mtime_ns: i64,
    identity: Option<(u64, u64, i64)>,
}

struct CueSheetUpdate {
    file: DiskFile,
    status: CueSheetStatus,
    tracks: Vec<CueCatalogTrack>,
}

struct CueCatalogTrack {
    parsed: cue::CueTrack,
    audio: DiskFile,
}

#[derive(Default)]
struct CueWork {
    changed: bool,
    sheets: Vec<CueSheetUpdate>,
}

fn existing_cue_sheets(connection: &Connection, source_id: i64) -> Result<HashMap<PathBuf, ExistingCueSheet>> {
    let path_format = path_codec::native_format()?;
    let mut statement = connection
        .prepare("SELECT relative_path,size,mtime_ns,device,inode,ctime_ns,parser_version,status,error_code,track_count FROM cue_sheets WHERE source_id=? ORDER BY relative_path")
        .map_err(|error| format!("read existing CUE sheets: {error}"))?;
    let rows = statement.query_map([source_id], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            DiskFile {
                relative: PathBuf::new(),
                size: row.get(1)?,
                mtime_ns: row.get(2)?,
                device: u64::from_be_bytes(row.get::<_, Vec<u8>>(3)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?),
                inode: u64::from_be_bytes(row.get::<_, Vec<u8>>(4)?.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?),
                ctime_ns: row.get(5)?,
            },
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, i64>(9)?,
        ))
    }).map_err(|error| format!("read existing CUE sheets: {error}"))?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(|error| format!("read existing CUE sheets: {error}"))?;
    let mut existing = HashMap::new();
    for (relative_blob, mut file, parser_version, status, error_code, track_count) in rows {
        file.relative = path_codec::decode_relative(path_format, &relative_blob)?;
        existing.insert(file.relative.clone(), ExistingCueSheet {
            file,
            parser_version,
            status,
            error_code,
            track_count: usize::try_from(track_count).map_err(|_| "catalog CUE track count is invalid")?,
            audio_files: Vec::new(),
        });
    }
    let mut statement = connection.prepare("SELECT cue_tracks.cue_path,cue_tracks.audio_path,tracks.size,tracks.mtime_ns,cue_tracks.audio_device,cue_tracks.audio_inode,cue_tracks.audio_ctime_ns FROM cue_tracks JOIN tracks ON tracks.source_id=cue_tracks.source_id AND tracks.relative_path=cue_tracks.audio_path WHERE cue_tracks.source_id=? ORDER BY cue_tracks.cue_path,cue_tracks.track_number")
        .map_err(|error| format!("read existing CUE references: {error}"))?;
    let references = statement.query_map([source_id], |row| Ok((
        row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, i64>(2)?, row.get::<_, i64>(3)?,
        row.get::<_, Option<Vec<u8>>>(4)?, row.get::<_, Option<Vec<u8>>>(5)?, row.get::<_, Option<i64>>(6)?,
    )))
        .map_err(|error| format!("read existing CUE references: {error}"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("read existing CUE references: {error}"))?;
    for (cue_blob, audio_blob, size, mtime_ns, device, inode, ctime_ns) in references {
        let cue_path = path_codec::decode_relative(path_format, &cue_blob)?;
        let audio_path = path_codec::decode_relative(path_format, &audio_blob)?;
        let identity = match (device, inode, ctime_ns) {
            (Some(device), Some(inode), Some(ctime_ns)) => Some((
                u64::from_be_bytes(device.try_into().map_err(|_| "catalog CUE audio device identity is invalid")?),
                u64::from_be_bytes(inode.try_into().map_err(|_| "catalog CUE audio inode identity is invalid")?),
                ctime_ns,
            )),
            (None, None, None) => None,
            _ => return Err("catalog contains a partial CUE audio identity".into()),
        };
        if let Some(sheet) = existing.get_mut(&cue_path) {
            sheet.audio_files.push(ExistingCueAudio { relative: audio_path, size, mtime_ns, identity });
        }
    }
    Ok(existing)
}

fn cue_stamp_matches(left: &DiskFile, right: &DiskFile) -> bool {
    left.size == right.size
        && left.mtime_ns == right.mtime_ns
        && left.device == right.device
        && left.inode == right.inode
        && left.ctime_ns == right.ctime_ns
}

fn prepare_cue_work(
    root: &Path,
    source_id: Option<i64>,
    cue_files: &[DiskFile],
    audio_files: &[DiskFile],
    previous_audio_paths: Option<&std::collections::HashSet<PathBuf>>,
    connection: &Connection,
    force: bool,
) -> Result<CueWork> {
    let existing = match source_id {
        Some(source_id) => existing_cue_sheets(connection, source_id)?,
        None => HashMap::new(),
    };
    let audio_by_path = audio_files.iter().map(|file| (file.relative.clone(), file)).collect::<HashMap<_, _>>();
    let audio_membership_changed = previous_audio_paths.is_some_and(|previous| {
        previous.len() != audio_by_path.len() || previous.iter().any(|path| !audio_by_path.contains_key(path))
    });
    let mut unchanged = !force && existing.len() == cue_files.len();
    if unchanged {
        for file in cue_files {
            let Some(old) = existing.get(&file.relative) else { unchanged = false; break; };
            if !cue_stamp_matches(&old.file, file) || old.parser_version != CUE_PARSER_VERSION {
                unchanged = false;
                break;
            }
            if old.status == 1 {
                if old.audio_files.len() != old.track_count || old.audio_files.iter().any(|old_audio| {
                    let Some(current) = audio_by_path.get(&old_audio.relative) else { return true; };
                    old_audio.identity != Some((current.device, current.inode, current.ctime_ns))
                        || old_audio.size != current.size
                        || old_audio.mtime_ns != current.mtime_ns
                }) {
                    unchanged = false;
                    break;
                }
            } else if old.error_code.as_deref() == Some("cue_file_reference_unavailable") && audio_membership_changed {
                unchanged = false;
                break;
            }
        }
    }
    if unchanged {
        return Ok(CueWork::default());
    }

    let mut work = CueWork { changed: true, sheets: Vec::with_capacity(cue_files.len()) };
    for file in cue_files {
        let parsed = if file.size < 0 || file.size as u64 > cue::MAX_CUE_BYTES as u64 {
            Err("CUE sheet exceeds the configured size limit".to_owned())
        } else {
            scanner::with_open_file(root, file, |reader| {
                let mut bytes = Vec::with_capacity(file.size.min(cue::MAX_CUE_BYTES as i64) as usize);
                let mut limited = reader.take(cue::MAX_CUE_BYTES as u64 + 1);
                limited.read_to_end(&mut bytes).map_err(|_| "CUE sheet read failed".to_owned())?;
                if bytes.len() > cue::MAX_CUE_BYTES {
                    return Err("CUE sheet exceeds the configured size limit".into());
                }
                cue::parse(&bytes)
            })?
        };
        let mut tracks = Vec::new();
        let status = match parsed {
            Err(error) => CueSheetStatus::Rejected {
                error_code: if error == "CUE REM known directive is missing a value" {
                    "cue_rem_value_missing"
                } else {
                    "cue_parse_rejected"
                }
                .into(),
            },
            Ok(mut sheet) => {
                for track in &mut sheet.tracks {
                    track.file = cue::resolve_file_from_sheet(&file.relative, &track.file)?;
                }
                let resolved_sheet_files = sheet.files.iter()
                    .map(|path| cue::resolve_file_from_sheet(&file.relative, path))
                    .collect::<Result<Vec<_>>>()?;
                let all_references_present = resolved_sheet_files.iter().all(|path| audio_by_path.contains_key(path));
                if !all_references_present {
                    CueSheetStatus::Rejected { error_code: "cue_file_reference_unavailable".into() }
                } else {
                    let mut verified = BTreeSet::new();
                    for path in &resolved_sheet_files {
                        if verified.insert(path.clone()) {
                            let audio = audio_by_path.get(path).ok_or("CUE audio reference disappeared")?;
                            scanner::with_open_file(root, audio, |_| ())?;
                        }
                    }
                    tracks = sheet.tracks.into_iter().map(|parsed| {
                        let audio = (*audio_by_path.get(&parsed.file)
                            .ok_or("CUE audio reference disappeared")?).clone();
                        Ok(CueCatalogTrack { parsed, audio })
                    }).collect::<Result<Vec<_>>>()?;
                    CueSheetStatus::Indexed
                }
            }
        };
        work.sheets.push(CueSheetUpdate { file: file.clone(), status, tracks });
    }
    Ok(work)
}

fn replace_cue_catalog(tx: &Tx<'_>, source_id: i64, work: &CueWork) -> Result<()> {
    if !work.changed { return Ok(()); }
    tx.execute("DELETE FROM cue_sheets WHERE source_id=?", &[val_i(source_id)])?;
    for sheet in &work.sheets {
        let cue_blob = path_codec::encode_relative(&sheet.file.relative)?;
        let (status, error_code) = match &sheet.status {
            CueSheetStatus::Indexed => (1, None),
            CueSheetStatus::Rejected { error_code } => (2, Some(error_code.as_str())),
        };
        tx.execute("INSERT INTO cue_sheets(source_id,relative_path,size,mtime_ns,device,inode,ctime_ns,parser_version,status,error_code,track_count) VALUES(?,?,?,?,?,?,?,?,?,?,?)", &[
            val_i(source_id), val_b(cue_blob.clone()), val_i(sheet.file.size), val_i(sheet.file.mtime_ns),
            val_b(sheet.file.device.to_be_bytes().to_vec()), val_b(sheet.file.inode.to_be_bytes().to_vec()),
            val_i(sheet.file.ctime_ns), val_i(CUE_PARSER_VERSION), val_i(status),
            val_opt_t(error_code), val_i(sheet.tracks.len() as i64),
        ])?;
        for track in &sheet.tracks {
            let audio_blob = path_codec::encode_relative(&track.parsed.file)?;
            let start = i64::try_from(track.parsed.start_frame).map_err(|_| "CUE start frame exceeds catalog bounds")?;
            let end = track.parsed.end_frame.map(i64::try_from).transpose().map_err(|_| "CUE end frame exceeds catalog bounds")?;
            tx.execute("INSERT INTO cue_tracks(source_id,cue_path,track_number,audio_path,start_frame,end_frame,title,album,genre,date_text,disc_number,audio_device,audio_inode,audio_ctime_ns) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)", &[
                val_i(source_id), val_b(cue_blob.clone()), val_i(track.parsed.number as i64), val_b(audio_blob),
                val_i(start), val_opt_i(end), val_opt_t(track.parsed.title.as_deref()), val_opt_t(track.parsed.album.as_deref()),
                val_opt_t(track.parsed.genre.as_deref()), val_opt_t(track.parsed.date.as_deref()), val_opt_i(track.parsed.disc_number.map(i64::from)),
                val_b(track.audio.device.to_be_bytes().to_vec()), val_b(track.audio.inode.to_be_bytes().to_vec()), val_i(track.audio.ctime_ns),
            ])?;
            for (role, artists) in [(1_i64, &track.parsed.performers), (2_i64, &track.parsed.album_performers)] {
                for (index, artist) in artists.iter().enumerate() {
                    tx.execute("INSERT INTO cue_track_artists(source_id,cue_path,track_number,artist_role,ordinal,value) VALUES(?,?,?,?,?,?)", &[
                        val_i(source_id), val_b(cue_blob.clone()), val_i(track.parsed.number as i64), val_i(role), val_i(index as i64 + 1), val_t(artist),
                    ])?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct IndexPreviewSummary {
    album_count: usize,
    artist_count: usize,
}

fn build_index_preview(updates: &[MetadataUpdate]) -> Result<IndexPreviewSummary> {
    let candidate_tracks = updates
        .iter()
        .map(|update| {
            let successful = matches!(
                update.attempt.status,
                MetadataStatus::ParsedWithTags | MetadataStatus::ParsedWithoutTags
            );
            AlbumTrack {
                track: Track {
                    source_id: 0,
                    relative_path: update.relative.clone(),
                    size: update.size,
                    mtime_ns: update.mtime_ns,
                    generation: 1,
                    first_seen_ns: None,
                },
                metadata: Some(TrackMetadata {
                    status: update.attempt.status,
                    file_format: update.attempt.format.clone(),
                    parser_version: update.parser_version,
                    attempted_size: update.size,
                    attempted_mtime_ns: update.mtime_ns,
                    error: update.attempt.error.clone(),
                    last_success_size: successful.then_some(update.size),
                    last_success_mtime_ns: successful.then_some(update.mtime_ns),
                    last_success: successful.then(|| update.attempt.values.clone()).flatten(),
                    current_for_file: successful,
                }),
                metadata_stale: false,
            }
        })
        .collect::<Vec<_>>();
    let albums = group_albums(&candidate_tracks)?;
    let artists = grouped_artists(0, &candidate_tracks, &albums);
    Ok(IndexPreviewSummary {
        album_count: albums.len(),
        artist_count: artists.len(),
    })
}

fn is_wav(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| {
            value.eq_ignore_ascii_case("wav") || value.eq_ignore_ascii_case("wave")
        })
}

fn is_aiff(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "aif" | "aiff"))
}

fn is_aifc(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("aifc"))
}

fn is_aac(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("aac"))
}

fn is_ape(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("ape"))
}

fn is_flac(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("flac"))
}

fn is_mp3(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mp3"))
}

fn is_ogg_candidate(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "ogg" | "oga" | "opus"))
}

fn is_mp4_candidate(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "m4a" | "mp4"))
}

fn is_dsf(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("dsf"))
}

fn is_dff(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("dff"))
}

fn is_wavpack(p: &Path) -> bool {
    p.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("wv"))
}

fn prepare_metadata_with_progress<F>(
    root: &Path,
    files: &[DiskFile],
    previous: &HashMap<PathBuf, ExistingTrack>,
    force: bool,
    progress: F,
) -> Result<MetadataWork>
where
    F: FnMut(usize, usize, &[MetadataUpdate]) -> Result<()>,
{
    prepare_metadata_with_opener_and_progress(
        root,
        files,
        previous,
        force,
        |root, file, parser| scanner::with_open_file(root, file, parser),
        progress,
    )
}

type MetadataParser = fn(BufReader<File>) -> metadata::Attempt;

#[cfg(test)]
fn prepare_metadata_with_opener<F>(
    root: &Path,
    files: &[DiskFile],
    previous: &HashMap<PathBuf, ExistingTrack>,
    force: bool,
    open_file: F,
) -> Result<MetadataWork>
where
    F: Fn(&Path, &DiskFile, MetadataParser) -> Result<metadata::Attempt>,
{
    prepare_metadata_with_opener_and_progress(root, files, previous, force, open_file, |_, _, _| {
        Ok(())
    })
}

fn prepare_metadata_with_opener_and_progress<F, P>(
    root: &Path,
    files: &[DiskFile],
    previous: &HashMap<PathBuf, ExistingTrack>,
    force: bool,
    open_file: F,
    mut progress: P,
) -> Result<MetadataWork>
where
    F: Fn(&Path, &DiskFile, MetadataParser) -> Result<metadata::Attempt>,
    P: FnMut(usize, usize, &[MetadataUpdate]) -> Result<()>,
{
    let mut work = MetadataWork::default();
    for (index, file) in files.iter().enumerate() {
        let extension = file
            .relative
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown");
        let parser_version = metadata::parser_version_for_extension(extension);
        let already_attempted = previous.get(&file.relative).is_some_and(|old| {
            old.size == file.size
                && old.mtime_ns == file.mtime_ns
                && old.attempt_size == Some(file.size)
                && old.attempt_mtime_ns == Some(file.mtime_ns)
                && old.attempt_parser_version == Some(parser_version)
                && !old
                    .attempt_error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("cannot open validated audio file:"))
        });
        if !force && already_attempted {
            continue;
        }

        let parser: Option<MetadataParser> = if is_wav(&file.relative) {
            Some(metadata::parse_wav)
        } else if is_aiff(&file.relative) {
            Some(metadata::parse_aiff)
        } else if is_aifc(&file.relative) {
            Some(metadata::parse_aifc)
        } else if is_aac(&file.relative) {
            Some(metadata::parse_aac)
        } else if is_ape(&file.relative) {
            Some(metadata::parse_ape)
        } else if is_flac(&file.relative) {
            Some(metadata::parse_flac)
        } else if is_mp3(&file.relative) {
            Some(metadata::parse_mp3)
        } else if is_mp4_candidate(&file.relative) {
            Some(metadata::parse_mp4)
        } else if is_ogg_candidate(&file.relative) {
            Some(metadata::parse_ogg)
        } else if is_dsf(&file.relative) {
            Some(metadata::parse_dsf)
        } else if is_dff(&file.relative) {
            Some(metadata::parse_dff)
        } else if is_wavpack(&file.relative) {
            Some(metadata::parse_wavpack)
        } else {
            None
        };
        let attempt = if let Some(parser) = parser {
            work.metrics.files_opened_for_extraction += 1;
            work.metrics.files_processed_by_parser += 1;
            match open_file(root, file, parser) {
                Ok(attempt) => attempt,
                // Only failure to obtain a readable handle for an already
                // opened, validated regular-file descriptor is recoverable.
                // Enumeration races, symlinks, identity changes, and root or
                // directory integrity failures still abort the whole scan.
                Err(error) if error.starts_with("cannot open validated audio file:") => {
                    // The parser never received a reader in this branch, so
                    // the counters describe actual opens and parses.
                    work.metrics.files_opened_for_extraction -= 1;
                    work.metrics.files_processed_by_parser -= 1;
                    metadata::failed(extension, error)
                }
                Err(error) => return Err(error),
            }
        } else {
            metadata::unsupported(extension)
        };
        work.metrics.parsing_duration_us = work
            .metrics
            .parsing_duration_us
            .saturating_add(attempt.duration_us);
        match attempt.status {
            MetadataStatus::ParsedWithTags => work.metrics.successful_extractions += 1,
            MetadataStatus::ParsedWithoutTags => {
                work.metrics.successful_extractions += 1;
                work.metrics.files_without_tags += 1;
            }
            MetadataStatus::Unsupported => work.metrics.unsupported_formats += 1,
            MetadataStatus::Failed => work.metrics.extraction_failures += 1,
        }
        work.updates.push(MetadataUpdate {
            relative: file.relative.clone(),
            size: file.size,
            mtime_ns: file.mtime_ns,
            parser_version,
            attempt,
        });
        let processed = index + 1;
        if processed % 128 == 0 || processed == files.len() {
            progress(processed, files.len(), &work.updates)?;
        }
    }
    work.metrics.metadata_records_written = work.updates.len();
    Ok(work)
}

fn write_metadata_update(tx: &Tx<'_>, source_id: i64, update: &MetadataUpdate) -> Result<()> {
    let attempt = &update.attempt;
    let successful = matches!(
        attempt.status,
        MetadataStatus::ParsedWithTags | MetadataStatus::ParsedWithoutTags
    );
    let values = attempt.values.as_ref();
    let track_artists = normalized_artist_values(
        values
            .map(|value| value.track_artists.as_slice())
            .unwrap_or_default(),
    );
    let album_artists = normalized_artist_values(
        values
            .map(|value| value.album_artists.as_slice())
            .unwrap_or_default(),
    );
    let relative_blob = path_codec::encode_relative(&update.relative)?;
    tx.execute(
        "INSERT INTO track_metadata(\
            source_id,relative_path,attempt_status,attempt_size,attempt_mtime_ns,\
            attempt_parser_version,file_format,error,success_size,success_mtime_ns,\
            title,track_artist,album_artist,album,genre,date_text,track_number,disc_number,\
            duration_ms,sample_rate,channels) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
         ON CONFLICT(source_id,relative_path) DO UPDATE SET \
            attempt_status=excluded.attempt_status,attempt_size=excluded.attempt_size,\
            attempt_mtime_ns=excluded.attempt_mtime_ns,\
            attempt_parser_version=excluded.attempt_parser_version,file_format=excluded.file_format,\
            error=excluded.error,\
            success_size=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.success_size ELSE track_metadata.success_size END,\
            success_mtime_ns=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.success_mtime_ns ELSE track_metadata.success_mtime_ns END,\
            title=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.title ELSE track_metadata.title END,\
            track_artist=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.track_artist ELSE track_metadata.track_artist END,\
            album_artist=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.album_artist ELSE track_metadata.album_artist END,\
            album=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.album ELSE track_metadata.album END,\
            genre=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.genre ELSE track_metadata.genre END,\
            date_text=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.date_text ELSE track_metadata.date_text END,\
            track_number=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.track_number ELSE track_metadata.track_number END,\
            disc_number=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.disc_number ELSE track_metadata.disc_number END,\
            duration_ms=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.duration_ms ELSE track_metadata.duration_ms END,\
            sample_rate=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.sample_rate ELSE track_metadata.sample_rate END,\
            channels=CASE WHEN excluded.attempt_status IN (1,2) THEN excluded.channels ELSE track_metadata.channels END",
        &[
            val_i(source_id),
            val_b(relative_blob.clone()),
            val_i(attempt.status as i64),
            val_i(update.size),
            val_i(update.mtime_ns),
            val_i(update.parser_version),
            val_t(&attempt.format),
            val_opt_t(attempt.error.as_deref()),
            val_opt_i(successful.then_some(update.size)),
            val_opt_i(successful.then_some(update.mtime_ns)),
            val_opt_t(values.and_then(|value| value.title.as_deref())),
            val_opt_t(track_artists.first().map(String::as_str)),
            val_opt_t(album_artists.first().map(String::as_str)),
            val_opt_t(values.and_then(|value| value.album.as_deref())),
            val_opt_t(values.and_then(|value| value.genre.as_deref())),
            val_opt_t(values.and_then(|value| value.date_text.as_deref())),
            val_opt_i(values.and_then(|value| value.track_number)),
            val_opt_i(values.and_then(|value| value.disc_number)),
            val_opt_i(values.and_then(|value| value.duration_ms)),
            val_opt_i(values.and_then(|value| value.sample_rate)),
            val_opt_i(values.and_then(|value| value.channels)),
        ],
    )?;

    if successful {
        tx.execute(
            "DELETE FROM track_metadata_extra_artists WHERE source_id=? AND relative_path=?",
            &[val_i(source_id), val_b(relative_blob.clone())],
        )?;
        for (role, artists) in [(1_i64, &track_artists), (2_i64, &album_artists)] {
            for (index, artist) in artists.iter().enumerate().skip(1) {
                tx.execute(
                    "INSERT INTO track_metadata_extra_artists(source_id,relative_path,artist_role,ordinal,value) VALUES(?,?,?,?,?)",
                    &[
                        val_i(source_id),
                        val_b(relative_blob.clone()),
                        val_i(role),
                        val_i(i64::try_from(index + 1).unwrap_or(i64::MAX)),
                        val_t(artist),
                    ],
                )?;
            }
        }
    }
    Ok(())
}

fn normalized_artist_values(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && seen.insert((*value).to_owned()))
        .map(str::to_owned)
        .collect()
}

fn artist_values(
    connection: &Connection,
    source_id: i64,
    relative_path: &[u8],
    role: i64,
    first: Option<String>,
) -> Result<Vec<String>> {
    let rows = query(
        connection,
        "SELECT value FROM track_metadata_extra_artists \
         WHERE source_id=? AND relative_path=? AND artist_role=? ORDER BY ordinal",
        &[val_i(source_id), val_b(relative_path.to_vec()), val_i(role)],
    )?;
    let values = first
        .into_iter()
        .chain(rows.iter().map(|row| text(row, 0)))
        .collect::<Vec<_>>();
    Ok(normalized_artist_values(&values))
}

fn grouping_text(value: &str) -> String {
    value.trim().to_lowercase()
}

fn normalized_artist_set(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| grouping_text(value))
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn disc_folder_number(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    let suffix = ["disc", "disk", "cd"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))?;
    let suffix = if matches!(suffix.as_bytes().first(), Some(b' ' | b'-' | b'_')) {
        &suffix[1..]
    } else {
        suffix
    };
    let digit_count = suffix.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let (digits, remainder) = suffix.split_at(digit_count);
    let number = digits.parse::<u32>().ok().filter(|number| *number > 0)?;
    if remainder.is_empty() {
        return Some(number);
    }
    let separated = remainder.starts_with(char::is_whitespace)
        || remainder.starts_with(['(', '[', '-', '–', '—', ':']);
    (separated && remainder.chars().any(char::is_alphabetic)).then_some(number)
}

/// Port of the long-standing Local library title rule. It strips only a
/// terminal disc marker whose number agrees with the sibling disc folder.
fn multidisc_base_title(album_title: &str, disc_number: u32) -> Option<String> {
    let title = album_title.trim();
    if let Some(without_close) = title.strip_suffix(')') {
        if let Some(open_index) = without_close.rfind('(') {
            let base = without_close[..open_index].trim_end();
            let marker = without_close[open_index + 1..].trim();
            if !base.is_empty() && !marker.is_empty() {
                let expanded = format!("{base} {marker}");
                match multidisc_base_title(&expanded, disc_number) {
                    None => return None,
                    Some(parsed) if parsed == base => return Some(parsed),
                    _ => {}
                }
            }
        }
    }

    for (index, ch) in title.char_indices().rev() {
        if !ch.is_whitespace() {
            continue;
        }
        let suffix = title[index..].trim_start();
        let lower = suffix.to_ascii_lowercase();
        let Some(after_prefix) = ["disco", "disc", "disk", "cd"]
            .iter()
            .find_map(|prefix| lower.strip_prefix(prefix))
        else {
            continue;
        };
        let after_prefix = if matches!(
            after_prefix.as_bytes().first(),
            Some(b' ' | b'.' | b'-' | b'_')
        ) {
            &after_prefix[1..]
        } else {
            after_prefix
        };
        let digit_count = after_prefix.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            continue;
        }
        let (digits, remainder) = after_prefix.split_at(digit_count);
        let descriptor = remainder.trim_start();
        let valid_descriptor = if descriptor.is_empty() {
            true
        } else if let Some(inner) = descriptor
            .strip_prefix('(')
            .and_then(|text| text.strip_suffix(')'))
        {
            !inner.trim().is_empty()
        } else if let Some(inner) = descriptor
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
        {
            !inner.trim().is_empty()
        } else if let Some(inner) = descriptor.strip_prefix(['-', '–', '—', ':']) {
            !inner.trim().is_empty()
        } else {
            false
        };
        if !valid_descriptor {
            continue;
        }
        let number = digits.parse::<u32>().ok()?;
        if number != disc_number || number == 0 {
            return None;
        }
        let base = title[..index].trim_end();
        let base = match base.char_indices().next_back() {
            Some((separator_index, ',' | '-' | '–' | '—' | ':')) => {
                base[..separator_index].trim_end()
            }
            _ => base,
        };
        return (!base.is_empty()).then(|| base.to_string());
    }
    Some(title.to_string())
}

/// Infer a disc number from the immediate parent directory using the same
/// conservative naming rule as album consolidation. Explicit disc tags take
/// precedence at call sites.
pub fn disc_number_from_path(relative_path: &Path) -> Option<u32> {
    relative_path
        .parent()?
        .file_name()?
        .to_str()
        .and_then(disc_folder_number)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AlbumBaseKey {
    title: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct AlbumGroupKey {
    base: AlbumBaseKey,
    directory: PathBuf,
}

struct GroupedAlbum {
    album: Album,
    track_indexes: Vec<usize>,
}

fn append_identity_component(identity: &mut Vec<u8>, component: &[u8]) {
    identity.extend_from_slice(&(component.len() as u64).to_le_bytes());
    identity.extend_from_slice(component);
}

fn album_id(source_id: i64, key: &AlbumGroupKey) -> Result<AlbumId> {
    // Grouping semantics changed to match the mature Local catalog rules.
    // A domain version prevents identities from accidentally aliasing albums
    // constructed by older derived-key policies.
    let mut identity = b"sonante-derived-album-v2".to_vec();
    append_identity_component(&mut identity, key.base.title.as_bytes());
    let directory = if key.directory.as_os_str().is_empty() {
        Vec::new()
    } else {
        path_codec::encode_relative(&key.directory)?
    };
    append_identity_component(&mut identity, &directory);
    Ok(AlbumId {
        source_id,
        identity,
    })
}

fn group_albums(tracks: &[AlbumTrack]) -> Result<Vec<GroupedAlbum>> {
    let mut groups = BTreeMap::<AlbumGroupKey, Vec<usize>>::new();
    let mut directories_by_base = BTreeMap::<AlbumBaseKey, BTreeSet<PathBuf>>::new();
    for (index, track) in tracks.iter().enumerate() {
        let values = track
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.last_success.as_ref());
        let directory = track
            .track
            .relative_path
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .to_path_buf();
        // The previous Local implementation displayed folder-only releases
        // under their physical folder name. Keep that useful behavior while
        // leaving tracks with no successful metadata out of inferred groups.
        let title = values
            .and_then(|values| values.album.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                track
                    .track
                    .relative_path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .filter(|name| !name.trim().is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "Álbum Desconhecido".to_owned());
        let base = AlbumBaseKey {
            title: grouping_text(&title),
        };
        directories_by_base
            .entry(base.clone())
            .or_default()
            .insert(directory.clone());
        let key = AlbumGroupKey { base, directory };
        groups.entry(key).or_default().push(index);
    }

    // Historical Local rule: inside one exact folder, absorb title-tag
    // outliers only when one spelling accounts for at least 80% of its tracks.
    // This is conservative: it never joins separate directories or editions.
    let mut folders = BTreeMap::<PathBuf, Vec<AlbumGroupKey>>::new();
    for key in groups.keys() {
        folders
            .entry(key.directory.clone())
            .or_default()
            .push(key.clone());
    }
    for keys in folders.values_mut() {
        if keys.len() < 2 {
            continue;
        }
        keys.sort();
        let total = keys.iter().map(|key| groups[key].len()).sum::<usize>();
        let Some(dominant) = keys
            .iter()
            .find(|key| groups[*key].len() >= total - total / 5)
            .cloned()
        else {
            continue;
        };
        let mut merged = groups.remove(&dominant).expect("dominant album exists");
        for key in keys.iter().filter(|key| **key != dominant) {
            if let Some(mut variant) = groups.remove(key) {
                merged.append(&mut variant);
            }
        }
        groups.insert(dominant, merged);
    }

    // Merge sibling disc folders only when labels are conventional, disc
    // numbers are unique, and their album titles resolve to the same base.
    let mut disc_candidates = BTreeMap::<PathBuf, Vec<(u32, AlbumGroupKey, String)>>::new();
    for key in groups.keys() {
        let folder = &key.directory;
        let Some((root, label)) = folder.parent().zip(folder.file_name()) else {
            continue;
        };
        let Some(label) = label.to_str() else {
            continue;
        };
        let Some(number) = disc_folder_number(label) else {
            continue;
        };
        let Some(base_title) = multidisc_base_title(&key.base.title, number) else {
            continue;
        };
        disc_candidates
            .entry(root.to_path_buf())
            .or_default()
            .push((number, key.clone(), base_title));
    }
    for (root, mut candidates) in disc_candidates {
        if candidates.len() < 2 {
            continue;
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        if candidates.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            continue;
        }
        let base = grouping_text(&candidates[0].2);
        if candidates
            .iter()
            .any(|(_, _, title)| grouping_text(title) != base)
        {
            continue;
        }
        let merged_key = AlbumGroupKey {
            base: AlbumBaseKey { title: base },
            directory: root,
        };
        // Do not overwrite a real album already located at the parent path.
        if groups.contains_key(&merged_key) {
            continue;
        }
        let mut merged = Vec::new();
        for (_, key, _) in candidates {
            if let Some(mut indexes) = groups.remove(&key) {
                merged.append(&mut indexes);
            }
        }
        if !merged.is_empty() {
            groups.insert(merged_key, merged);
        }
    }

    // Re-evaluate ambiguity after the conservative disc merge so a confirmed
    // CD1/CD2 set does not look like multiple same-name editions merely because
    // its physical tracks occupy sibling directories.
    directories_by_base.clear();
    for key in groups.keys() {
        directories_by_base
            .entry(key.base.clone())
            .or_default()
            .insert(key.directory.clone());
    }

    let mut result = Vec::with_capacity(groups.len());
    for (key, mut indexes) in groups {
        indexes.sort_by(|left, right| {
            tracks[*left]
                .track
                .relative_path
                .cmp(&tracks[*right].track.relative_path)
        });
        let title = indexes
            .iter()
            .filter_map(|index| {
                let title = tracks[*index]
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.last_success.as_ref())
                    .and_then(|values| values.album.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        let parent = tracks[*index]
                            .track
                            .relative_path
                            .parent()
                            .unwrap_or_else(|| Path::new(""));
                        let is_merged_disc = parent != key.directory
                            && parent.parent() == Some(key.directory.as_path());
                        if is_merged_disc {
                            disc_number_from_path(&tracks[*index].track.relative_path)
                                .and_then(|number| multidisc_base_title(value, number))
                                .unwrap_or_else(|| value.to_owned())
                        } else {
                            value.to_owned()
                        }
                    })
                    .or_else(|| {
                        tracks[*index]
                            .track
                            .relative_path
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|name| name.to_str())
                            .map(str::to_owned)
                    });
                let title = title.or_else(|| {
                    tracks[*index]
                        .track
                        .relative_path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .map(str::to_owned)
                });
                title.filter(|title| grouping_text(title) == key.base.title)
            })
            .max()
            .unwrap_or_else(|| {
                if key.base.title == grouping_text("Álbum Desconhecido") {
                    "Álbum Desconhecido".to_owned()
                } else {
                    key.base.title.clone()
                }
            });
        let mut album_artists = Vec::new();
        let mut dates = BTreeSet::new();
        let mut artist_orders = BTreeSet::new();
        let mut tagged_titles = BTreeSet::new();
        let mut used_folder_title = false;
        let mut stale_track_count = 0;
        for index in &indexes {
            let track = &tracks[*index];
            if track.metadata_stale {
                stale_track_count += 1;
            }
            if let Some(values) = track
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.last_success.as_ref())
            {
                if let Some(title) = values
                    .album
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                {
                    let parent = track
                        .track
                        .relative_path
                        .parent()
                        .unwrap_or_else(|| Path::new(""));
                    let normalized = if parent != key.directory
                        && parent.parent() == Some(key.directory.as_path())
                    {
                        disc_number_from_path(&track.track.relative_path)
                            .and_then(|number| multidisc_base_title(title, number))
                            .unwrap_or_else(|| title.to_owned())
                    } else {
                        title.to_owned()
                    };
                    tagged_titles.insert(grouping_text(&normalized));
                } else {
                    used_folder_title = true;
                }
                for artist in &values.album_artists {
                    if !album_artists.contains(artist) {
                        album_artists.push(artist.clone());
                    }
                }
                if let Some(date) = values
                    .date_text
                    .as_deref()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                {
                    dates.insert(date.to_owned());
                }
                let order = values
                    .album_artists
                    .iter()
                    .map(|artist| grouping_text(artist))
                    .filter(|artist| !artist.is_empty())
                    .collect::<Vec<_>>();
                artist_orders.insert(order);
            } else {
                used_folder_title = true;
            }
        }
        let same_title_multiple_directories = directories_by_base
            .get(&key.base)
            .is_some_and(|directories| directories.len() > 1);
        let mut grouping_notes = Vec::new();
        if used_folder_title {
            grouping_notes.push("folder_name_used_for_missing_album_tag".to_owned());
        }
        if tagged_titles.len() > 1 {
            grouping_notes.push("dominant_album_title_variant_selected".to_owned());
        }
        let disc_numbers = indexes
            .iter()
            .filter_map(|index| disc_number_from_path(&tracks[*index].track.relative_path))
            .collect::<BTreeSet<_>>();
        let tracks_in_sibling_disc_folders = indexes.iter().any(|index| {
            let parent = tracks[*index]
                .track
                .relative_path
                .parent()
                .unwrap_or_else(|| Path::new(""));
            parent != key.directory && parent.parent() == Some(key.directory.as_path())
        });
        if tracks_in_sibling_disc_folders && disc_numbers.len() > 1 {
            grouping_notes.push("validated_sibling_disc_folders_consolidated".to_owned());
        }
        if album_artists.is_empty() {
            grouping_notes.push("album_artist_missing".to_owned());
        }
        if artist_orders.len() > 1 {
            grouping_notes.push("album_artist_values_inconsistent".to_owned());
        }
        if dates.len() > 1 {
            grouping_notes.push("date_values_conflict".to_owned());
        }
        if stale_track_count > 0 {
            grouping_notes.push("some_track_metadata_is_stale".to_owned());
        }
        if same_title_multiple_directories {
            grouping_notes.push("same_title_exists_in_multiple_directories".to_owned());
        }
        let ambiguous = album_artists.is_empty()
            || dates.len() > 1
            || artist_orders.len() > 1
            || stale_track_count > 0
            || same_title_multiple_directories;
        result.push(GroupedAlbum {
            album: Album {
                id: album_id(tracks[indexes[0]].track.source_id, &key)?,
                source_id: tracks[indexes[0]].track.source_id,
                title,
                album_artists,
                track_count: indexes.len(),
                first_seen_ns: indexes
                    .iter()
                    .map(|index| tracks[*index].track.first_seen_ns)
                    .collect::<Option<Vec<_>>>()
                    .and_then(|values| values.into_iter().min()),
                stale_track_count,
                date_values: dates.into_iter().collect(),
                ambiguous,
                grouping_notes,
                directory_hint: key.directory,
            },
            track_indexes: indexes,
        });
    }
    result.sort_by(|left, right| {
        grouping_text(&left.album.title)
            .cmp(&grouping_text(&right.album.title))
            .then_with(|| {
                normalized_artist_set(&left.album.album_artists)
                    .cmp(&normalized_artist_set(&right.album.album_artists))
            })
            .then_with(|| left.album.directory_hint.cmp(&right.album.directory_hint))
            .then_with(|| left.album.id.cmp(&right.album.id))
    });
    Ok(result)
}

fn compare_optional_number(left: Option<i64>, right: Option<i64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn artist_key(value: &str) -> String {
    grouping_text(value)
}

#[derive(Default)]
struct ArtistAccumulator {
    spellings: BTreeSet<String>,
    track_paths: BTreeSet<PathBuf>,
    stale_paths: BTreeSet<PathBuf>,
    album_ids: BTreeSet<AlbumId>,
    appears_as_track_artist: bool,
    appears_as_album_artist: bool,
}

fn grouped_artists(source_id: i64, tracks: &[AlbumTrack], albums: &[GroupedAlbum]) -> Vec<Artist> {
    let mut album_by_track = vec![None; tracks.len()];
    for (album_index, album) in albums.iter().enumerate() {
        for index in &album.track_indexes {
            album_by_track[*index] = Some(album_index);
        }
    }
    let mut accumulators = BTreeMap::<String, ArtistAccumulator>::new();

    for (index, track) in tracks.iter().enumerate() {
        let Some(values) = track
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.last_success.as_ref())
        else {
            continue;
        };
        let album_id = album_by_track[index].map(|album_index| &albums[album_index].album.id);
        let unique_track_artists = values
            .track_artists
            .iter()
            .map(|name| (artist_key(name), name.trim().to_owned()))
            .filter(|(key, name)| !key.is_empty() && !name.is_empty())
            .collect::<BTreeMap<_, _>>();
        for (key, spelling) in unique_track_artists {
            let entry = accumulators.entry(key).or_default();
            entry.spellings.insert(spelling);
            entry.track_paths.insert(track.track.relative_path.clone());
            entry.appears_as_track_artist = true;
            if let Some(album_id) = album_id {
                entry.album_ids.insert(album_id.clone());
            }
            if track.metadata_stale {
                entry.stale_paths.insert(track.track.relative_path.clone());
            }
        }
        let unique_album_artists = values
            .album_artists
            .iter()
            .map(|name| (artist_key(name), name.trim().to_owned()))
            .filter(|(key, name)| !key.is_empty() && !name.is_empty())
            .collect::<BTreeMap<_, _>>();
        for (key, spelling) in unique_album_artists {
            let entry = accumulators.entry(key).or_default();
            entry.spellings.insert(spelling);
            entry.appears_as_album_artist = true;
            if let Some(album_id) = album_id {
                entry.album_ids.insert(album_id.clone());
            }
            if track.metadata_stale {
                entry.stale_paths.insert(track.track.relative_path.clone());
            }
        }
    }

    let mut artists = accumulators
        .into_iter()
        .map(|(key, entry)| Artist {
            source_id,
            name: entry.spellings.into_iter().next().unwrap_or(key),
            appears_as_track_artist: entry.appears_as_track_artist,
            appears_as_album_artist: entry.appears_as_album_artist,
            track_count: entry.track_paths.len(),
            album_count: entry.album_ids.len(),
            stale_track_count: entry.stale_paths.len(),
        })
        .collect::<Vec<_>>();
    artists.sort_by(|left, right| {
        artist_key(&left.name)
            .cmp(&artist_key(&right.name))
            .then_with(|| left.name.cmp(&right.name))
    });
    artists
}

fn compare_track_indexes(tracks: &[AlbumTrack], left: usize, right: usize) -> std::cmp::Ordering {
    let left_values = tracks[left]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.last_success.as_ref());
    let right_values = tracks[right]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.last_success.as_ref());
    compare_optional_number(
        left_values.and_then(|values| values.disc_number),
        right_values.and_then(|values| values.disc_number),
    )
    .then_with(|| {
        compare_optional_number(
            left_values.and_then(|values| values.track_number),
            right_values.and_then(|values| values.track_number),
        )
    })
    .then_with(|| {
        tracks[left]
            .track
            .relative_path
            .cmp(&tracks[right].track.relative_path)
    })
}

pub struct Catalog {
    db: Db,
}

/// Immutable, source-scoped navigation snapshot built from one SQLite read
/// snapshot. It owns its data and performs no filesystem or SQLite access while
/// being browsed. Call [`Catalog::is_source_library_view_current`] when a
/// consumer needs to know whether it still represents the committed source.
#[derive(Debug)]
pub struct SourceLibraryView {
    source_id: i64,
    generation: i64,
    source_root: PathBuf,
    tracks: Vec<AlbumTrack>,
    albums: Vec<Album>,
    artists: Vec<Artist>,
    cue_tracks: Vec<CueVirtualTrack>,
    tracks_by_album: HashMap<AlbumId, Vec<usize>>,
    album_by_track: HashMap<PathBuf, usize>,
    albums_by_artist: BTreeMap<String, Vec<usize>>,
}

impl SourceLibraryView {
    pub fn source_id(&self) -> i64 {
        self.source_id
    }

    pub fn generation(&self) -> i64 {
        self.generation
    }

    pub fn albums(&self) -> &[Album] {
        &self.albums
    }

    /// Resolve an indexed relative track to the exact grouped album snapshot
    /// that owns it. This is identity-based; callers must not infer an album
    /// from title/artist text or a path prefix.
    pub fn album_for_track(&self, relative_path: &Path) -> Option<&Album> {
        self.album_by_track
            .get(relative_path)
            .and_then(|index| self.albums.get(*index))
    }

    /// Resolve a cataloged relative track to a path for an external playback
    /// engine after descriptor-anchored validation. This verifies that the
    /// indexed entry still exists as a regular file below the source root and
    /// retains the indexed size and modification time. The external engine
    /// necessarily opens the returned path later, so callers must hand it off
    /// immediately; this does not eliminate that final path-based TOCTOU window.
    pub fn verified_track_path(&self, relative_path: &Path) -> Result<Option<PathBuf>> {
        let Some(track) = self
            .tracks
            .iter()
            .find(|track| track.track.relative_path == relative_path)
        else {
            return Ok(None);
        };
        scanner::verify_relative_file(
            &self.source_root,
            relative_path,
            track.track.size,
            track.track.mtime_ns,
        )
        .map(Some)
    }

    /// Open a current physical catalog track as a retained descriptor. This
    /// is intended for decoders that can consume an already-open reader and
    /// must not reopen a user path after validation.
    pub fn open_verified_track_file(&self, relative_path: &Path) -> Result<Option<VerifiedTrackFile>> {
        let Some(track) = self.tracks.iter().find(|track| track.track.relative_path == relative_path) else {
            return Ok(None);
        };
        let (file, identity) = scanner::open_verified_file(
            &self.source_root, relative_path, track.track.size, track.track.mtime_ns,
        )?;
        Ok(Some(VerifiedTrackFile { file, identity }))
    }

    pub fn artists(&self) -> &[Artist] {
        &self.artists
    }

    /// CUE-defined virtual segments persisted for this source. These are kept
    /// separate from physical file tracks until a consumer presents an opaque
    /// CUE locator and resolves it through the catalog.
    pub fn cue_tracks(&self) -> &[CueVirtualTrack] {
        &self.cue_tracks
    }

    /// Revalidate a virtual CUE entry before handing its native MPD URI to the
    /// playback engine. The external cue sheet and referenced physical audio
    /// are checked against their full indexed identities.
    pub fn verified_cue_track(
        &self,
        cue_path: &Path,
        track_number: u32,
    ) -> Result<Option<CueVirtualTrack>> {
        let Some(track) = self.cue_tracks.iter().find(|track| {
            track.cue_path == cue_path && track.track_number == track_number
        }) else {
            return Ok(None);
        };
        let sheet = DiskFile {
            relative: track.cue_path.clone(),
            size: track.cue_size,
            mtime_ns: track.cue_mtime_ns,
            device: track.cue_device,
            inode: track.cue_inode,
            ctime_ns: track.cue_ctime_ns,
        };
        scanner::with_open_file(&self.source_root, &sheet, |_| ())?;
        let (Some(device), Some(inode), Some(ctime_ns)) =
            (track.audio_device, track.audio_inode, track.audio_ctime_ns)
        else {
            return Err("CUE audio identity must be refreshed by an explicit source verification".into());
        };
        let audio = DiskFile {
            relative: track.audio_path.clone(),
            size: track.audio_size,
            mtime_ns: track.audio_mtime_ns,
            device,
            inode,
            ctime_ns,
        };
        scanner::with_open_file(&self.source_root, &audio, |_| ())?;
        Ok(Some(track.clone()))
    }

    /// Open the physical audio behind one virtual CUE segment only after the
    /// sheet and audio identities have both been verified. The returned
    /// descriptor remains bound to that physical object for its lifetime.
    pub fn open_verified_cue_audio_file(
        &self,
        cue_path: &Path,
        track_number: u32,
    ) -> Result<Option<(CueVirtualTrack, VerifiedTrackFile)>> {
        let Some(cue) = self.verified_cue_track(cue_path, track_number)? else { return Ok(None) };
        let (file, identity) = scanner::open_verified_file(
            &self.source_root, &cue.audio_path, cue.audio_size, cue.audio_mtime_ns,
        )?;
        if Some(identity.0) != cue.audio_device
            || Some(identity.1) != cue.audio_inode
            || Some(identity.4) != cue.audio_ctime_ns
        {
            return Err("CUE audio identity changed while opening".into());
        }
        let after = self.verified_cue_track(cue_path, track_number)?
            .ok_or_else(|| "CUE track disappeared while opening".to_owned())?;
        if after != cue { return Err("CUE sheet changed while opening".into()); }
        Ok(Some((cue, VerifiedTrackFile { file, identity })))
    }

    /// Read the first recognized image with one of the supplied conventional
    /// names from this album's indexed directories. Reads are descriptor-
    /// anchored, bounded, and never follow symlinks. This does not update the
    /// SQLite catalog or enumerate the source.
    pub fn read_album_artwork(
        &self,
        album_id: &AlbumId,
        names: &[&str],
        max_bytes: usize,
    ) -> Result<Option<AlbumArtwork>> {
        if album_id.source_id != self.source_id {
            return Ok(None);
        }
        let Some(album) = self.albums.iter().find(|album| album.id == *album_id) else {
            return Ok(None);
        };

        let mut directories = Vec::<PathBuf>::new();
        let mut seen = BTreeSet::<PathBuf>::new();
        if seen.insert(album.directory_hint.clone()) {
            directories.push(album.directory_hint.clone());
        }
        if let Some(indexes) = self.tracks_by_album.get(album_id) {
            for index in indexes {
                if let Some(parent) = self.tracks[*index].track.relative_path.parent() {
                    let parent = parent.to_path_buf();
                    if seen.insert(parent.clone()) {
                        directories.push(parent);
                    }
                }
            }
        }

        for directory in directories {
            for name in names {
                let relative = directory.join(name);
                let Some(bytes) =
                    scanner::read_relative_file(&self.source_root, &relative, max_bytes)?
                else {
                    continue;
                };
                if let Some(mime_type) = image_mime_type(&bytes) {
                    return Ok(Some(AlbumArtwork { mime_type, bytes }));
                }
            }
        }
        Ok(None)
    }

    /// Read conventional artwork from a source-relative directory selected by
    /// an already validated catalog record (including a CUE sheet's folder).
    /// The scanner anchors every lookup below the source root and rejects
    /// symlinks; this method does not enumerate or index the directory.
    pub fn read_source_artwork_in_directory(
        &self,
        directory: &Path,
        names: &[&str],
        max_bytes: usize,
    ) -> Result<Option<AlbumArtwork>> {
        if directory.is_absolute()
            || !directory.components().all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err("artwork directory is not a source-relative path".into());
        }
        for name in names {
            let relative = directory.join(name);
            let Some(bytes) = scanner::read_relative_file(&self.source_root, &relative, max_bytes)? else {
                continue;
            };
            if let Some(mime_type) = image_mime_type(&bytes) {
                return Ok(Some(AlbumArtwork { mime_type, bytes }));
            }
        }
        Ok(None)
    }

    /// Read one bounded embedded image from an indexed CUE segment's physical
    /// audio file. Both the CUE sheet and full physical-file identity are
    /// revalidated before parsing; callers remain responsible for a strict
    /// source-size ceiling because tag parsers can materialize pictures.
    pub fn read_cue_embedded_artwork(
        &self,
        cue_path: &Path,
        track_number: u32,
        max_source_bytes: u64,
        max_image_bytes: usize,
    ) -> Result<Option<AlbumArtwork>> {
        let Some(track) = self.verified_cue_track(cue_path, track_number)? else { return Ok(None) };
        if track.audio_size <= 0 || track.audio_size as u64 > max_source_bytes { return Ok(None) }
        let audio = DiskFile {
            relative: track.audio_path,
            size: track.audio_size,
            mtime_ns: track.audio_mtime_ns,
            device: track.audio_device.ok_or("CUE audio identity is unavailable")?,
            inode: track.audio_inode.ok_or("CUE audio identity is unavailable")?,
            ctime_ns: track.audio_ctime_ns.ok_or("CUE audio identity is unavailable")?,
        };
        scanner::with_open_file(&self.source_root, &audio, |reader| {
            metadata::extract_embedded_artwork(reader, max_image_bytes)
        })
    }

    /// Read a bounded embedded album cover from the first few tracks of an
    /// album. Files are opened only through the Linux scanner's anchored
    /// descriptor path, checked against indexed size/mtime, and rechecked
    /// after parsing. The caller supplies a strict input-file ceiling because
    /// Lofty materializes embedded image blocks while decoding tags.
    pub fn read_album_embedded_artwork(
        &self,
        album_id: &AlbumId,
        max_source_bytes: u64,
        max_image_bytes: usize,
    ) -> Result<Option<AlbumArtwork>> {
        if album_id.source_id != self.source_id {
            return Ok(None);
        }
        let Some(indexes) = self.tracks_by_album.get(album_id) else {
            return Ok(None);
        };
        let mut first_error = None;
        for index in indexes.iter().take(8) {
            let track = &self.tracks[*index].track;
            if track.size <= 0 || track.size as u64 > max_source_bytes {
                continue;
            }
            let parsed = scanner::with_verified_file(
                &self.source_root,
                &track.relative_path,
                track.size,
                track.mtime_ns,
                |reader, length| {
                    (length <= max_source_bytes)
                        .then(|| metadata::extract_embedded_artwork(reader, max_image_bytes))
                        .flatten()
                },
            );
            match parsed {
                Ok(Some(artwork)) => return Ok(Some(artwork)),
                Ok(None) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(None)
    }

    /// Return tracks in the same disc/track/path order as the legacy catalog API.
    pub fn tracks_for_album(
        &self,
        album_id: &AlbumId,
    ) -> Option<impl ExactSizeIterator<Item = &AlbumTrack>> {
        self.tracks_by_album
            .get(album_id)
            .map(|indexes| indexes.iter().map(|index| &self.tracks[*index]))
    }

    /// Return albums whose track artist or explicit album artist matches the
    /// existing normalized artist key, in the catalog's deterministic order.
    pub fn albums_for_artist(&self, artist_name: &str) -> impl Iterator<Item = &Album> {
        let requested = artist_key(artist_name);
        self.albums_by_artist
            .get(&requested)
            .into_iter()
            .flat_map(|indexes| indexes.iter().map(|index| &self.albums[*index]))
    }

    /// Search the already-loaded source snapshot without SQLite or filesystem
    /// access. Metadata from an older file signature is excluded; the filename
    /// remains searchable as a physical-catalog fallback.
    pub fn search_tracks(&self, query: &str, limit: usize) -> Vec<&AlbumTrack> {
        let terms = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|term| !term.is_empty())
            .collect::<Vec<_>>();
        if terms.is_empty() || limit == 0 {
            return Vec::new();
        }

        let mut matches = self
            .tracks
            .iter()
            .filter_map(|track| {
                let values = track
                    .metadata
                    .as_ref()
                    .filter(|metadata| metadata.current_for_file)
                    .and_then(|metadata| metadata.last_success.as_ref());
                let title = values
                    .and_then(|values| values.title.as_deref())
                    .or_else(|| {
                        track
                            .track
                            .relative_path
                            .file_name()
                            .and_then(|name| name.to_str())
                    })
                    .unwrap_or("");
                let title_key = title.to_lowercase();
                let mut searchable = title_key.clone();
                if let Some(values) = values {
                    for value in values
                        .track_artists
                        .iter()
                        .chain(values.album_artists.iter())
                        .chain(values.album.iter())
                    {
                        searchable.push(' ');
                        searchable.push_str(&value.to_lowercase());
                    }
                }
                searchable.push(' ');
                searchable.push_str(&track.track.relative_path.to_string_lossy().to_lowercase());
                terms.iter().all(|term| searchable.contains(term)).then(|| {
                    let query_key = query.trim().to_lowercase();
                    let rank = if title_key == query_key {
                        0
                    } else if title_key.starts_with(&query_key) {
                        1
                    } else if title_key.contains(&query_key) {
                        2
                    } else {
                        3
                    };
                    (rank, track)
                })
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_rank, left), (right_rank, right)| {
            left_rank
                .cmp(right_rank)
                .then_with(|| left.track.relative_path.cmp(&right.track.relative_path))
        });
        matches
            .into_iter()
            .take(limit)
            .map(|(_, track)| track)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumArtwork {
    pub mime_type: &'static str,
    pub bytes: Vec<u8>,
}

fn image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

impl Catalog {
    /// Open/create the database and schema only. This never enumerates sources.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(p) = path.as_ref().parent() {
            fs::create_dir_all(p).map_err(|e| format!("cannot create database directory: {e}"))?;
        }
        Ok(Self {
            db: Db::open(path.as_ref())?,
        })
    }

    /// Holds SQLite's cross-connection writer reservation while a caller
    /// performs a short operation that depends on the listed source
    /// generations. Filesystem enumeration must happen before this call.
    /// The immediate transaction prevents a verifier, remover, or other
    /// SQLite connection from replacing those generations until `operation`
    /// returns. If the operation fails, the read-only transaction is dropped
    /// and no catalog state is changed.
    pub fn with_source_generation_guard<T, F>(
        &mut self,
        generations: &[(i64, i64)],
        operation: F,
    ) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        let mut ordered = generations.to_vec();
        ordered.sort_unstable_by_key(|(source_id, _)| *source_id);
        for pair in ordered.windows(2) {
            if pair[0].0 == pair[1].0 && pair[0].1 != pair[1].1 {
                return Err("source generation guard received conflicting generations".into());
            }
        }
        ordered.dedup_by_key(|(source_id, _)| *source_id);

        let transaction = Tx::begin(&mut self.db)?;
        for (source_id, generation) in ordered {
            validate_observed_generation(&transaction.transaction, source_id, generation)?;
        }
        let value = operation()?;
        transaction.commit()?;
        Ok(value)
    }
    #[cfg(test)]
    fn close(self) -> Result<()> {
        self.db.close()
    }
    pub fn add_source(
        &mut self,
        folder: impl AsRef<Path>,
        display_name: impl Into<String>,
    ) -> Result<AddReport> {
        self.add_source_with_progress(folder, display_name, |_| {})
    }

    pub fn add_source_with_progress<F>(
        &mut self,
        folder: impl AsRef<Path>,
        display_name: impl Into<String>,
        mut progress: F,
    ) -> Result<AddReport>
    where
        F: FnMut(IndexProgress),
    {
        let started = Instant::now();
        let root = canonical_dir(folder.as_ref())?;
        validate_non_overlap(&self.db.connection, &root)?;
        let scanned =
            scanner::walk_with_progress(&root, |entries, files_examined, audio_files| {
                progress(IndexProgress {
                    phase: IndexPhase::Scanning,
                    entries_visited: entries,
                    files_examined,
                    audio_files_found: audio_files,
                    metadata_processed: 0,
                    metadata_total: 0,
                    identified_albums: 0,
                    identified_artists: 0,
                    counts_provisional: true,
                });
            })?;
        let cue_work = prepare_cue_work(
            &root,
            None,
            &scanned.cue_files,
            &scanned.files,
            None,
            &self.db.connection,
            false,
        )?;
        let mut latest_preview = IndexPreviewSummary::default();
        let metadata_work = prepare_metadata_with_progress(
            &root,
            &scanned.files,
            &HashMap::new(),
            false,
            |processed, total, updates| {
                let preview = build_index_preview(updates)?;
                latest_preview = preview;
                progress(IndexProgress {
                    phase: IndexPhase::ReadingMetadata,
                    entries_visited: scanned.entries,
                    files_examined: scanned.files_examined,
                    audio_files_found: scanned.files.len(),
                    metadata_processed: processed,
                    metadata_total: total,
                    identified_albums: preview.album_count,
                    identified_artists: preview.artist_count,
                    counts_provisional: true,
                });
                Ok(())
            },
        )?;
        progress(IndexProgress {
            phase: IndexPhase::Grouping,
            entries_visited: scanned.entries,
            files_examined: scanned.files_examined,
            audio_files_found: scanned.files.len(),
            metadata_processed: scanned.files.len(),
            metadata_total: scanned.files.len(),
            identified_albums: latest_preview.album_count,
            identified_artists: latest_preview.artist_count,
            counts_provisional: true,
        });
        let final_preview = build_index_preview(&metadata_work.updates)?;
        let generation = 1;
        let display = display_name.into();
        let path_format = path_codec::native_format()?;
        let path = path_codec::encode_root(&root)?;
        progress(IndexProgress {
            phase: IndexPhase::Persisting,
            entries_visited: scanned.entries,
            files_examined: scanned.files_examined,
            audio_files_found: scanned.files.len(),
            metadata_processed: scanned.files.len(),
            metadata_total: scanned.files.len(),
            identified_albums: final_preview.album_count,
            identified_artists: final_preview.artist_count,
            counts_provisional: true,
        });
        let tx = Tx::begin(&mut self.db)?;
        validate_non_overlap(&tx.transaction, &root)?;
        tx.execute("INSERT INTO sources(path_format,canonical_path,display_name,generation,last_checked_ns) VALUES(?,?,?,?,?)", &[val_i(path_format),val_b(path),val_t(display),val_i(generation),val_i(now_ns())])?;
        let id = tx.last_insert_rowid();
        for f in &scanned.files {
            let relative = path_codec::encode_relative(&f.relative)?;
            tx.execute("INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation,first_seen_ns) VALUES(?,?,?,?,?,?)", &[val_i(id),val_b(relative),val_i(f.size),val_i(f.mtime_ns),val_i(generation),val_i(now_ns())])?;
        }
        for update in &metadata_work.updates {
            write_metadata_update(&tx, id, update)?;
        }
        replace_cue_catalog(&tx, id, &cue_work)?;
        tx.commit()?;
        progress(IndexProgress {
            phase: IndexPhase::Completed,
            entries_visited: scanned.entries,
            files_examined: scanned.files_examined,
            audio_files_found: scanned.files.len(),
            metadata_processed: scanned.files.len(),
            metadata_total: scanned.files.len(),
            identified_albums: final_preview.album_count,
            identified_artists: final_preview.artist_count,
            counts_provisional: false,
        });
        Ok(AddReport {
            source_id: id,
            scan: ScanReport {
                generation,
                metrics: Metrics {
                    sources_examined: 1,
                    entries_visited: scanned.entries,
                    files_examined: scanned.files_examined,
                    files_new: scanned.files.len(),
                    records_written: scanned.files.len() + 1,
                    duration_us: started.elapsed().as_micros(),
                    ..metadata_work.metrics
                },
            },
        })
    }
    pub fn verify_source(&mut self, id: i64) -> Result<ScanReport> {
        self.verify_inner(id, false, false, || {}, || {}, scanner::walk)
    }
    pub fn verify_source_with_progress<F>(&mut self, id: i64, mut progress: F) -> Result<ScanReport>
    where
        F: FnMut(IndexProgress),
    {
        let view = self.source_library_view(id)?;
        let result = self.verify_inner_with_progress(
            id,
            false,
            false,
            || {},
            || {},
            |root, callback| scanner::walk_with_progress(root, callback),
            |mut event| {
                event.identified_albums = view.albums().len();
                event.identified_artists = view.artists().len();
                event.counts_provisional = false;
                progress(event);
            },
        )?;
        let view = self.source_library_view(id)?;
        progress(IndexProgress {
            phase: IndexPhase::Completed,
            entries_visited: result.metrics.entries_visited,
            files_examined: result.metrics.files_examined,
            audio_files_found: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            metadata_processed: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            metadata_total: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            identified_albums: view.albums().len(),
            identified_artists: view.artists().len(),
            counts_provisional: false,
        });
        Ok(result)
    }

    /// Reconcile a source whose last tag edit may have committed on disk while
    /// the catalog update was interrupted. This is explicit and never runs at
    /// catalog open. The pending marker is cleared in the same transaction as
    /// the successful scan result.
    pub fn reconcile_tag_edit_with_progress<F>(
        &mut self,
        id: i64,
        mut progress: F,
    ) -> Result<ScanReport>
    where
        F: FnMut(IndexProgress),
    {
        let result = self.verify_inner_with_progress_mode(
            id,
            false,
            false,
            || {},
            || {},
            |root, callback| scanner::walk_with_progress(root, callback),
            |event| progress(event),
            true,
        )?;
        let view = self.source_library_view(id)?;
        progress(IndexProgress {
            phase: IndexPhase::Completed,
            entries_visited: result.metrics.entries_visited,
            files_examined: result.metrics.files_examined,
            audio_files_found: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            metadata_processed: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            metadata_total: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            identified_albums: view.albums().len(),
            identified_artists: view.artists().len(),
            counts_provisional: false,
        });
        Ok(result)
    }

    pub fn source_tag_edit_pending(&self, source_id: i64) -> Result<bool> {
        let rows = self.db.query(
            "SELECT 1 FROM source_tag_edit_pending WHERE source_id=?",
            &[val_i(source_id)],
        )?;
        Ok(!rows.is_empty())
    }

    /// Persist an edit-in-progress marker before touching the media file. This
    /// prevents startup/navigation from presenting old tags after a crash.
    pub fn begin_source_tag_edit(&mut self, source_id: i64, generation: i64) -> Result<Source> {
        let tx = Tx::begin(&mut self.db)?;
        validate_observed_generation(&tx.transaction, source_id, generation)?;
        if !query(
            &tx.transaction,
            "SELECT 1 FROM source_tag_edit_pending WHERE source_id=?",
            &[val_i(source_id)],
        )?
        .is_empty()
        {
            return Err("source already has a tag edit requiring reconciliation".into());
        }
        tx.execute(
            "INSERT INTO source_tag_edit_pending(source_id,generation,started_ns) VALUES(?,?,?)",
            &[val_i(source_id), val_i(generation), val_i(now_ns())],
        )?;
        let source = source_row(&tx.transaction, source_id)?;
        tx.commit()?;
        Ok(source)
    }

    /// Remove a marker only when no media file was committed. A committed or
    /// uncertain write must be reconciled by a complete source verification.
    pub fn cancel_source_tag_edit(&mut self, source_id: i64, generation: i64) -> Result<()> {
        let tx = Tx::begin(&mut self.db)?;
        tx.execute(
            "DELETE FROM source_tag_edit_pending WHERE source_id=? AND generation=?",
            &[val_i(source_id), val_i(generation)],
        )?;
        tx.commit()
    }

    /// Read a catalog member through the secure scanner while binding the
    /// access to the exact source generation and indexed relative path.
    pub fn with_verified_source_track_file<T, F>(
        &self,
        source_id: i64,
        generation: i64,
        relative: &Path,
        read: F,
    ) -> Result<T>
    where
        F: FnOnce(BufReader<File>, u64) -> T,
    {
        if self.source_tag_edit_pending(source_id)? {
            return Err("source requires explicit reconciliation after a tag edit".into());
        }
        let source = source_row(&self.db.connection, source_id)?;
        validate_observed_generation(&self.db.connection, source_id, generation)?;
        let track = source_track_row(&self.db.connection, source_id, relative)?;
        let value = scanner::with_verified_file(
            &source.canonical_path,
            relative,
            track.size,
            track.mtime_ns,
            read,
        )?;
        validate_observed_generation(&self.db.connection, source_id, generation)?;
        Ok(value)
    }

    /// Check for a sibling CUE sheet without following paths outside the
    /// indexed source. A CUE in the same directory is conservatively treated
    /// as making the physical file non-editable by the album editor.
    pub fn source_track_directory_has_cue(
        &self,
        source_id: i64,
        generation: i64,
        relative: &Path,
    ) -> Result<bool> {
        if self.source_tag_edit_pending(source_id)? {
            return Err("source requires explicit reconciliation after a tag edit".into());
        }
        let source = source_row(&self.db.connection, source_id)?;
        validate_observed_generation(&self.db.connection, source_id, generation)?;
        let _track = source_track_row(&self.db.connection, source_id, relative)?;
        let has_cue = scanner::directory_has_cue(&source.canonical_path, relative)?;
        validate_observed_generation(&self.db.connection, source_id, generation)?;
        Ok(has_cue)
    }

    /// Atomically replace one exact catalog member after an edit intent has
    /// been committed. SQLite generation and file membership are rechecked
    /// before the scanner performs its descriptor-anchored rename.
    pub fn replace_source_track_file<T, F>(
        &mut self,
        source_id: i64,
        generation: i64,
        relative: &Path,
        transform: F,
    ) -> std::result::Result<TrackFileReplaceSuccess<T>, TrackFileReplaceFailure>
    where
        F: FnOnce(&mut File, &mut File, &std::fs::Metadata) -> std::result::Result<T, String>,
    {
        let source = source_row(&self.db.connection, source_id)
            .map_err(|message| TrackFileReplaceFailure { message, committed: false })?;
        validate_observed_generation(&self.db.connection, source_id, generation)
            .map_err(|message| TrackFileReplaceFailure { message, committed: false })?;
        let pending = self
            .source_tag_edit_pending(source_id)
            .map_err(|message| TrackFileReplaceFailure { message, committed: false })?;
        if !pending {
            return Err(TrackFileReplaceFailure {
                message: "tag edit intent is not active for this source".into(),
                committed: false,
            });
        }
        let track = source_track_row(&self.db.connection, source_id, relative)
            .map_err(|message| TrackFileReplaceFailure { message, committed: false })?;
        let result = scanner::replace_verified_file(
            &source.canonical_path,
            relative,
            track.size,
            track.mtime_ns,
            transform,
        )?;
        if let Err(message) = validate_observed_generation(&self.db.connection, source_id, generation) {
            return Err(TrackFileReplaceFailure { message, committed: true });
        }
        Ok(result)
    }
    /// Force-refresh metadata for one source; the previous committed rows remain until commit.
    pub fn rebuild_source(&mut self, id: i64) -> Result<ScanReport> {
        self.verify_inner(id, true, false, || {}, || {}, scanner::walk)
    }
    /// Force-refresh metadata for one source while reporting the same progress
    /// phases as an ordinary verification. The previous committed generation
    /// remains readable until the new scan is committed atomically.
    pub fn rebuild_source_with_progress<F>(
        &mut self,
        id: i64,
        mut progress: F,
    ) -> Result<ScanReport>
    where
        F: FnMut(IndexProgress),
    {
        let view = self.source_library_view(id)?;
        let result = self.verify_inner_with_progress(
            id,
            true,
            false,
            || {},
            || {},
            |root, callback| scanner::walk_with_progress(root, callback),
            |mut event| {
                event.identified_albums = view.albums().len();
                event.identified_artists = view.artists().len();
                event.counts_provisional = true;
                progress(event);
            },
        )?;
        let view = self.source_library_view(id)?;
        progress(IndexProgress {
            phase: IndexPhase::Completed,
            entries_visited: result.metrics.entries_visited,
            files_examined: result.metrics.files_examined,
            audio_files_found: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            metadata_processed: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            metadata_total: result.metrics.files_new
                + result.metrics.files_modified
                + result.metrics.files_unchanged,
            identified_albums: view.albums().len(),
            identified_artists: view.artists().len(),
            counts_provisional: false,
        });
        Ok(result)
    }
    #[cfg(test)]
    fn verify_with_interruption(&mut self, id: i64) -> Result<ScanReport> {
        self.verify_inner(id, false, true, || {}, || {}, scanner::walk)
    }
    #[cfg(test)]
    fn verify_with_hook<F: FnOnce()>(&mut self, id: i64, after_scan: F) -> Result<ScanReport> {
        self.verify_inner(id, false, false, after_scan, || {}, scanner::walk)
    }
    #[cfg(test)]
    fn verify_with_transaction_hook<F>(&mut self, id: i64, before_commit: F) -> Result<ScanReport>
    where
        F: FnOnce(),
    {
        self.verify_inner(id, false, false, || {}, before_commit, scanner::walk)
    }
    #[cfg(all(test, target_os = "linux"))]
    fn verify_with_walk_hook<F: FnMut(&Path)>(&mut self, id: i64, hook: F) -> Result<ScanReport> {
        self.verify_inner(
            id,
            false,
            false,
            || {},
            || {},
            |root| scanner::walk_with_hook(root, hook),
        )
    }
    #[cfg(all(test, target_os = "linux"))]
    fn verify_with_scan_failure_after(&mut self, id: i64, entries: usize) -> Result<ScanReport> {
        self.verify_inner(
            id,
            false,
            false,
            || {},
            || {},
            |root| scanner::walk_with_failure_after(root, entries),
        )
    }
    #[cfg(all(test, target_os = "linux"))]
    fn verify_with_metadata_failure(&mut self, id: i64, relative: &Path) -> Result<ScanReport> {
        self.verify_inner(
            id,
            false,
            false,
            || {},
            || {},
            |root| scanner::walk_with_metadata_failure(root, relative),
        )
    }
    fn verify_inner<F: FnOnce(), C: FnOnce(), W: FnOnce(&Path) -> Result<WalkResult>>(
        &mut self,
        id: i64,
        force: bool,
        interrupt: bool,
        after_scan: F,
        before_commit: C,
        scan: W,
    ) -> Result<ScanReport> {
        self.verify_inner_with_progress(
            id,
            force,
            interrupt,
            after_scan,
            before_commit,
            |root, _| scan(root),
            |_| {},
        )
    }
    fn verify_inner_with_progress<F, C, W, P>(
        &mut self,
        id: i64,
        force: bool,
        interrupt: bool,
        after_scan: F,
        before_commit: C,
        scan: W,
        progress: P,
    ) -> Result<ScanReport>
    where
        F: FnOnce(),
        C: FnOnce(),
        W: FnOnce(&Path, &mut dyn FnMut(usize, usize, usize)) -> Result<WalkResult>,
        P: FnMut(IndexProgress),
    {
        self.verify_inner_with_progress_mode(
            id,
            force,
            interrupt,
            after_scan,
            before_commit,
            scan,
            progress,
            false,
        )
    }

    fn verify_inner_with_progress_mode<F, C, W, P>(
        &mut self,
        id: i64,
        force: bool,
        interrupt: bool,
        after_scan: F,
        before_commit: C,
        scan: W,
        mut progress: P,
        reconcile_pending: bool,
    ) -> Result<ScanReport>
    where
        F: FnOnce(),
        C: FnOnce(),
        W: FnOnce(&Path, &mut dyn FnMut(usize, usize, usize)) -> Result<WalkResult>,
        P: FnMut(IndexProgress),
    {
        let started = Instant::now();
        // This read captures the version the filesystem scan is based on.
        let source = source_row(&self.db.connection, id)?;
        let pending = self.source_tag_edit_pending(id)?;
        if pending != reconcile_pending {
            return Err(if pending {
                "source has a tag edit requiring explicit reconciliation".into()
            } else {
                "source has no pending tag edit to reconcile".into()
            });
        }
        let root = canonical_dir(&source.canonical_path)?;
        if root != source.canonical_path {
            return Err("source identity changed; explicit repair required".into());
        }
        // Filesystem I/O intentionally remains outside the SQLite write transaction.
        let scanned = scan(&root, &mut |entries, files_examined, audio_files_found| {
            progress(IndexProgress {
                phase: IndexPhase::Scanning,
                entries_visited: entries,
                files_examined,
                audio_files_found,
                metadata_processed: 0,
                metadata_total: 0,
                identified_albums: 0,
                identified_artists: 0,
                counts_provisional: false,
            });
        })?;
        let old = existing_tracks(&self.db.connection, id)?;
        let previous_audio_paths = old.keys().cloned().collect::<std::collections::HashSet<_>>();
        let cue_work = prepare_cue_work(
            &root,
            Some(id),
            &scanned.cue_files,
            &scanned.files,
            Some(&previous_audio_paths),
            &self.db.connection,
            force,
        )?;
        let metadata_work = prepare_metadata_with_progress(
            &root,
            &scanned.files,
            &old,
            force,
            |processed, total, _updates| {
                progress(IndexProgress {
                    phase: IndexPhase::ReadingMetadata,
                    entries_visited: scanned.entries,
                    files_examined: scanned.files_examined,
                    audio_files_found: scanned.files.len(),
                    metadata_processed: processed,
                    metadata_total: total,
                    identified_albums: 0,
                    identified_artists: 0,
                    counts_provisional: false,
                });
                Ok(())
            },
        )?;
        progress(IndexProgress {
            phase: IndexPhase::Grouping,
            entries_visited: scanned.entries,
            files_examined: scanned.files_examined,
            audio_files_found: scanned.files.len(),
            metadata_processed: scanned.files.len(),
            metadata_total: scanned.files.len(),
            identified_albums: 0,
            identified_artists: 0,
            counts_provisional: false,
        });
        let found: std::collections::HashSet<PathBuf> =
            scanned.files.iter().map(|f| f.relative.clone()).collect();
        let mut new = 0;
        let mut modified = 0;
        let mut unchanged = 0;
        for f in &scanned.files {
            match old.get(&f.relative) {
                None => new += 1,
                Some(previous) if previous.size == f.size && previous.mtime_ns == f.mtime_ns => {
                    unchanged += 1
                }
                _ => modified += 1,
            }
        }
        let removed = old.keys().filter(|p| !found.contains(*p)).count();
        after_scan();

        if !force && new == 0 && modified == 0 && removed == 0 && metadata_work.updates.is_empty() && !cue_work.changed {
            progress(IndexProgress {
                phase: IndexPhase::Persisting,
                entries_visited: scanned.entries,
                files_examined: scanned.files_examined,
                audio_files_found: scanned.files.len(),
                metadata_processed: scanned.files.len(),
                metadata_total: scanned.files.len(),
                identified_albums: 0,
                identified_artists: 0,
                counts_provisional: false,
            });
            let tx = Tx::begin(&mut self.db)?;
            validate_observed_generation(&tx.transaction, id, source.generation)?;
            validate_tag_edit_pending(&tx.transaction, id, reconcile_pending)?;
            tx.execute(
                "UPDATE sources SET last_checked_ns=? WHERE id=? AND generation=?",
                &[val_i(now_ns()), val_i(id), val_i(source.generation)],
            )?;
            if reconcile_pending {
                tx.execute(
                    "DELETE FROM source_tag_edit_pending WHERE source_id=? AND generation=?",
                    &[val_i(id), val_i(source.generation)],
                )?;
            }
            tx.commit()?;
            return Ok(ScanReport {
                generation: source.generation,
                metrics: Metrics {
                    sources_examined: 1,
                    entries_visited: scanned.entries,
                    files_examined: scanned.files_examined,
                    files_unchanged: unchanged,
                    records_written: 1,
                    duration_us: started.elapsed().as_micros(),
                    ..Metrics::default()
                },
            });
        }

        progress(IndexProgress {
            phase: IndexPhase::Persisting,
            entries_visited: scanned.entries,
            files_examined: scanned.files_examined,
            audio_files_found: scanned.files.len(),
            metadata_processed: scanned.files.len(),
            metadata_total: scanned.files.len(),
            identified_albums: 0,
            identified_artists: 0,
            counts_provisional: false,
        });
        let generation = source
            .generation
            .checked_add(1)
            .ok_or("generation overflow")?;
        let tx = Tx::begin(&mut self.db)?;
        // BEGIN IMMEDIATE serializes writers across independent SQLite connections.
        // Re-read the source under that lock, so stale scans cannot commit after a
        // newer verification or after a concurrent source removal.
        validate_observed_generation(&tx.transaction, id, source.generation)?;
        validate_tag_edit_pending(&tx.transaction, id, reconcile_pending)?;
        for f in &scanned.files {
            let unchanged = old
                .get(&f.relative)
                .is_some_and(|previous| previous.size == f.size && previous.mtime_ns == f.mtime_ns);
            if force || !unchanged {
                let relative = path_codec::encode_relative(&f.relative)?;
                tx.execute("INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation,first_seen_ns) VALUES(?,?,?,?,?,?) ON CONFLICT(source_id,relative_path) DO UPDATE SET size=excluded.size,mtime_ns=excluded.mtime_ns,generation=excluded.generation", &[val_i(id),val_b(relative),val_i(f.size),val_i(f.mtime_ns),val_i(generation),val_i(now_ns())])?;
            }
        }
        for update in &metadata_work.updates {
            write_metadata_update(&tx, id, update)?;
        }
        replace_cue_catalog(&tx, id, &cue_work)?;
        for p in old.keys().filter(|p| !found.contains(*p)) {
            tx.execute(
                "DELETE FROM tracks WHERE source_id=? AND relative_path=?",
                &[val_i(id), val_b(path_codec::encode_relative(Path::new(p))?)],
            )?;
        }
        if interrupt {
            return Err("simulated interruption before transaction commit".into());
        }
        before_commit();
        tx.execute(
            "UPDATE sources SET generation=?,last_checked_ns=? WHERE id=? AND generation=?",
            &[
                val_i(generation),
                val_i(now_ns()),
                val_i(id),
                val_i(source.generation),
            ],
        )?;
        if reconcile_pending {
            tx.execute(
                "DELETE FROM source_tag_edit_pending WHERE source_id=? AND generation=?",
                &[val_i(id), val_i(source.generation)],
            )?;
        }
        tx.commit()?;
        let writes = if force {
            scanned.files.len()
        } else {
            new + modified
        };
        Ok(ScanReport {
            generation,
            metrics: Metrics {
                sources_examined: 1,
                entries_visited: scanned.entries,
                files_examined: scanned.files_examined,
                files_new: new,
                files_modified: modified,
                files_unchanged: unchanged,
                files_removed: removed,
                records_written: writes + removed + 1,
                duration_us: started.elapsed().as_micros(),
                ..metadata_work.metrics
            },
        })
    }
    pub fn remove_source(&mut self, id: i64) -> Result<usize> {
        if self.source_tag_edit_pending(id)? {
            return Err("source has a tag edit requiring reconciliation".into());
        }
        let tx = Tx::begin(&mut self.db)?;
        // Recheck under BEGIN IMMEDIATE: a tag edit may have installed its
        // persistent intent after the optimistic read above.
        validate_tag_edit_pending(&tx.transaction, id, false)?;
        let n = tx
            .query(
                "SELECT count(*) FROM tracks WHERE source_id=?",
                &[val_i(id)],
            )?
            .first()
            .map(|row| int(row, 0) as usize)
            .unwrap_or(0);
        tx.execute("DELETE FROM sources WHERE id=?", &[val_i(id)])?;
        tx.commit()?;
        Ok(n)
    }
    pub fn sources(&self) -> Result<Vec<Source>> {
        self.db.query("SELECT id,path_format,canonical_path,display_name,generation,last_checked_ns FROM sources ORDER BY id",&[])?.iter().map(|r|Ok(Source{id:int(r,0),canonical_path:path_codec::decode_root(int(r,1),&blob(r,2))?,display_name:text(r,3),generation:int(r,4),last_checked_ns:int(r,5)})).collect()
    }
    pub fn tracks(&self, id: i64) -> Result<Vec<Track>> {
        let mut tracks = self.db.query("SELECT tracks.source_id,sources.path_format,tracks.relative_path,tracks.size,tracks.mtime_ns,tracks.generation,tracks.first_seen_ns FROM tracks JOIN sources ON sources.id=tracks.source_id WHERE tracks.source_id=?",&[val_i(id)])?.iter().map(|r|Ok(Track{source_id:int(r,0),relative_path:path_codec::decode_relative(int(r,1),&blob(r,2))?,size:int(r,3),mtime_ns:int(r,4),generation:int(r,5),first_seen_ns:optional_int(r,6)})).collect::<Result<Vec<_>>>()?;
        tracks.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(tracks)
    }

    /// Read only the exact lowercase `.lrc` sibling of a cataloged physical
    /// track. The audio locator/path is never supplied by an external caller.
    /// Source generation and the audio file identity are checked on both sides
    /// of the bounded sidecar read; all filesystem opens use the scanner's
    /// descriptor-anchored, no-symlink reader.
    pub fn read_verified_track_lrc(
        &self,
        source_id: i64,
        generation: i64,
        relative: &Path,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>> {
        const MAX_LRC_BYTES: usize = 512 * 1024;
        if max_bytes > MAX_LRC_BYTES {
            return Err("LRC read limit exceeds the supported maximum".into());
        }
        if relative.is_absolute()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err("invalid source-relative track identity".into());
        }
        let source = source_row(&self.db.connection, source_id)?;
        validate_observed_generation(&self.db.connection, source_id, generation)?;
        if self.source_tag_edit_pending(source_id)? {
            return Err("source requires reconciliation after a tag edit".into());
        }
        let track = source_track_row(&self.db.connection, source_id, relative)?;
        if track.generation != generation {
            return Err("track does not belong to the requested source generation".into());
        }
        let (audio_before, audio_identity) = scanner::open_verified_file(
            &source.canonical_path,
            relative,
            track.size,
            track.mtime_ns,
        )?;
        let mut sidecar = relative.to_path_buf();
        sidecar.set_extension("lrc");
        if sidecar == relative || sidecar.parent() != relative.parent() {
            return Err("track has no safe sidecar name".into());
        }
        let bytes = scanner::read_relative_file(&source.canonical_path, &sidecar, max_bytes)?;
        let (audio_after, after_identity) = scanner::open_verified_file(
            &source.canonical_path,
            relative,
            track.size,
            track.mtime_ns,
        )?;
        if audio_identity != after_identity
            || !scanner::identity_matches(&audio_before, audio_identity)
            || !scanner::identity_matches(&audio_after, after_identity)
        {
            return Err("cataloged audio identity changed during sidecar read".into());
        }
        validate_observed_generation(&self.db.connection, source_id, generation)?;
        if self.source_tag_edit_pending(source_id)? {
            return Err("source requires reconciliation after a tag edit".into());
        }
        let track_after = source_track_row(&self.db.connection, source_id, relative)?;
        if track_after.generation != track.generation
            || track_after.size != track.size
            || track_after.mtime_ns != track.mtime_ns
        {
            return Err("cataloged track changed during sidecar read".into());
        }
        Ok(bytes)
    }

    pub fn cue_sheets_for_source(&self, source_id: i64) -> Result<Vec<CueSheetRecord>> {
        source_row(&self.db.connection, source_id)?;
        let path_format = path_codec::native_format()?;
        self.db.query("SELECT relative_path,size,mtime_ns,device,inode,ctime_ns,status,error_code,track_count FROM cue_sheets WHERE source_id=? ORDER BY relative_path", &[val_i(source_id)])?
            .iter()
            .map(|row| {
                let count = usize::try_from(int(row, 8)).map_err(|_| "catalog CUE track count is invalid")?;
                let status = match int(row, 6) {
                    1 => CueSheetStatus::Indexed,
                    2 => CueSheetStatus::Rejected { error_code: optional_text(row, 7).ok_or("rejected CUE sheet is missing its error code")? },
                    _ => return Err("catalog contains an unknown CUE sheet status".into()),
                };
                let device = u64::from_be_bytes(blob(row, 3).try_into().map_err(|_| "catalog CUE device identity is invalid")?);
                let inode = u64::from_be_bytes(blob(row, 4).try_into().map_err(|_| "catalog CUE inode identity is invalid")?);
                Ok(CueSheetRecord {
                    source_id,
                    relative_path: path_codec::decode_relative(path_format, &blob(row, 0))?,
                    size: int(row, 1),
                    mtime_ns: int(row, 2),
                    device,
                    inode,
                    ctime_ns: int(row, 5),
                    status,
                    track_count: count,
                })
            })
            .collect()
    }

    pub fn cue_tracks_for_source(&self, source_id: i64) -> Result<Vec<CueVirtualTrack>> {
        source_row(&self.db.connection, source_id)?;
        let rows = self.db.query(
            "SELECT sources.path_format,cue_tracks.cue_path,cue_tracks.track_number,cue_tracks.audio_path,\
                    cue_tracks.start_frame,cue_tracks.end_frame,cue_tracks.title,cue_tracks.album,\
                    cue_tracks.genre,cue_tracks.date_text,cue_tracks.disc_number,tracks.size,tracks.mtime_ns,\
                    CASE WHEN track_metadata.success_size=tracks.size AND track_metadata.success_mtime_ns=tracks.mtime_ns \
                         THEN track_metadata.duration_ms ELSE NULL END,\
                    cue_sheets.size,cue_sheets.mtime_ns,cue_sheets.device,cue_sheets.inode,cue_sheets.ctime_ns,\
                    cue_tracks.audio_device,cue_tracks.audio_inode,cue_tracks.audio_ctime_ns,tracks.first_seen_ns \
             FROM cue_tracks JOIN cue_sheets ON cue_sheets.source_id=cue_tracks.source_id \
                    AND cue_sheets.relative_path=cue_tracks.cue_path AND cue_sheets.status=1 \
             JOIN tracks ON tracks.source_id=cue_tracks.source_id AND tracks.relative_path=cue_tracks.audio_path \
             JOIN sources ON sources.id=cue_tracks.source_id \
             LEFT JOIN track_metadata ON track_metadata.source_id=tracks.source_id \
                    AND track_metadata.relative_path=tracks.relative_path \
             WHERE cue_tracks.source_id=? ORDER BY cue_tracks.cue_path,cue_tracks.track_number",
            &[val_i(source_id)],
        )?;
        let mut artists = HashMap::<(Vec<u8>, i64, i64), Vec<String>>::new();
        for row in self.db.query("SELECT cue_path,track_number,artist_role,value FROM cue_track_artists WHERE source_id=? ORDER BY cue_path,track_number,artist_role,ordinal", &[val_i(source_id)])? {
            artists.entry((blob(&row, 0), int(&row, 1), int(&row, 2))).or_default().push(text(&row, 3));
        }
        let path_format = path_codec::native_format()?;
        rows.iter().map(|row| {
            let cue_blob = blob(row, 1);
            let track_number = u32::try_from(int(row, 2)).map_err(|_| "catalog CUE track number is invalid")?;
            let start_frame = u64::try_from(int(row, 4)).map_err(|_| "catalog CUE start frame is invalid")?;
            let end_frame = optional_int(row, 5).map(u64::try_from).transpose().map_err(|_| "catalog CUE end frame is invalid")?;
            let start_ms = start_frame.checked_mul(1000).ok_or("catalog CUE start frame overflows")? / 75;
            let duration_ms = match end_frame {
                Some(end) => Some(i64::try_from(end.checked_sub(start_frame).ok_or("catalog CUE segment is reversed")?.checked_mul(1000).ok_or("catalog CUE duration overflows")? / 75).map_err(|_| "catalog CUE duration is invalid")?),
                None => optional_int(row, 13).and_then(|total| u64::try_from(total).ok()).and_then(|total| total.checked_sub(start_ms)).map(i64::try_from).transpose().map_err(|_| "catalog CUE duration is invalid")?,
            };
            let cue_device = u64::from_be_bytes(blob(row, 16).try_into().map_err(|_| "catalog CUE device identity is invalid")?);
            let cue_inode = u64::from_be_bytes(blob(row, 17).try_into().map_err(|_| "catalog CUE inode identity is invalid")?);
            let audio_identity = match (row.get(19), row.get(20), optional_int(row, 21)) {
                (Some(Value::Blob(device)), Some(Value::Blob(inode)), Some(ctime_ns)) => Some((
                    u64::from_be_bytes(device.clone().try_into().map_err(|_| "catalog CUE audio device identity is invalid")?),
                    u64::from_be_bytes(inode.clone().try_into().map_err(|_| "catalog CUE audio inode identity is invalid")?),
                    ctime_ns,
                )),
                (Some(Value::Null), Some(Value::Null), None) => None,
                _ => return Err("catalog contains a partial CUE audio identity".into()),
            };
            Ok(CueVirtualTrack {
                source_id,
                cue_path: path_codec::decode_relative(path_format, &cue_blob)?,
                track_number,
                audio_path: path_codec::decode_relative(path_format, &blob(row, 3))?,
                start_frame,
                end_frame,
                duration_ms,
                title: optional_text(row, 6),
                performers: normalized_artist_values(
                    artists
                        .get(&(cue_blob.clone(), int(row, 2), 1))
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                ),
                album: optional_text(row, 7),
                album_performers: normalized_artist_values(
                    artists
                        .get(&(cue_blob, int(row, 2), 2))
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                ),
                genre: optional_text(row, 8),
                date: optional_text(row, 9),
                disc_number: optional_int(row, 10).map(u32::try_from).transpose().map_err(|_| "catalog CUE disc number is invalid")?,
                cue_size: int(row, 14),
                cue_mtime_ns: int(row, 15),
                cue_device,
                cue_inode,
                cue_ctime_ns: int(row, 18),
                audio_size: int(row, 11),
                audio_mtime_ns: int(row, 12),
                audio_device: audio_identity.map(|identity| identity.0),
                audio_inode: audio_identity.map(|identity| identity.1),
                audio_ctime_ns: audio_identity.map(|identity| identity.2),
                first_seen_ns: optional_int(row, 22),
            })
        }).collect()
    }
    pub fn metadata_for_track(
        &self,
        source_id: i64,
        relative_path: impl AsRef<Path>,
    ) -> Result<Option<TrackMetadata>> {
        let relative_blob = path_codec::encode_relative(relative_path.as_ref())?;
        let rows = self.db.query(
            "SELECT tracks.size,tracks.mtime_ns,track_metadata.attempt_status,\
                    track_metadata.file_format,track_metadata.attempt_parser_version,\
                    track_metadata.attempt_size,track_metadata.attempt_mtime_ns,track_metadata.error,\
                    track_metadata.success_size,track_metadata.success_mtime_ns,\
                    track_metadata.title,track_metadata.track_artist,track_metadata.album_artist,\
                    track_metadata.album,track_metadata.genre,track_metadata.date_text,\
                    track_metadata.track_number,track_metadata.disc_number,track_metadata.duration_ms,\
                    track_metadata.sample_rate,track_metadata.channels \
             FROM tracks JOIN track_metadata ON track_metadata.source_id=tracks.source_id \
                 AND track_metadata.relative_path=tracks.relative_path \
             WHERE tracks.source_id=? AND tracks.relative_path=?",
            &[val_i(source_id), val_b(relative_blob.clone())],
        )?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let status = MetadataStatus::from_i64(int(row, 2))
            .ok_or_else(|| "catalog contains an unknown metadata attempt state".to_owned())?;
        let last_success_size = optional_int(row, 8);
        let last_success_mtime_ns = optional_int(row, 9);
        let last_success = if last_success_size.is_some() && last_success_mtime_ns.is_some() {
            Some(TrackMetadataValues {
                title: optional_text(row, 10),
                track_artists: artist_values(
                    &self.db.connection,
                    source_id,
                    &relative_blob,
                    1,
                    optional_text(row, 11),
                )?,
                album_artists: artist_values(
                    &self.db.connection,
                    source_id,
                    &relative_blob,
                    2,
                    optional_text(row, 12),
                )?,
                album: optional_text(row, 13),
                genre: optional_text(row, 14),
                date_text: optional_text(row, 15),
                track_number: optional_int(row, 16),
                disc_number: optional_int(row, 17),
                duration_ms: optional_int(row, 18),
                sample_rate: optional_int(row, 19),
                channels: optional_int(row, 20),
            })
        } else {
            None
        };
        Ok(Some(TrackMetadata {
            status,
            file_format: text(row, 3),
            parser_version: int(row, 4),
            attempted_size: int(row, 5),
            attempted_mtime_ns: int(row, 6),
            error: optional_text(row, 7),
            last_success_size,
            last_success_mtime_ns,
            current_for_file: last_success_size == Some(int(row, 0))
                && last_success_mtime_ns == Some(int(row, 1)),
            last_success,
        }))
    }

    /// Read tracks and all metadata/artists for one source in one SQL statement.
    /// A single SQLite statement observes one consistent read snapshot and
    /// avoids per-track metadata queries. This path touches no filesystem.
    fn browse_batch(&self, source_id: i64) -> Result<BrowseBatch> {
        self.browse_batch_inner(source_id, || {})
    }

    #[cfg(test)]
    fn browse_batch_with_row_hook<F>(
        &self,
        source_id: i64,
        after_first_row: F,
    ) -> Result<BrowseBatch>
    where
        F: FnOnce(),
    {
        let mut hook = Some(after_first_row);
        self.browse_batch_inner(source_id, || {
            if let Some(hook) = hook.take() {
                hook();
            }
        })
    }

    fn browse_batch_inner<F>(&self, source_id: i64, mut after_first_row: F) -> Result<BrowseBatch>
    where
        F: FnMut(),
    {
        #[cfg(not(test))]
        let _ = &mut after_first_row;
        #[cfg(test)]
        let started = Instant::now();
        let mut statement = self
            .db
            .connection
            .prepare(
                "SELECT tracks.source_id,source.path_format,tracks.relative_path,tracks.size,\
                    tracks.mtime_ns,tracks.generation,track_metadata.attempt_status,\
                    track_metadata.file_format,track_metadata.attempt_parser_version,\
                    track_metadata.attempt_size,track_metadata.attempt_mtime_ns,track_metadata.error,\
                    track_metadata.success_size,track_metadata.success_mtime_ns,\
                    track_metadata.title,track_metadata.track_artist,track_metadata.album_artist,\
                    track_metadata.album,track_metadata.genre,track_metadata.date_text,\
                    track_metadata.track_number,track_metadata.disc_number,track_metadata.duration_ms,\
                    track_metadata.sample_rate,track_metadata.channels,\
                    extra.artist_role,extra.ordinal,extra.value,source.generation,tracks.first_seen_ns \
             FROM sources AS source LEFT JOIN tracks ON tracks.source_id=source.id \
             LEFT JOIN track_metadata ON track_metadata.source_id=tracks.source_id \
                 AND track_metadata.relative_path=tracks.relative_path \
             LEFT JOIN track_metadata_extra_artists AS extra \
                 ON extra.source_id=tracks.source_id AND extra.relative_path=tracks.relative_path \
             WHERE source.id=? \
             ORDER BY tracks.relative_path,extra.artist_role,extra.ordinal",
            )
            .map_err(|error| format!("sqlite browse prepare: {error}"))?;
        let column_count = statement.column_count();
        let mut rows = statement
            .query([source_id])
            .map_err(|error| format!("sqlite browse query: {error}"))?;

        let mut tracks = Vec::<AlbumTrack>::new();
        let mut current_blob: Option<Vec<u8>> = None;
        let mut source_generation = None;
        #[cfg(test)]
        let mut rows_read = 0;
        #[cfg(test)]
        let mut invoked_row_hook = false;
        while let Some(sql_row) = rows
            .next()
            .map_err(|error| format!("sqlite browse step: {error}"))?
        {
            let row = (0..column_count)
                .map(|column| sql_row.get::<_, Value>(column))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("sqlite browse row: {error}"))?;
            #[cfg(test)]
            {
                rows_read += 1;
            }
            let row_generation = optional_int(&row, 28)
                .ok_or_else(|| "source disappeared inside browse snapshot".to_owned())?;
            if source_generation.is_some_and(|generation| generation != row_generation) {
                return Err("source generation changed inside browse snapshot".into());
            }
            source_generation = Some(row_generation);
            // LEFT JOIN yields one null-track row for a configured source that
            // currently has no catalog tracks.
            if optional_int(&row, 0).is_none() {
                continue;
            }
            let relative_blob = blob(&row, 2);
            let starts_track = current_blob.as_ref() != Some(&relative_blob);
            if starts_track {
                current_blob = Some(relative_blob.clone());
                let path_format = int(&row, 1);
                let relative_path = path_codec::decode_relative(path_format, &relative_blob)?;
                let size = int(&row, 3);
                let mtime_ns = int(&row, 4);
                let track = Track {
                    source_id: int(&row, 0),
                    relative_path,
                    size,
                    mtime_ns,
                    generation: int(&row, 5),
                    first_seen_ns: optional_int(&row, 29),
                };
                if optional_int(&row, 6).is_some() {
                    let status = MetadataStatus::from_i64(int(&row, 6)).ok_or_else(|| {
                        "catalog contains an unknown metadata attempt state".to_owned()
                    })?;
                    let last_success_size = optional_int(&row, 12);
                    let last_success_mtime_ns = optional_int(&row, 13);
                    let last_success =
                        if last_success_size.is_some() && last_success_mtime_ns.is_some() {
                            Some(TrackMetadataValues {
                                title: optional_text(&row, 14),
                                track_artists: optional_text(&row, 15).into_iter().collect(),
                                album_artists: optional_text(&row, 16).into_iter().collect(),
                                album: optional_text(&row, 17),
                                genre: optional_text(&row, 18),
                                date_text: optional_text(&row, 19),
                                track_number: optional_int(&row, 20),
                                disc_number: optional_int(&row, 21),
                                duration_ms: optional_int(&row, 22),
                                sample_rate: optional_int(&row, 23),
                                channels: optional_int(&row, 24),
                            })
                        } else {
                            None
                        };
                    let current_for_file =
                        last_success_size == Some(size) && last_success_mtime_ns == Some(mtime_ns);
                    let metadata_stale = last_success.is_some()
                        && (!current_for_file
                            || !matches!(
                                status,
                                MetadataStatus::ParsedWithTags | MetadataStatus::ParsedWithoutTags
                            ));
                    let metadata = TrackMetadata {
                        status,
                        file_format: text(&row, 7),
                        parser_version: int(&row, 8),
                        attempted_size: int(&row, 9),
                        attempted_mtime_ns: int(&row, 10),
                        error: optional_text(&row, 11),
                        last_success_size,
                        last_success_mtime_ns,
                        last_success,
                        current_for_file,
                    };
                    tracks.push(AlbumTrack {
                        track,
                        metadata: Some(metadata),
                        metadata_stale,
                    });
                } else {
                    tracks.push(AlbumTrack {
                        track,
                        metadata: None,
                        metadata_stale: false,
                    });
                }
            }

            if let (Some(role), Some(value)) = (optional_int(&row, 25), optional_text(&row, 27)) {
                if let Some(values) = tracks
                    .last_mut()
                    .and_then(|track| track.metadata.as_mut())
                    .and_then(|metadata| metadata.last_success.as_mut())
                {
                    match role {
                        1 => values.track_artists.push(value),
                        2 => values.album_artists.push(value),
                        _ => return Err("catalog contains an unknown extra artist role".into()),
                    }
                }
            }
            #[cfg(test)]
            if !invoked_row_hook {
                invoked_row_hook = true;
                after_first_row();
            }
        }

        for track in &mut tracks {
            if let Some(values) = track
                .metadata
                .as_mut()
                .and_then(|metadata| metadata.last_success.as_mut())
            {
                values.track_artists = normalized_artist_values(&values.track_artists);
                values.album_artists = normalized_artist_values(&values.album_artists);
            }
        }

        Ok(BrowseBatch {
            tracks,
            source_generation,
            #[cfg(test)]
            metrics: BrowseReadMetrics {
                sql_queries: 1,
                rows_read,
                read_duration_us: started.elapsed().as_micros(),
            },
        })
    }

    /// Build one immutable navigation snapshot from a single SQLite statement
    /// snapshot. If the source is changed or removed while the view is being
    /// assembled, construction fails instead of returning a view advertised as
    /// current. Rebuilding is explicit; navigation methods never do it for you.
    pub fn source_library_view(&self, source_id: i64) -> Result<SourceLibraryView> {
        if self.source_tag_edit_pending(source_id)? {
            return Err("source requires explicit reconciliation after a tag edit".into());
        }
        self.source_library_view_inner(source_id, || {})
    }

    #[cfg(test)]
    fn source_library_view_with_row_hook<F>(
        &self,
        source_id: i64,
        after_first_row: F,
    ) -> Result<SourceLibraryView>
    where
        F: FnOnce(),
    {
        let mut hook = Some(after_first_row);
        self.source_library_view_inner(source_id, || {
            if let Some(hook) = hook.take() {
                hook();
            }
        })
    }

    fn source_library_view_inner<F>(
        &self,
        source_id: i64,
        mut after_first_row: F,
    ) -> Result<SourceLibraryView>
    where
        F: FnMut(),
    {
        let source = source_row(&self.db.connection, source_id)?;
        let batch = self.browse_batch_inner(source_id, &mut after_first_row)?;
        let generation = batch
            .source_generation
            .ok_or_else(|| "source not found".to_owned())?;
        let grouped = group_albums(&batch.tracks)?;
        let artists = grouped_artists(source_id, &batch.tracks, &grouped);
        let cue_tracks = self.cue_tracks_for_source(source_id)?;
        let mut albums_by_artist = BTreeMap::<String, BTreeSet<usize>>::new();
        let mut tracks_by_album = HashMap::<AlbumId, Vec<usize>>::new();
        let mut album_by_track = HashMap::<PathBuf, usize>::new();

        for (album_index, group) in grouped.iter().enumerate() {
            let album = &group.album;
            let indexes = tracks_by_album.entry(album.id.clone()).or_default();
            indexes.extend(group.track_indexes.iter().copied());
            for track_index in &group.track_indexes {
                album_by_track.insert(
                    batch.tracks[*track_index].track.relative_path.clone(),
                    album_index,
                );
            }
            for name in &album.album_artists {
                let key = artist_key(name);
                if !key.is_empty() {
                    albums_by_artist.entry(key).or_default().insert(album_index);
                }
            }
            for track_index in &group.track_indexes {
                if let Some(values) = batch.tracks[*track_index]
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.last_success.as_ref())
                {
                    for name in &values.track_artists {
                        let key = artist_key(name);
                        if !key.is_empty() {
                            albums_by_artist.entry(key).or_default().insert(album_index);
                        }
                    }
                }
            }
        }
        for indexes in tracks_by_album.values_mut() {
            indexes.sort_by(|left, right| compare_track_indexes(&batch.tracks, *left, *right));
        }

        let albums = grouped.into_iter().map(|group| group.album).collect();
        let albums_by_artist = albums_by_artist
            .into_iter()
            .map(|(key, indexes)| (key, indexes.into_iter().collect()))
            .collect();
        let view = SourceLibraryView {
            source_id,
            generation,
            source_root: source.canonical_path,
            tracks: batch.tracks,
            albums,
            artists,
            cue_tracks,
            tracks_by_album,
            album_by_track,
            albums_by_artist,
        };
        if !self.is_source_library_view_current(&view)? {
            return Err("source changed while building library view".into());
        }
        Ok(view)
    }

    /// Compare this snapshot's source generation with the persisted generation.
    /// This is a single scalar SQLite read; it does not refresh the view.
    pub fn is_source_library_view_current(&self, view: &SourceLibraryView) -> Result<bool> {
        let rows = self.db.query(
            "SELECT generation FROM sources WHERE id=?",
            &[val_i(view.source_id)],
        )?;
        Ok(rows
            .first()
            .is_some_and(|row| int(row, 0) == view.generation))
    }

    /// Dynamically group albums from committed v4 metadata. No catalog schema
    /// or filesystem scan is involved.
    pub fn albums_for_source(&self, source_id: i64) -> Result<Vec<Album>> {
        let batch = self.browse_batch(source_id)?;
        Ok(group_albums(&batch.tracks)?
            .into_iter()
            .map(|group| group.album)
            .collect())
    }

    #[cfg(test)]
    fn albums_for_source_with_row_hook<F>(
        &self,
        source_id: i64,
        after_first_row: F,
    ) -> Result<Vec<Album>>
    where
        F: FnOnce(),
    {
        let batch = self.browse_batch_with_row_hook(source_id, after_first_row)?;
        Ok(group_albums(&batch.tracks)?
            .into_iter()
            .map(|group| group.album)
            .collect())
    }

    /// List source-local artists with their observed metadata roles and counts.
    pub fn artists_for_source(&self, source_id: i64) -> Result<Vec<Artist>> {
        let batch = self.browse_batch(source_id)?;
        let albums = group_albums(&batch.tracks)?;
        Ok(grouped_artists(source_id, &batch.tracks, &albums))
    }

    /// Return the tracks for a derived album ID. IDs are source-scoped and may
    /// change after metadata or path changes.
    pub fn tracks_for_album(&self, album_id: &AlbumId) -> Result<Vec<AlbumTrack>> {
        let batch = self.browse_batch(album_id.source_id)?;
        let Some(group) = group_albums(&batch.tracks)?
            .into_iter()
            .find(|group| group.album.id == *album_id)
        else {
            return Ok(Vec::new());
        };
        let mut indexes = group.track_indexes;
        indexes.sort_by(|left, right| compare_track_indexes(&batch.tracks, *left, *right));
        let result = indexes
            .into_iter()
            .map(|index| batch.tracks[index].clone())
            .collect::<Vec<_>>();
        Ok(result)
    }

    /// Find albums where the normalized name appears as either a track artist
    /// or an explicit album artist, within this source only.
    pub fn albums_for_artist(&self, source_id: i64, artist_name: &str) -> Result<Vec<Album>> {
        let requested = artist_key(artist_name);
        if requested.is_empty() {
            return Ok(Vec::new());
        }
        let batch = self.browse_batch(source_id)?;
        let mut matches = Vec::new();
        for group in group_albums(&batch.tracks)? {
            let album_match = group
                .album
                .album_artists
                .iter()
                .any(|name| artist_key(name) == requested);
            let track_match = group.track_indexes.iter().any(|index| {
                batch.tracks[*index]
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.last_success.as_ref())
                    .is_some_and(|values| {
                        values
                            .track_artists
                            .iter()
                            .any(|name| artist_key(name) == requested)
                    })
            });
            if album_match || track_match {
                matches.push(group.album);
            }
        }
        Ok(matches)
    }
    pub fn track_count(&self) -> Result<usize> {
        Ok(self
            .db
            .query("SELECT count(*) FROM tracks", &[])?
            .first()
            .map(|r| int(r, 0) as usize)
            .unwrap_or(0))
    }
    pub fn track_count_for(&self, id: i64) -> Result<usize> {
        Ok(self
            .db
            .query(
                "SELECT count(*) FROM tracks WHERE source_id=?",
                &[val_i(id)],
            )?
            .first()
            .map(|r| int(r, 0) as usize)
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::MetadataExt;
    use std::{
        fs::File,
        io::{Read, Write},
        process::{Child, Command, Stdio},
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "sonante-sqlite-catalog-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self, p: &str) -> PathBuf {
            self.0.join(p)
        }
        fn dir(&self, p: &str) -> PathBuf {
            let x = self.path(p);
            fs::create_dir_all(&x).unwrap();
            x
        }
        fn db(&self) -> PathBuf {
            self.path("catalog.sqlite3")
        }
        fn file(p: &Path, contents: &[u8]) {
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap()
            }
            let mut f = File::create(p).unwrap();
            f.write_all(contents).unwrap();
            f.sync_all().unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn make_audio(dir: &Path, n: usize) {
        for i in 0..n {
            Fixture::file(&dir.join(format!("track-{i:05}.flac")), b"synthetic");
        }
    }

    fn wav_chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(id);
        chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
        chunk.extend_from_slice(data);
        if data.len() % 2 != 0 {
            chunk.push(0);
        }
        chunk
    }

    fn pcm_wav(title: Option<&str>) -> Vec<u8> {
        let mut wave = b"WAVE".to_vec();
        let mut fmt = Vec::new();
        fmt.extend(1_u16.to_le_bytes());
        fmt.extend(2_u16.to_le_bytes());
        fmt.extend(44_100_u32.to_le_bytes());
        fmt.extend(176_400_u32.to_le_bytes());
        fmt.extend(4_u16.to_le_bytes());
        fmt.extend(16_u16.to_le_bytes());
        wave.extend(wav_chunk(b"fmt ", &fmt));
        wave.extend(wav_chunk(b"data", &vec![0_u8; 4_410 * 4]));
        if let Some(title) = title {
            let mut info = b"INFO".to_vec();
            let mut text = title.as_bytes().to_vec();
            text.push(0);
            info.extend(wav_chunk(b"INAM", &text));
            let artist = b"Synthetic Artist\0".to_vec();
            info.extend(wav_chunk(b"IART", &artist));
            wave.extend(wav_chunk(b"LIST", &info));
        }
        let mut riff = b"RIFF".to_vec();
        riff.extend((wave.len() as u32).to_le_bytes());
        riff.extend(wave);
        riff
    }

    fn dsdiff_chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut chunk = Vec::new();
        chunk.extend_from_slice(id);
        chunk.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        chunk.extend_from_slice(payload);
        if payload.len() % 2 != 0 {
            chunk.push(0);
        }
        chunk
    }

    fn catalog_dsf_fixture(items: &[(lofty::tag::ItemKey, &str)]) -> Vec<u8> {
        let mp3 = metadata::synthetic_mp3(&[(lofty::tag::TagType::Id3v2, items)], false);
        let tag_size = mp3[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | usize::from(*byte));
        let tag = &mp3[..10 + tag_size];
        let data_len = 12 + 32usize;
        let tag_offset = 80 + data_len;
        let total = tag_offset + tag.len();
        let mut dsf = Vec::new();
        dsf.extend_from_slice(b"DSD ");
        dsf.extend_from_slice(&28u64.to_le_bytes());
        dsf.extend_from_slice(&(total as u64).to_le_bytes());
        dsf.extend_from_slice(&(tag_offset as u64).to_le_bytes());
        dsf.extend_from_slice(b"fmt ");
        dsf.extend_from_slice(&52u64.to_le_bytes());
        dsf.extend_from_slice(&[0; 40]);
        dsf.extend_from_slice(b"data");
        dsf.extend_from_slice(&(data_len as u64).to_le_bytes());
        dsf.resize(80 + data_len, 0);
        dsf.extend_from_slice(tag);
        dsf
    }

    fn catalog_dff_fixture() -> Vec<u8> {
        let mut body = b"DSD ".to_vec();
        body.extend(dsdiff_chunk(b"FVER", &0x01050000u32.to_be_bytes()));
        body.extend(dsdiff_chunk(b"PROP", b"SND "));
        body.extend(dsdiff_chunk(b"DSD ", &[0; 4]));
        let mut info = Vec::new();
        for (id, value) in [(b"DIAR", "Artist DFF"), (b"DITI", "Native DFF")] {
            let mut payload = (value.len() as u32).to_be_bytes().to_vec();
            payload.extend_from_slice(value.as_bytes());
            info.extend(dsdiff_chunk(id, &payload));
        }
        body.extend(dsdiff_chunk(b"DIIN", &info));
        let mut dff = b"FRM8".to_vec();
        dff.extend_from_slice(&(body.len() as u64).to_be_bytes());
        dff.extend(body);
        dff
    }

    fn catalog_id3_bytes(items: &[(lofty::tag::ItemKey, &str)]) -> Vec<u8> {
        let mp3 = metadata::synthetic_mp3(&[(lofty::tag::TagType::Id3v2, items)], false);
        let tag_size = mp3[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | usize::from(*byte));
        mp3[..10 + tag_size].to_vec()
    }

    fn catalog_aac_fixture(items: &[(lofty::tag::ItemKey, &str)]) -> Vec<u8> {
        let mut bytes = catalog_id3_bytes(items);
        let frame = [0xff, 0xf1, 0x50, 0x80, 0x00, 0xe0, 0xfc];
        bytes.extend_from_slice(&frame);
        bytes.extend_from_slice(&frame);
        bytes
    }

    fn catalog_ape_fixture(items: &[(&str, &str)]) -> Vec<u8> {
        let mut encoded_items = Vec::new();
        for (key, value) in items {
            encoded_items.extend_from_slice(&(value.len() as u32).to_le_bytes());
            encoded_items.extend_from_slice(&0u32.to_le_bytes());
            encoded_items.extend_from_slice(key.as_bytes());
            encoded_items.push(0);
            encoded_items.extend_from_slice(value.as_bytes());
        }
        let tag_size = encoded_items.len() as u32 + 32;
        let mut bytes = b"MAC ".to_vec();
        bytes.extend_from_slice(&[0; 32]);
        bytes.extend(encoded_items);
        bytes.extend_from_slice(b"APETAGEX");
        bytes.extend_from_slice(&2000u32.to_le_bytes());
        bytes.extend_from_slice(&tag_size.to_le_bytes());
        bytes.extend_from_slice(&(items.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    #[test]
    fn metadata_open_failure_is_recorded_per_file_and_does_not_abort_sibling_extraction() {
        let fixture = Fixture::new();
        let root = fixture.dir("read-failure");
        Fixture::file(&root.join("a-good.wav"), &pcm_wav(Some("Readable")));
        Fixture::file(
            &root.join("b-unreadable.wav"),
            &pcm_wav(Some("Unavailable")),
        );
        Fixture::file(&root.join("c-good.wav"), &pcm_wav(Some("Still readable")));
        let walk = scanner::walk(&root).unwrap();
        let failed_relative = PathBuf::from("b-unreadable.wav");
        let work = prepare_metadata_with_opener(
            &root,
            &walk.files,
            &HashMap::new(),
            false,
            |root, file, parser| {
                if file.relative == failed_relative {
                    Err("cannot open validated audio file: Permission denied".to_owned())
                } else {
                    scanner::with_open_file(root, file, parser)
                }
            },
        )
        .unwrap();

        assert_eq!(work.updates.len(), 3);
        assert_eq!(work.metrics.files_opened_for_extraction, 2);
        assert_eq!(work.metrics.files_processed_by_parser, 2);
        assert_eq!(work.metrics.successful_extractions, 2);
        assert_eq!(work.metrics.extraction_failures, 1);
        let failed = work
            .updates
            .iter()
            .find(|update| update.relative == failed_relative)
            .unwrap();
        assert_eq!(failed.attempt.status, MetadataStatus::Failed);
        assert_eq!(
            failed.attempt.error.as_deref(),
            Some("cannot open validated audio file: Permission denied")
        );
        assert_eq!(
            work.updates
                .iter()
                .filter(|update| update.attempt.status == MetadataStatus::ParsedWithTags)
                .count(),
            2
        );

        let previous = work
            .updates
            .iter()
            .map(|update| {
                (
                    update.relative.clone(),
                    ExistingTrack {
                        size: update.size,
                        mtime_ns: update.mtime_ns,
                        attempt_size: Some(update.size),
                        attempt_mtime_ns: Some(update.mtime_ns),
                        attempt_parser_version: Some(update.parser_version),
                        attempt_error: update.attempt.error.clone(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let retry = prepare_metadata_with_opener(
            &root,
            &walk.files,
            &previous,
            false,
            |root, file, parser| scanner::with_open_file(root, file, parser),
        )
        .unwrap();
        assert_eq!(retry.updates.len(), 1);
        assert_eq!(retry.metrics.files_opened_for_extraction, 1);
        assert_eq!(retry.metrics.successful_extractions, 1);
        assert_eq!(retry.metrics.extraction_failures, 0);
        assert_eq!(retry.updates[0].relative, failed_relative);
        assert_eq!(
            retry.updates[0].attempt.status,
            MetadataStatus::ParsedWithTags
        );
    }

    #[cfg(target_os = "linux")]
    fn synthetic_aiff_with_embedded_cover() -> Vec<u8> {
        use lofty::{
            config::WriteOptions,
            picture::{MimeType, Picture, PictureType},
            tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType},
        };

        let png = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00,
            0x00, 0xb5, 0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xda, 0x63, 0xfc, 0xff, 0x1f, 0x00, 0x03, 0x03, 0x02, 0x00, 0xef, 0xa3, 0x3d, 0x50,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let mut bytes =
            metadata::synthetic_aiff(&[(ItemKey::AlbumTitle, "Embedded Cover Album")], &[], false);
        let mut tag = Tag::new(TagType::Id3v2);
        assert!(tag.push(TagItem::new(
            ItemKey::AlbumArtist,
            ItemValue::Text("Synthetic Artist".into()),
        )));
        tag.push_picture(
            Picture::unchecked(png)
                .mime_type(MimeType::Png)
                .pic_type(PictureType::CoverFront)
                .build(),
        );
        let id3: lofty::id3::v2::Id3v2Tag = tag.into();
        let mut id3_bytes = Vec::new();
        id3.dump_to(&mut id3_bytes, WriteOptions::default())
            .unwrap();
        bytes.extend_from_slice(b"ID3 ");
        bytes.extend_from_slice(&(id3_bytes.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&id3_bytes);
        if id3_bytes.len() % 2 != 0 {
            bytes.push(0);
        }
        let form_length = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&form_length.to_be_bytes());
        bytes
    }

    fn schema_signature(connection: &Connection) -> Vec<(String, String, String)> {
        let mut statement = connection
            .prepare(
                "SELECT type,name,sql FROM sqlite_master \
                 WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
            )
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn assert_database_integrity(path: &Path) {
        let connection = Connection::open(path).unwrap();
        let result: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(result, "ok");
        let mut statement = connection.prepare("PRAGMA foreign_key_check").unwrap();
        let mut rows = statement.query([]).unwrap();
        assert!(rows.next().unwrap().is_none());
    }

    fn assert_single_track_state(path: &Path, source_id: i64, generation: i64, size: i64) {
        let catalog = Catalog::open(path).unwrap();
        let sources = catalog.sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id, source_id);
        assert_eq!(sources[0].generation, generation);
        let tracks = catalog.tracks(source_id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].size, size);
        drop(catalog);
        assert_database_integrity(path);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn local_lrc_read_is_exact_bounded_and_source_generation_fenced() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let root = f.dir("Letras Ü");
        let audio = root.join("01 - Airbag.flac");
        let sidecar = root.join("01 - Airbag.lrc");
        Fixture::file(&audio, b"synthetic audio bytes");
        Fixture::file(&sidecar, b"[00:01.00]fixture lyrics");
        let outside = f.path("outside.lrc");
        Fixture::file(&outside, b"private fixture");
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "synthetic").unwrap().source_id;
        let source = catalog
            .sources()
            .unwrap()
            .into_iter()
            .find(|item| item.id == source_id)
            .unwrap();
        assert_eq!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    512 * 1024
                )
                .unwrap()
                .as_deref(),
            Some(&b"[00:01.00]fixture lyrics"[..])
        );
        assert!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    512 * 1024 + 1
                )
                .is_err(),
            "the catalog API cannot widen the parser's fixed 512 KiB boundary"
        );
        assert!(catalog
            .read_verified_track_lrc(
                source_id,
                source.generation,
                Path::new("missing.flac"),
                512 * 1024
            )
            .is_err());

        fs::remove_file(&sidecar).unwrap();
        Fixture::file(
            &root.join("01 - Airbag.LRC"),
            b"uppercase extension is not selected",
        );
        assert_eq!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    512 * 1024
                )
                .unwrap(),
            None,
            "only the exact lowercase .lrc sidecar is admitted on a case-sensitive filesystem"
        );
        fs::remove_file(root.join("01 - Airbag.LRC")).unwrap();
        Fixture::file(&sidecar, b"[00:01.00]fixture lyrics");

        fs::remove_file(&sidecar).unwrap();
        symlink(&outside, &sidecar).unwrap();
        assert!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    512 * 1024
                )
                .is_err(),
            "a symlink is never treated as a sidecar or followed"
        );
        fs::remove_file(&sidecar).unwrap();
        Fixture::file(&sidecar, &vec![b'x'; 33]);
        assert!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    32
                )
                .is_err(),
            "oversize is rejected before content is returned"
        );

        fs::write(&audio, b"synthetic audio bytes changed").unwrap();
        assert!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    512 * 1024
                )
                .is_err(),
            "a changed physical track cannot authorize its sidecar"
        );
        let next = catalog.verify_source(source_id).unwrap();
        assert!(next.generation > source.generation);
        assert!(
            catalog
                .read_verified_track_lrc(
                    source_id,
                    source.generation,
                    Path::new("01 - Airbag.flac"),
                    512 * 1024
                )
                .is_err(),
            "old generation cannot authorize a later read"
        );
    }

    #[test]
    fn add_source_parses_and_persists_wav_metadata_across_reopen() {
        let f = Fixture::new();
        let root = f.dir("A");
        let track = root.join("tagged.wav");
        Fixture::file(&track, &pcm_wav(Some("Opening title")));
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            let report = catalog.add_source(&root, "A").unwrap();
            source_id = report.source_id;
            assert_eq!(report.scan.metrics.files_opened_for_extraction, 1);
            assert_eq!(report.scan.metrics.files_processed_by_parser, 1);
            assert_eq!(report.scan.metrics.successful_extractions, 1);
            assert_eq!(report.scan.metrics.metadata_records_written, 1);
            let metadata = catalog
                .metadata_for_track(source_id, Path::new("tagged.wav"))
                .unwrap()
                .unwrap();
            assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
            assert!(metadata.current_for_file);
            assert_eq!(
                metadata.last_success.unwrap().title.as_deref(),
                Some("Opening title")
            );
        }

        let reopened = Catalog::open(f.db()).unwrap();
        let metadata = reopened
            .metadata_for_track(source_id, "tagged.wav")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(
            metadata
                .last_success
                .unwrap()
                .track_artists
                .first()
                .map(String::as_str),
            Some("Synthetic Artist")
        );
        assert_database_integrity(&f.db());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wavpack_tags_and_cue_image_recover_incrementally_and_persist() {
        let fixture = Fixture::new();
        let root = fixture.dir("WavPack/Café Album");
        Fixture::file(&root.join("album.wv"), &metadata::synthetic_wavpack());
        Fixture::file(&root.join("album.wvc"), &metadata::synthetic_wavpack());
        Fixture::file(
            &root.join("album.cue"),
            "TITLE \"Fixture Album\"\nPERFORMER \"Fixture Album Artist\"\nFILE \"album.wv\" WAVE\nTRACK 01 AUDIO\nTITLE \"Opening\"\nPERFORMER \"Track Artist\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"Closing\"\nINDEX 01 00:01:00\n".as_bytes(),
        );
        let database = fixture.db();
        let mut catalog = Catalog::open(&database).unwrap();
        let source_id = catalog.add_source(&root, "WavPack CUE fixture").unwrap().source_id;
        let metadata = catalog
            .metadata_for_track(source_id, Path::new("album.wv"))
            .unwrap()
            .unwrap();
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(metadata.parser_version, metadata::WAVPACK_PARSER_VERSION);
        let values = metadata.last_success.unwrap();
        assert_eq!(values.title.as_deref(), Some("Fixture Track"));
        assert_eq!(values.album.as_deref(), Some("Fixture Album"));
        assert_eq!(values.album_artists, ["Fixture Album Artist"]);
        assert_eq!(values.track_artists, ["Fixture Artist"]);

        let tracks = catalog.cue_tracks_for_source(source_id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().all(|track| track.audio_path == Path::new("album.wv")));
        assert_eq!(tracks[0].title.as_deref(), Some("Opening"));
        assert_eq!(tracks[0].performers, ["Track Artist"]);
        assert_eq!(tracks[0].start_frame, 0);
        assert_eq!(tracks[0].end_frame, Some(75));
        assert_eq!(tracks[0].duration_ms, Some(1000));
        assert_eq!(tracks[1].start_frame, 75);
        assert_eq!(tracks[1].end_frame, None);
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 1);
        drop(catalog);
        let cue_parser_version: i64 = Connection::open(&database).unwrap().query_row(
            "SELECT parser_version FROM cue_sheets WHERE source_id=?",
            [source_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(cue_parser_version, CUE_PARSER_VERSION);

        // Reproduce persisted unsupported rows from the previous parser and
        // CUE parser versions; normal verification must selectively recover.
        let connection = Connection::open(&database).unwrap();
        connection.execute(
            "UPDATE track_metadata SET attempt_status=3,attempt_parser_version=2 WHERE source_id=?",
            [source_id],
        ).unwrap();
        connection.execute("DELETE FROM cue_track_artists WHERE source_id=?", [source_id]).unwrap();
        connection.execute("DELETE FROM cue_tracks WHERE source_id=?", [source_id]).unwrap();
        connection.execute(
            "UPDATE cue_sheets SET parser_version=3,status=2,error_code='cue_file_reference_unavailable',track_count=0 WHERE source_id=?",
            [source_id],
        ).unwrap();
        drop(connection);

        let mut reopened = Catalog::open(&database).unwrap();
        let report = reopened.verify_source(source_id).unwrap();
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        let metadata = reopened.metadata_for_track(source_id, "album.wv").unwrap().unwrap();
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(metadata.parser_version, metadata::WAVPACK_PARSER_VERSION);
        let sheets = reopened.cue_sheets_for_source(source_id).unwrap();
        assert_eq!(sheets[0].status, CueSheetStatus::Indexed);
        assert_eq!(sheets[0].track_count, 2);
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap().len(), 2);
        drop(reopened);

        let reopened = Catalog::open(&database).unwrap();
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap().len(), 2);
        assert_eq!(reopened.track_count_for(source_id).unwrap(), 1);
        assert_database_integrity(&database);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cue_can_bind_multiple_wavpack_files_with_unicode_and_spaces() {
        let fixture = Fixture::new();
        let root = fixture.dir("WavPack multi-file");
        Fixture::file(&root.join("disc one.wv"), &metadata::synthetic_wavpack());
        Fixture::file(&root.join("disc 二.wv"), &metadata::synthetic_wavpack());
        Fixture::file(
            &root.join("album.cue"),
            "TITLE \"Two files\"\nFILE \"disc one.wv\" WAVE\nTRACK 01 AUDIO\nTITLE \"Part one\"\nINDEX 01 00:00:00\nFILE \"disc 二.wv\" WAVE\nTRACK 02 AUDIO\nTITLE \"Part two\"\nINDEX 01 00:00:00\n".as_bytes(),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "WavPack multi-file").unwrap().source_id;
        let tracks = catalog.cue_tracks_for_source(source_id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].audio_path, Path::new("disc one.wv"));
        assert_eq!(tracks[1].audio_path, Path::new("disc 二.wv"));
        assert_eq!(tracks[0].title.as_deref(), Some("Part one"));
        assert_eq!(tracks[1].title.as_deref(), Some("Part two"));
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cue_two_wavpack_files_with_twelve_tracks_and_unknown_rem_reindexes_and_persists() {
        let fixture = Fixture::new();
        let root = fixture.dir("Patricia Barber Cafe Blue 1STEP");
        Fixture::file(
            &root.join("Patricia Barber Cafe Blue 1LP.wv"),
            &metadata::synthetic_wavpack(),
        );
        Fixture::file(
            &root.join("Patricia Barber Cafe Blue 2LP.wv"),
            &metadata::synthetic_wavpack(),
        );

        let mut cue = String::from(
            "REM GENRE Jazz\nREM DATE 2020\nREM BAZAR_VinylRiP\nREM LABEL Impex Records - IMP 6035-1\nPERFORMER \"Patricia Barber\"\nTITLE \"Cafe Blue [1STEP  45rpm IMXLPO6035-45]\"\nFILE \"Patricia Barber Cafe Blue 1LP.wv\" WAVE\n",
        );
        let first = ["00:00:00", "05:24:00", "12:20:00", "17:00:00", "21:33:00", "26:42:00"];
        let second = ["00:00:00", "05:23:00", "13:33:00", "18:06:00", "27:07:00", "28:07:00"];
        for (offset, timestamps) in [(0_u32, first), (6_u32, second)] {
            if offset == 6 {
                cue.push_str("FILE \"Patricia Barber Cafe Blue 2LP.wv\" WAVE\n");
            }
            for (index, timestamp) in timestamps.into_iter().enumerate() {
                let number = offset + index as u32 + 1;
                cue.push_str(&format!(
                    "TRACK {number:02} AUDIO\nTITLE \"Cafe Blue - {number:02}\"\nINDEX 01 {timestamp}\n"
                ));
            }
        }
        Fixture::file(&root.join("album.cue"), cue.as_bytes());

        let database = fixture.db();
        let mut catalog = Catalog::open(&database).unwrap();
        let source_id = catalog.add_source(&root, "Patricia Barber fixture").unwrap().source_id;
        let tracks = catalog.cue_tracks_for_source(source_id).unwrap();
        assert_eq!(tracks.len(), 12);
        assert_eq!(tracks.iter().map(|track| track.track_number).collect::<Vec<_>>(), (1..=12).collect::<Vec<_>>());
        assert!(tracks[..6].iter().all(|track| track.audio_path == Path::new("Patricia Barber Cafe Blue 1LP.wv")));
        assert!(tracks[6..].iter().all(|track| track.audio_path == Path::new("Patricia Barber Cafe Blue 2LP.wv")));
        assert_eq!(tracks.iter().map(|track| track.start_frame).collect::<Vec<_>>(), [
            0, 24_300, 55_500, 76_500, 96_975, 120_150,
            0, 24_225, 60_975, 81_450, 122_025, 126_525,
        ]);
        assert_eq!(tracks.iter().map(|track| track.end_frame).collect::<Vec<_>>(), [
            Some(24_300), Some(55_500), Some(76_500), Some(96_975), Some(120_150), None,
            Some(24_225), Some(60_975), Some(81_450), Some(122_025), Some(126_525), None,
        ]);
        for track in &tracks {
            assert_eq!(track.album.as_deref(), Some("Cafe Blue [1STEP  45rpm IMXLPO6035-45]"));
            assert_eq!(track.album_performers, ["Patricia Barber"]);
            assert_eq!(track.performers, ["Patricia Barber"]);
            assert_eq!(track.genre.as_deref(), Some("Jazz"));
            assert_eq!(track.date.as_deref(), Some("2020"));
        }
        assert_eq!(tracks[0].title.as_deref(), Some("Cafe Blue - 01"));
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 2, "physical files remain two catalog tracks; CUE rows are virtual");
        assert_eq!(catalog.cue_sheets_for_source(source_id).unwrap()[0].status, CueSheetStatus::Indexed);

        // Simulate a legacy row rejected by parser version 4. Version 5 must
        // revisit it, restore the virtual tracks, and then remain incremental.
        let connection = Connection::open(&database).unwrap();
        connection.execute("DELETE FROM cue_tracks WHERE source_id=?", [source_id]).unwrap();
        connection.execute(
            "UPDATE cue_sheets SET parser_version=4,status=2,error_code='cue_parse_rejected',track_count=0 WHERE source_id=?",
            [source_id],
        ).unwrap();
        drop(connection);
        let old_generation = catalog.sources().unwrap()[0].generation;
        assert_eq!(catalog.verify_source(source_id).unwrap().generation, old_generation + 1);
        assert_eq!(catalog.cue_tracks_for_source(source_id).unwrap().len(), 12);
        let version: i64 = Connection::open(&database).unwrap().query_row(
            "SELECT parser_version FROM cue_sheets WHERE source_id=?", [source_id], |row| row.get(0),
        ).unwrap();
        assert_eq!(version, CUE_PARSER_VERSION);

        let restored = catalog.cue_tracks_for_source(source_id).unwrap();
        drop(catalog);
        let mut reopened = Catalog::open(&database).unwrap();
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap(), restored);
        let generation = reopened.sources().unwrap()[0].generation;
        assert_eq!(reopened.verify_source(source_id).unwrap().generation, generation);
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap().len(), 12);
        assert_database_integrity(&database);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn malformed_known_cue_rem_is_diagnosed_without_virtual_tracks() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-known-rem-invalid");
        Fixture::file(&root.join("disc one.wv"), &metadata::synthetic_wavpack());
        Fixture::file(&root.join("disc two.wv"), &metadata::synthetic_wavpack());
        Fixture::file(
            &root.join("album.cue"),
            b"REM GENRE\nTITLE \"Album\"\nFILE \"disc one.wv\" WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nFILE \"disc two.wv\" WAVE\nTRACK 02 AUDIO\nINDEX 01 00:00:00\n",
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "Invalid CUE fixture").unwrap().source_id;
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 2);
        assert!(catalog.cue_tracks_for_source(source_id).unwrap().is_empty());
        assert!(matches!(
            catalog.cue_sheets_for_source(source_id).unwrap()[0].status,
            CueSheetStatus::Rejected { ref error_code } if error_code == "cue_rem_value_missing"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cue_segments_are_scanned_safely_persisted_and_restored_without_startup_scan() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-library");
        Fixture::file(&root.join("image.wav"), &pcm_wav(Some("Physical image")));
        Fixture::file(
            &root.join("album.cue"),
            b"TITLE \"Synthetic CUE Album\"\nPERFORMER \"Album Artist\"\nREM GENRE \"Jazz\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"First\"\nPERFORMER \"Artist One\"\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nTITLE \"Second\"\nPERFORMER \"Artist Two\"\nINDEX 01 00:00:02\n",
        );
        let database = fixture.db();
        let mut catalog = Catalog::open(&database).unwrap();
        let added = catalog.add_source(&root, "CUE fixture").unwrap();
        let source_id = added.source_id;
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 1);
        let cue_metadata = fs::metadata(root.join("album.cue")).unwrap();
        assert_eq!(catalog.cue_sheets_for_source(source_id).unwrap(), [CueSheetRecord {
            source_id,
            relative_path: PathBuf::from("album.cue"),
            size: cue_metadata.len() as i64,
            mtime_ns: cue_metadata.modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64,
            device: cue_metadata.dev(),
            inode: cue_metadata.ino(),
            ctime_ns: cue_metadata.ctime().saturating_mul(1_000_000_000).saturating_add(cue_metadata.ctime_nsec()),
            status: CueSheetStatus::Indexed,
            track_count: 2,
        }]);
        let cue_tracks = catalog.cue_tracks_for_source(source_id).unwrap();
        assert_eq!(cue_tracks.len(), 2);
        assert_eq!(cue_tracks[0].cue_path, Path::new("album.cue"));
        assert_eq!(cue_tracks[0].audio_path, Path::new("image.wav"));
        assert_eq!(cue_tracks[0].start_frame, 0);
        assert_eq!(cue_tracks[0].end_frame, Some(2));
        assert_eq!(cue_tracks[0].duration_ms, Some(26));
        assert_eq!(cue_tracks[0].title.as_deref(), Some("First"));
        assert_eq!(cue_tracks[0].performers, ["Artist One"]);
        assert_eq!(cue_tracks[0].album.as_deref(), Some("Synthetic CUE Album"));
        assert_eq!(cue_tracks[0].album_performers, ["Album Artist"]);
        assert_eq!(cue_tracks[0].genre.as_deref(), Some("Jazz"));
        assert!(cue_tracks[0].audio_device.is_some());
        assert!(cue_tracks[0].audio_inode.is_some());
        assert!(cue_tracks[0].audio_ctime_ns.is_some());
        assert!(cue_tracks[1].duration_ms.is_some());
        let view = catalog.source_library_view(source_id).unwrap();
        assert_eq!(view.cue_tracks(), cue_tracks);
        assert_eq!(view.verified_cue_track(Path::new("album.cue"), 1).unwrap(), Some(cue_tracks[0].clone()));
        let generation = catalog.sources().unwrap()[0].generation;
        drop(catalog);

        let mut reopened = Catalog::open(&database).unwrap();
        assert_eq!(reopened.sources().unwrap()[0].generation, generation);
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap(), cue_tracks);
        let check = reopened.verify_source(source_id).unwrap();
        assert_eq!(check.generation, generation, "unchanged CUE sheets must not force a generation write");
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap(), cue_tracks);
        assert_database_integrity(&database);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nested_cue_sheet_resolves_single_and_multiple_audio_files_relative_to_its_directory() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-nested-relative-files");
        let album = root.join("Artist/Album");
        fs::create_dir_all(album.join("Audio")).unwrap();
        // A root-level decoy catches accidental source-root-relative binding.
        Fixture::file(&root.join("disc one.wav"), &pcm_wav(Some("Wrong decoy")));
        Fixture::file(&album.join("disc one.wav"), &pcm_wav(Some("Disc one")));
        Fixture::file(&album.join("Audio/disc two.wav"), &pcm_wav(Some("Disc two")));
        Fixture::file(
            &album.join("set.cue"),
            b"TITLE \"Nested Set\"\nFILE \"disc one.wav\" WAVE\nTRACK 01 AUDIO\nTITLE \"First\"\nINDEX 01 00:00:00\nFILE \"Audio/disc two.wav\" WAVE\nTRACK 02 AUDIO\nTITLE \"Second\"\nINDEX 01 00:00:01\n",
        );

        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "Nested CUE").unwrap().source_id;
        let tracks = catalog.cue_tracks_for_source(source_id).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].audio_path, Path::new("Artist/Album/disc one.wav"));
        assert_eq!(tracks[1].audio_path, Path::new("Artist/Album/Audio/disc two.wav"));
        assert!(tracks.iter().all(|track| track.cue_path == Path::new("Artist/Album/set.cue")));
        let view = catalog.source_library_view(source_id).unwrap();
        assert!(view.verified_cue_track(Path::new("Artist/Album/set.cue"), 1).unwrap().is_some());
        assert!(view.verified_cue_track(Path::new("Artist/Album/set.cue"), 2).unwrap().is_some());
        assert_eq!(catalog.verify_source(source_id).unwrap().generation, catalog.sources().unwrap()[0].generation);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cue_parser_version_change_rebinds_legacy_root_relative_rows_without_schema_migration() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-parser-v1-refresh");
        let album = root.join("Artist/Album");
        fs::create_dir_all(&album).unwrap();
        Fixture::file(&root.join("disc.wav"), &pcm_wav(Some("Wrong root decoy")));
        Fixture::file(&album.join("disc.wav"), &pcm_wav(Some("Correct nested audio")));
        Fixture::file(&album.join("album.cue"), b"FILE disc.wav WAVE\nTRACK 01 AUDIO\nTITLE \"One\"\nINDEX 01 00:00:00\n");

        let database = fixture.db();
        let mut catalog = Catalog::open(&database).unwrap();
        let source_id = catalog.add_source(&root, "Parser version refresh").unwrap().source_id;
        assert_eq!(catalog.cue_tracks_for_source(source_id).unwrap()[0].audio_path, Path::new("Artist/Album/disc.wav"));
        drop(catalog);

        let decoy = fs::metadata(root.join("disc.wav")).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection.execute(
            "UPDATE cue_sheets SET parser_version=1 WHERE source_id=?",
            [source_id],
        ).unwrap();
        connection.execute(
            "UPDATE cue_tracks SET audio_path=?,audio_device=?,audio_inode=?,audio_ctime_ns=? WHERE source_id=?",
            rusqlite::params![
                b"disc.wav".as_slice(),
                decoy.dev().to_be_bytes().as_slice(),
                decoy.ino().to_be_bytes().as_slice(),
                decoy.ctime().saturating_mul(1_000_000_000).saturating_add(decoy.ctime_nsec()),
                source_id,
            ],
        ).unwrap();
        drop(connection);

        let mut reopened = Catalog::open(&database).unwrap();
        let previous_generation = reopened.sources().unwrap()[0].generation;
        let report = reopened.verify_source(source_id).unwrap();
        assert_eq!(report.generation, previous_generation + 1);
        assert_eq!(reopened.cue_tracks_for_source(source_id).unwrap()[0].audio_path, Path::new("Artist/Album/disc.wav"));
        assert_database_integrity(&database);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn v7_cue_audio_identity_migration_is_atomic_and_requires_explicit_refresh() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-v7-migration");
        Fixture::file(&root.join("image.wav"), &pcm_wav(Some("Image")));
        Fixture::file(&root.join("album.cue"), b"FILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"Segment\"\nINDEX 01 00:00:00\n");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "CUE v7 migration").unwrap().source_id;
        let generation = catalog.sources().unwrap()[0].generation;
        let before_track = catalog.cue_tracks_for_source(source_id).unwrap()[0].clone();
        assert!(before_track.audio_device.is_some());
        drop(catalog);

        let connection = Connection::open(fixture.db()).unwrap();
        connection.execute_batch(
            "DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections;\
             ALTER TABLE cue_tracks DROP COLUMN audio_device;\
             ALTER TABLE cue_tracks DROP COLUMN audio_inode;\
             ALTER TABLE cue_tracks DROP COLUMN audio_ctime_ns;\
             PRAGMA user_version=7;",
        ).unwrap();
        drop(connection);
        let schema_v7 = schema_signature(&Connection::open(fixture.db()).unwrap());

        let mut connection = Connection::open(fixture.db()).unwrap();
        assert!(migrate_v7_to_v8_failing(&mut connection).unwrap_err().contains("injected v7-to-v8"));
        drop(connection);
        let connection = Connection::open(fixture.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), SCHEMA_V7_VERSION);
        assert_eq!(schema_signature(&connection), schema_v7);
        assert_eq!(connection.query_row("SELECT count(*) FROM cue_tracks", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        drop(connection);

        let mut migrated = Catalog::open(fixture.db()).unwrap();
        assert_eq!(migrated.sources().unwrap()[0].generation, generation, "schema migration must not rescan the source");
        let legacy_track = migrated.cue_tracks_for_source(source_id).unwrap().remove(0);
        assert_eq!(legacy_track.title, before_track.title);
        assert!(legacy_track.audio_device.is_none());
        assert!(legacy_track.audio_inode.is_none());
        assert!(legacy_track.audio_ctime_ns.is_none());
        let view = migrated.source_library_view(source_id).unwrap();
        assert!(view.verified_cue_track(Path::new("album.cue"), 1).is_err(), "legacy nullable identity cannot authorize playback");

        migrated.verify_source(source_id).unwrap();
        let refreshed = migrated.cue_tracks_for_source(source_id).unwrap().remove(0);
        assert!(refreshed.audio_device.is_some());
        assert!(refreshed.audio_inode.is_some());
        assert!(refreshed.audio_ctime_ns.is_some());
        let refreshed_view = migrated.source_library_view(source_id).unwrap();
        assert!(refreshed_view.verified_cue_track(Path::new("album.cue"), 1).unwrap().is_some());
        assert_database_integrity(&fixture.db());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cue_path_traversal_and_missing_references_never_create_virtual_tracks() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-invalid");
        Fixture::file(&root.join("physical.wav"), &pcm_wav(Some("Physical remains")));
        Fixture::file(&root.join("unsafe.cue"), b"FILE ../outside.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n");
        Fixture::file(&root.join("missing.cue"), b"FILE absent.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "Invalid cue fixture").unwrap().source_id;
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 1);
        assert!(catalog.cue_tracks_for_source(source_id).unwrap().is_empty());
        let sheets = catalog.cue_sheets_for_source(source_id).unwrap();
        assert_eq!(sheets.len(), 2);
        assert!(sheets.iter().all(|sheet| matches!(sheet.status, CueSheetStatus::Rejected { .. })));
        assert!(sheets.iter().any(|sheet| matches!(&sheet.status, CueSheetStatus::Rejected { error_code } if error_code == "cue_file_reference_unavailable")));

        Fixture::file(&root.join("absent.wav"), &pcm_wav(Some("Now present")));
        catalog.verify_source(source_id).unwrap();
        assert_eq!(catalog.cue_tracks_for_source(source_id).unwrap().len(), 1);
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replacing_cue_audio_at_same_relative_path_is_rejected_by_the_captured_identity() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-audio-identity");
        let bytes = pcm_wav(Some("Image"));
        Fixture::file(&root.join("image.wav"), &bytes);
        Fixture::file(&root.join("album.cue"), b"FILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"One\"\nINDEX 01 00:00:00\n");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "CUE identity").unwrap().source_id;
        let old_view = catalog.source_library_view(source_id).unwrap();
        let expected = old_view.cue_tracks()[0].clone();
        assert!(old_view.verified_cue_track(Path::new("album.cue"), 1).unwrap().is_some());

        Fixture::file(&root.join("replacement.wav"), &bytes);
        let old_image = root.parent().unwrap().join("old-image.wav");
        fs::rename(root.join("image.wav"), &old_image).unwrap();
        fs::rename(root.join("replacement.wav"), root.join("image.wav")).unwrap();
        assert!(old_view.verified_cue_track(Path::new("album.cue"), 1).is_err(),
            "same path/size is insufficient when the physical audio object has changed");

        catalog.verify_source(source_id).unwrap();
        let refreshed = catalog.source_library_view(source_id).unwrap();
        assert!(refreshed.verified_cue_track(Path::new("album.cue"), 1).unwrap().is_some());
        assert_ne!(refreshed.cue_tracks()[0].audio_inode, expected.audio_inode);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cue_changes_and_removal_reconcile_atomically_and_source_removal_cascades() {
        let fixture = Fixture::new();
        let root = fixture.dir("cue-reconcile");
        Fixture::file(&root.join("image.wav"), &pcm_wav(Some("Image")));
        Fixture::file(&root.join("album.cue"), b"TITLE \"Old\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"Old track\"\nINDEX 01 00:00:00\n");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "Reconcile").unwrap().source_id;
        let old_generation = catalog.sources().unwrap()[0].generation;
        assert_eq!(catalog.cue_tracks_for_source(source_id).unwrap()[0].title.as_deref(), Some("Old track"));
        Fixture::file(&root.join("album.cue"), b"TITLE \"New album title\"\nFILE image.wav WAVE\nTRACK 01 AUDIO\nTITLE \"New track title\"\nINDEX 01 00:00:00\n");
        let updated = catalog.verify_source(source_id).unwrap();
        assert!(updated.generation > old_generation);
        assert_eq!(catalog.cue_tracks_for_source(source_id).unwrap()[0].title.as_deref(), Some("New track title"));

        fs::remove_file(root.join("album.cue")).unwrap();
        catalog.verify_source(source_id).unwrap();
        assert!(catalog.cue_sheets_for_source(source_id).unwrap().is_empty());
        assert!(catalog.cue_tracks_for_source(source_id).unwrap().is_empty());
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 1, "removing a CUE sheet must not remove its physical audio file");
        catalog.remove_source(source_id).unwrap();
        assert!(catalog.cue_sheets_for_source(source_id).is_err());
    }

    #[test]
    fn add_source_reports_monotonic_sanitized_progress_through_commit() {
        let f = Fixture::new();
        let root = f.dir("progress-root");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("One")));
        Fixture::file(&root.join("two.wav"), &pcm_wav(Some("Two")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let mut progress = Vec::new();
        let added = catalog
            .add_source_with_progress(&root, "progress", |event| progress.push(event))
            .unwrap();

        assert_eq!(progress.first().unwrap().phase, IndexPhase::Scanning);
        assert!(progress
            .iter()
            .any(|event| event.phase == IndexPhase::ReadingMetadata));
        let metadata = progress
            .iter()
            .find(|event| event.phase == IndexPhase::ReadingMetadata)
            .unwrap();
        assert_eq!(metadata.identified_albums, 1);
        assert_eq!(metadata.identified_artists, 1);
        assert_eq!(
            progress.iter().rev().nth(1).unwrap().phase,
            IndexPhase::Persisting
        );
        assert_eq!(progress.last().unwrap().phase, IndexPhase::Completed);
        assert!(progress.windows(2).all(|pair| {
            pair[0].entries_visited <= pair[1].entries_visited
                && pair[0].files_examined <= pair[1].files_examined
                && pair[0].audio_files_found <= pair[1].audio_files_found
        }));
        let last = progress.last().unwrap();
        assert_eq!(last.audio_files_found, 2);
        assert_eq!(last.metadata_processed, 2);
        assert_eq!(last.metadata_total, 2);
        assert!(!format!("{progress:?}").contains(root.to_string_lossy().as_ref()));

        let mut verification = Vec::new();
        let report = catalog
            .verify_source_with_progress(added.source_id, |event| verification.push(event))
            .unwrap();
        assert!(verification
            .iter()
            .any(|event| event.phase == IndexPhase::Scanning));
        assert_eq!(verification.last().unwrap().phase, IndexPhase::Completed);
        assert_eq!(verification.last().unwrap().audio_files_found, 2);
        assert_eq!(verification.last().unwrap().metadata_total, 2);
        assert_eq!(verification.last().unwrap().identified_albums, 1);
        assert_eq!(verification.last().unwrap().identified_artists, 1);
        assert!(!verification.last().unwrap().counts_provisional);
        assert_eq!(report.metrics.files_unchanged, 2);
        assert_database_integrity(&f.db());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn source_view_reads_embedded_cover_from_verified_open_file_without_reindexing() {
        let f = Fixture::new();
        let root = f.dir("embedded-art");
        Fixture::file(
            &root.join("track.aiff"),
            &synthetic_aiff_with_embedded_cover(),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "Embedded art").unwrap().source_id;
        let view = catalog.source_library_view(source_id).unwrap();
        let album = view
            .albums()
            .iter()
            .find(|album| album.title == "Embedded Cover Album")
            .unwrap();
        let cover = view
            .read_album_embedded_artwork(&album.id, 1024 * 1024, 1024)
            .unwrap()
            .unwrap();
        assert_eq!(cover.mime_type, "image/png");
        assert_eq!(cover.bytes.len(), 68);
        assert_eq!(catalog.sources().unwrap()[0].generation, view.generation());
        let unchanged = catalog.verify_source(source_id).unwrap();
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
    }

    #[test]
    fn unchanged_wav_is_not_reopened_or_reparsed() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("Stable")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        let report = catalog.verify_source(id).unwrap();
        assert_eq!(report.metrics.files_unchanged, 1);
        assert_eq!(report.metrics.files_opened_for_extraction, 0);
        assert_eq!(report.metrics.files_processed_by_parser, 0);
        assert_eq!(report.metrics.metadata_records_written, 0);
    }

    #[test]
    fn modified_wav_reprocesses_only_that_file() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("Old title")));
        Fixture::file(&root.join("two.wav"), &pcm_wav(Some("Unchanged title")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        Fixture::file(
            &root.join("one.wav"),
            &pcm_wav(Some("Updated title, longer")),
        );
        let report = catalog.verify_source(id).unwrap();
        assert_eq!(report.metrics.files_modified, 1);
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
        assert_eq!(
            catalog
                .metadata_for_track(id, "one.wav")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Updated title, longer")
        );
    }

    #[test]
    fn parse_failure_keeps_last_success_stale_then_recovery_replaces_it() {
        let f = Fixture::new();
        let root = f.dir("A");
        let path = root.join("one.wav");
        Fixture::file(&path, &pcm_wav(Some("Previously valid")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;

        Fixture::file(&path, b"invalid WAV payload");
        let failed = catalog.verify_source(id).unwrap();
        assert_eq!(failed.metrics.extraction_failures, 1);
        assert_eq!(failed.metrics.successful_extractions, 0);
        let stale = catalog.metadata_for_track(id, "one.wav").unwrap().unwrap();
        assert_eq!(stale.status, MetadataStatus::Failed);
        assert!(!stale.current_for_file);
        assert_eq!(
            stale.last_success.unwrap().title.as_deref(),
            Some("Previously valid")
        );

        Fixture::file(&path, &pcm_wav(Some("Recovered title")));
        let recovered = catalog.verify_source(id).unwrap();
        assert_eq!(recovered.metrics.successful_extractions, 1);
        let current = catalog.metadata_for_track(id, "one.wav").unwrap().unwrap();
        assert!(current.current_for_file);
        assert_eq!(
            current.last_success.unwrap().title.as_deref(),
            Some("Recovered title")
        );
    }

    #[test]
    fn rebuild_source_forces_reparse_of_an_unchanged_wav() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("Force")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        assert_eq!(
            catalog
                .verify_source(id)
                .unwrap()
                .metrics
                .files_processed_by_parser,
            0
        );
        let report = catalog.rebuild_source(id).unwrap();
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
    }

    #[test]
    fn unsupported_audio_is_marked_without_claiming_no_tags_or_parsing() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(
            &root.join("not-yet-supported.wma"),
            b"synthetic fixture bytes",
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        let metrics = catalog.verify_source(id).unwrap().metrics;
        assert_eq!(metrics.files_processed_by_parser, 0);
        let metadata = catalog
            .metadata_for_track(id, "not-yet-supported.wma")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.status, MetadataStatus::Unsupported);
        assert_eq!(metadata.file_format, "extension:wma");
        assert!(metadata.last_success.is_none());
    }

    #[test]
    fn mp3_tags_and_properties_persist_across_catalog_reopen() {
        use lofty::tag::{ItemKey, TagType};

        let f = Fixture::new();
        let root = f.dir("MP3");
        let bytes = metadata::synthetic_mp3(
            &[(
                TagType::Id3v2,
                &[
                    (ItemKey::TrackTitle, "Canção 東京"),
                    (ItemKey::TrackArtist, "Track One"),
                    (ItemKey::TrackArtist, "Track Two"),
                    (ItemKey::AlbumArtist, "Compilation Artist"),
                    (ItemKey::AlbumTitle, "Synthetic Album"),
                    (ItemKey::Genre, "Jazz"),
                    (ItemKey::RecordingDate, "2001-03"),
                    (ItemKey::TrackNumber, "7/14"),
                    (ItemKey::DiscNumber, "2"),
                ],
            )],
            false,
        );
        Fixture::file(&root.join("tagged.mp3"), &bytes);
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            let report = catalog.add_source(&root, "MP3").unwrap();
            source_id = report.source_id;
            assert_eq!(report.scan.metrics.files_examined, 1);
            assert_eq!(report.scan.metrics.files_opened_for_extraction, 1);
            assert_eq!(report.scan.metrics.files_processed_by_parser, 1);
            assert_eq!(report.scan.metrics.successful_extractions, 1);
            assert_eq!(report.scan.metrics.metadata_records_written, 1);
            let track = catalog
                .metadata_for_track(source_id, "tagged.mp3")
                .unwrap()
                .unwrap();
            assert_eq!(track.parser_version, metadata::MP3_PARSER_VERSION);
            assert_eq!(track.file_format, "mp3");
            assert!(track.current_for_file);
            let values = track.last_success.unwrap();
            assert_eq!(values.title.as_deref(), Some("Canção 東京"));
            assert_eq!(values.track_artists, ["Track One", "Track Two"]);
            assert_eq!(values.album_artists, ["Compilation Artist"]);
            assert_eq!(values.album.as_deref(), Some("Synthetic Album"));
            assert_eq!(values.genre.as_deref(), Some("Jazz"));
            assert_eq!(values.date_text.as_deref(), Some("2001-03"));
            assert_eq!(values.track_number, Some(7));
            assert_eq!(values.disc_number, Some(2));
            assert_eq!(values.sample_rate, Some(44_100));
            assert_eq!(values.channels, Some(2));
            assert!(values.duration_ms.is_some_and(|duration| duration > 0));
        }
        let reopened = Catalog::open(f.db()).unwrap();
        let track = reopened
            .metadata_for_track(source_id, "tagged.mp3")
            .unwrap()
            .unwrap();
        assert!(track.current_for_file);
        assert_eq!(
            track.last_success.unwrap().track_artists,
            ["Track One", "Track Two"]
        );
    }

    #[test]
    fn mp3_integration_retries_previous_unsupported_version_without_reparsing_wav_or_flac() {
        use lofty::tag::{ItemKey, TagType};

        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(
            &root.join("one.mp3"),
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "Now supported")])],
                false,
            ),
        );
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("WAV stable")));
        Fixture::file(
            &root.join("one.flac"),
            &metadata::synthetic_flac(&[("TITLE", "FLAC stable")]),
        );
        let database = f.db();
        let mut catalog = Catalog::open(&database).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        catalog
            .db
            .connection
            .execute(
                "UPDATE track_metadata SET attempt_status=3,file_format='extension:mp3',\
                 attempt_parser_version=2,success_size=NULL,success_mtime_ns=NULL,title=NULL,\
                 track_artist=NULL,album_artist=NULL,album=NULL,genre=NULL,date_text=NULL,\
                 track_number=NULL,disc_number=NULL,duration_ms=NULL,sample_rate=NULL,channels=NULL \
                 WHERE source_id=?1 AND relative_path=?2",
                rusqlite::params![
                    id,
                    path_codec::encode_relative(Path::new("one.mp3")).unwrap()
                ],
            )
            .unwrap();
        drop(catalog);

        let mut reopened = Catalog::open(&database).unwrap();
        let report = reopened.verify_source(id).unwrap();
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
        let mp3 = reopened.metadata_for_track(id, "one.mp3").unwrap().unwrap();
        assert_eq!(mp3.status, MetadataStatus::ParsedWithTags);
        assert_eq!(mp3.parser_version, metadata::MP3_PARSER_VERSION);
        assert_eq!(
            mp3.last_success.unwrap().title.as_deref(),
            Some("Now supported")
        );

        let unchanged = reopened.verify_source(id).unwrap();
        assert_eq!(unchanged.metrics.files_examined, 3);
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
        assert_eq!(unchanged.metrics.metadata_records_written, 0);
    }

    #[test]
    fn mp3_modification_failure_force_rebuild_and_recovery_are_incremental() {
        use lofty::tag::{ItemKey, TagType};

        let f = Fixture::new();
        let root = f.dir("A");
        let path = root.join("one.mp3");
        Fixture::file(
            &path,
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "Original title")])],
                false,
            ),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        let changed = metadata::synthetic_mp3(
            &[(
                TagType::Id3v2,
                &[(ItemKey::TrackTitle, "Updated title, longer")],
            )],
            false,
        );
        Fixture::file(&path, &changed);
        let report = catalog.verify_source(id).unwrap();
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
        let current = catalog.metadata_for_track(id, "one.mp3").unwrap().unwrap();
        assert_eq!(
            current.last_success.unwrap().title.as_deref(),
            Some("Updated title, longer")
        );

        Fixture::file(&path, b"corrupt MP3 payload with a changed size");
        let failed = catalog.verify_source(id).unwrap();
        assert_eq!(failed.metrics.files_opened_for_extraction, 1);
        assert_eq!(failed.metrics.extraction_failures, 1);
        let stale = catalog.metadata_for_track(id, "one.mp3").unwrap().unwrap();
        assert_eq!(stale.status, MetadataStatus::Failed);
        assert!(!stale.current_for_file);
        assert_eq!(
            stale.last_success.unwrap().title.as_deref(),
            Some("Updated title, longer")
        );

        Fixture::file(
            &path,
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "Recovered")])],
                false,
            ),
        );
        let recovered = catalog.verify_source(id).unwrap();
        assert_eq!(recovered.metrics.files_opened_for_extraction, 1);
        assert_eq!(
            catalog
                .metadata_for_track(id, "one.mp3")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Recovered")
        );

        let forced = catalog.rebuild_source(id).unwrap();
        assert_eq!(forced.metrics.files_opened_for_extraction, 1);
        assert_eq!(forced.metrics.files_processed_by_parser, 1);
        assert_eq!(forced.metrics.metadata_records_written, 1);
    }

    #[test]
    fn metadata_updates_roll_back_when_verification_is_interrupted_before_commit() {
        let f = Fixture::new();
        let root = f.dir("A");
        let path = root.join("one.wav");
        Fixture::file(&path, &pcm_wav(Some("Committed title")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        Fixture::file(&path, &pcm_wav(Some("Uncommitted changed title")));
        assert!(catalog
            .verify_with_interruption(id)
            .unwrap_err()
            .contains("simulated interruption"));

        let reopened = Catalog::open(f.db()).unwrap();
        let metadata = reopened.metadata_for_track(id, "one.wav").unwrap().unwrap();
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert!(metadata.current_for_file);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Committed title")
        );
        assert_eq!(reopened.sources().unwrap()[0].generation, 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replacing_wav_with_symlink_after_enumeration_is_rejected_without_commit() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let root = f.dir("A");
        let track = root.join("one.wav");
        Fixture::file(&track, &pcm_wav(Some("Original")));
        let outside = f.path("outside.wav");
        Fixture::file(&outside, &pcm_wav(Some("Outside")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        Fixture::file(&track, &pcm_wav(Some("Changed before next scan")));
        let result = catalog.verify_inner(
            id,
            false,
            false,
            || {},
            || {},
            |root| {
                let scanned = scanner::walk(root)?;
                fs::remove_file(&track).map_err(|error| error.to_string())?;
                symlink(&outside, &track).map_err(|error| error.to_string())?;
                Ok(scanned)
            },
        );
        assert!(result.unwrap_err().contains("regular file"));
        assert_eq!(catalog.sources().unwrap()[0].generation, 1);
        let metadata = catalog.metadata_for_track(id, "one.wav").unwrap().unwrap();
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Original")
        );
    }

    #[test]
    fn adding_source_b_parses_only_b_and_keeps_a_cataloged() {
        let f = Fixture::new();
        let a = f.dir("A");
        let b = f.dir("B");
        Fixture::file(&a.join("a.wav"), &pcm_wav(Some("A")));
        Fixture::file(&b.join("b.wav"), &pcm_wav(Some("B")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let a_id = catalog.add_source(&a, "A").unwrap().source_id;
        let b_report = catalog.add_source(&b, "B").unwrap();
        assert_eq!(b_report.scan.metrics.sources_examined, 1);
        assert_eq!(b_report.scan.metrics.files_examined, 1);
        assert_eq!(b_report.scan.metrics.files_processed_by_parser, 1);
        assert_eq!(catalog.track_count().unwrap(), 2);
        assert_eq!(catalog.track_count_for(a_id).unwrap(), 1);
        assert_eq!(
            catalog
                .metadata_for_track(a_id, "a.wav")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("A")
        );
    }

    #[test]
    fn flac_tags_properties_and_multiple_artist_values_persist_across_reopen() {
        let f = Fixture::new();
        let root = f.dir("FLAC");
        Fixture::file(
            &root.join("tagged.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Synthetic FLAC"),
                ("ARTIST", "Track One"),
                ("ARTIST", "Track Two"),
                ("ALBUMARTIST", "Album One"),
                ("ALBUMARTIST", "Album Two"),
                ("ALBUM", "Test Album"),
                ("GENRE", "Jazz"),
                ("DATE", "1987-09"),
                ("TRACKNUMBER", "4/11"),
                ("DISCNUMBER", "2"),
            ]),
        );
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            let report = catalog.add_source(&root, "FLAC").unwrap();
            source_id = report.source_id;
            assert_eq!(report.scan.metrics.files_examined, 1);
            assert_eq!(report.scan.metrics.files_opened_for_extraction, 1);
            assert_eq!(report.scan.metrics.files_processed_by_parser, 1);
            assert_eq!(report.scan.metrics.successful_extractions, 1);
            assert_eq!(report.scan.metrics.metadata_records_written, 1);
            let metadata = catalog
                .metadata_for_track(source_id, "tagged.flac")
                .unwrap()
                .unwrap();
            assert_eq!(metadata.file_format, "flac");
            assert!(metadata.current_for_file);
            let values = metadata.last_success.unwrap();
            assert_eq!(values.title.as_deref(), Some("Synthetic FLAC"));
            assert_eq!(values.track_artists, ["Track One", "Track Two"]);
            assert_eq!(values.album_artists, ["Album One", "Album Two"]);
            assert_eq!(values.album.as_deref(), Some("Test Album"));
            assert_eq!(values.genre.as_deref(), Some("Jazz"));
            assert_eq!(values.date_text.as_deref(), Some("1987-09"));
            assert_eq!(values.track_number, Some(4));
            assert_eq!(values.disc_number, Some(2));
            assert_eq!(values.duration_ms, Some(100));
            assert_eq!(values.sample_rate, Some(44_100));
            assert_eq!(values.channels, Some(2));
        }

        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(reopened.track_count_for(source_id).unwrap(), 1);
        let unchanged = reopened
            .metadata_for_track(source_id, "tagged.flac")
            .unwrap()
            .unwrap();
        assert_eq!(
            unchanged.last_success.unwrap().track_artists,
            ["Track One", "Track Two"]
        );
        assert_database_integrity(&f.db());
    }

    #[test]
    fn mp4_aac_and_alac_metadata_persist_and_unchanged_files_are_not_reparsed() {
        use lofty::tag::ItemKey;

        let f = Fixture::new();
        let root = f.dir("MP4");
        let aac = metadata::synthetic_mp4(
            include_bytes!("fixtures/synthetic-aac.m4a"),
            &[
                (ItemKey::TrackTitle, "Synthetic AAC"),
                (ItemKey::TrackArtist, "Singer One"),
                (ItemKey::TrackArtist, "Singer Two"),
                (ItemKey::AlbumArtist, "Album Project"),
                (ItemKey::AlbumTitle, "AAC Album"),
                (ItemKey::Genre, "Jazz"),
                (ItemKey::RecordingDate, "2025-04"),
                (ItemKey::TrackNumber, "3/10"),
                (ItemKey::DiscNumber, "2/2"),
            ],
        );
        Fixture::file(&root.join("aac.m4a"), &aac);
        Fixture::file(
            &root.join("alac.mp4"),
            &metadata::synthetic_mp4(
                include_bytes!("fixtures/synthetic-alac.m4a"),
                &[(ItemKey::TrackTitle, "Synthetic ALAC")],
            ),
        );

        let mut catalog = Catalog::open(f.db()).unwrap();
        let report = catalog.add_source(&root, "MP4 audio").unwrap();
        let id = report.source_id;
        assert_eq!(report.scan.metrics.files_examined, 2);
        assert_eq!(report.scan.metrics.files_opened_for_extraction, 2);
        assert_eq!(report.scan.metrics.files_processed_by_parser, 2);
        assert_eq!(report.scan.metrics.successful_extractions, 2);
        assert_eq!(report.scan.metrics.metadata_records_written, 2);

        let aac_metadata = catalog.metadata_for_track(id, "aac.m4a").unwrap().unwrap();
        assert_eq!(aac_metadata.file_format, "mp4_aac");
        assert_eq!(aac_metadata.parser_version, metadata::MP4_PARSER_VERSION);
        let values = aac_metadata.last_success.unwrap();
        assert_eq!(values.title.as_deref(), Some("Synthetic AAC"));
        assert_eq!(values.track_artists, ["Singer One", "Singer Two"]);
        assert_eq!(values.album_artists, ["Album Project"]);
        assert_eq!(values.album.as_deref(), Some("AAC Album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2025-04"));
        assert_eq!(values.track_number, Some(3));
        assert_eq!(values.disc_number, Some(2));
        assert_eq!(values.duration_ms, Some(173));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));

        let alac_metadata = catalog.metadata_for_track(id, "alac.mp4").unwrap().unwrap();
        assert_eq!(alac_metadata.file_format, "mp4_alac");
        assert_eq!(
            alac_metadata.last_success.unwrap().sample_rate,
            Some(48_000)
        );

        let unchanged = catalog.verify_source(id).unwrap();
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
        assert_eq!(unchanged.metrics.metadata_records_written, 0);
        assert_eq!(catalog.track_count_for(id).unwrap(), 2);

        Fixture::file(
            &root.join("aac.m4a"),
            &metadata::synthetic_mp4(
                include_bytes!("fixtures/synthetic-aac.m4a"),
                &[
                    (ItemKey::TrackTitle, "Changed AAC title"),
                    (ItemKey::TrackArtist, "Singer One"),
                    (ItemKey::TrackArtist, "Singer Two"),
                ],
            ),
        );
        let modified = catalog.verify_source(id).unwrap();
        assert_eq!(modified.metrics.files_opened_for_extraction, 1);
        assert_eq!(modified.metrics.files_processed_by_parser, 1);
        assert_eq!(modified.metrics.metadata_records_written, 1);
        assert_eq!(
            catalog
                .metadata_for_track(id, "aac.m4a")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Changed AAC title")
        );

        let forced = catalog.rebuild_source(id).unwrap();
        assert_eq!(forced.metrics.files_opened_for_extraction, 2);
        assert_eq!(forced.metrics.files_processed_by_parser, 2);
        assert_eq!(forced.metrics.metadata_records_written, 2);

        let reopened = Catalog::open(f.db()).unwrap();
        let restored = reopened.metadata_for_track(id, "aac.m4a").unwrap().unwrap();
        assert_eq!(restored.file_format, "mp4_aac");
        assert_eq!(
            restored.last_success.unwrap().track_artists,
            ["Singer One", "Singer Two"]
        );
    }

    #[test]
    fn old_unsupported_m4a_is_retried_by_the_mp4_parser_version() {
        use lofty::tag::ItemKey;

        let f = Fixture::new();
        let root = f.dir("M4A");
        Fixture::file(
            &root.join("newly-supported.m4a"),
            &metadata::synthetic_mp4(
                include_bytes!("fixtures/synthetic-aac.m4a"),
                &[(ItemKey::TrackTitle, "Now supported")],
            ),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "M4A").unwrap().source_id;
        catalog
            .db
            .connection
            .execute(
                "UPDATE track_metadata SET attempt_status=3,file_format='extension:m4a',\
                 attempt_parser_version=?1,success_size=NULL,success_mtime_ns=NULL,title=NULL \
                 WHERE source_id=?2 AND relative_path=?3",
                rusqlite::params![
                    metadata::PARSER_VERSION,
                    id,
                    val_b(path_codec::encode_relative(Path::new("newly-supported.m4a")).unwrap())
                ],
            )
            .unwrap();

        let report = catalog.verify_source(id).unwrap();
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
        let metadata = catalog
            .metadata_for_track(id, "newly-supported.m4a")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.parser_version, metadata::MP4_PARSER_VERSION);
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Now supported")
        );
    }

    #[test]
    fn mixed_five_codec_catalog_persists_ogg_codec_detected_from_content() {
        use lofty::tag::{ItemKey, TagType};

        let f = Fixture::new();
        let root = f.dir("Mixed");
        Fixture::file(&root.join("pcm.wav"), &pcm_wav(Some("WAV")));
        Fixture::file(
            &root.join("lossless.flac"),
            &metadata::synthetic_flac(&[("TITLE", "FLAC")]),
        );
        Fixture::file(
            &root.join("mpeg.mp3"),
            &metadata::synthetic_mp3(&[(TagType::Id3v2, &[(ItemKey::TrackTitle, "MP3")])], false),
        );
        // Deliberately mismatch suffix and codec to prove content-based Ogg detection.
        Fixture::file(
            &root.join("vorbis.opus"),
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(ItemKey::TrackTitle, "Vorbis")],
            ),
        );
        Fixture::file(
            &root.join("opus.ogg"),
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-opus.opus"),
                &[(ItemKey::TrackTitle, "Opus")],
            ),
        );

        let mut catalog = Catalog::open(f.db()).unwrap();
        let report = catalog.add_source(&root, "Mixed").unwrap();
        let id = report.source_id;
        assert_eq!(report.scan.metrics.files_examined, 5);
        assert_eq!(report.scan.metrics.files_opened_for_extraction, 5);
        assert_eq!(report.scan.metrics.files_processed_by_parser, 5);
        assert_eq!(report.scan.metrics.successful_extractions, 5);
        assert_eq!(report.scan.metrics.metadata_records_written, 5);
        assert_eq!(catalog.track_count_for(id).unwrap(), 5);
        let unchanged = catalog.verify_source(id).unwrap();
        assert_eq!(unchanged.metrics.entries_visited, 5);
        assert_eq!(unchanged.metrics.files_examined, 5);
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
        assert_eq!(unchanged.metrics.metadata_records_written, 0);
        assert_eq!(
            catalog
                .metadata_for_track(id, "vorbis.opus")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Vorbis")
        );
        assert_eq!(
            catalog
                .metadata_for_track(id, "opus.ogg")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Opus")
        );

        drop(catalog);
        fs::remove_dir_all(&root).unwrap();
        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(reopened.track_count_for(id).unwrap(), 5);
        assert_eq!(
            reopened
                .metadata_for_track(id, "opus.ogg")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Opus")
        );
        assert_database_integrity(&f.db());
    }

    #[test]
    fn ogg_verification_is_incremental_and_preserves_last_success_on_parse_error() {
        use lofty::tag::ItemKey;

        let f = Fixture::new();
        let root = f.dir("Ogg");
        let changing_path = root.join("changing.oga");
        Fixture::file(
            &changing_path,
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(ItemKey::TrackTitle, "Initial Vorbis title")],
            ),
        );
        Fixture::file(
            &root.join("stable.opus"),
            include_bytes!("fixtures/synthetic-opus.opus"),
        );

        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "Ogg").unwrap().source_id;
        let unchanged = catalog.verify_source(id).unwrap();
        assert_eq!(unchanged.metrics.files_examined, 2);
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
        assert_eq!(unchanged.metrics.metadata_records_written, 0);

        Fixture::file(
            &changing_path,
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(
                    ItemKey::TrackTitle,
                    "Updated Vorbis title after file changed",
                )],
            ),
        );
        let changed = catalog.verify_source(id).unwrap();
        assert_eq!(changed.metrics.files_modified, 1);
        assert_eq!(changed.metrics.files_opened_for_extraction, 1);
        assert_eq!(changed.metrics.files_processed_by_parser, 1);
        assert_eq!(changed.metrics.metadata_records_written, 1);
        assert_eq!(changed.metrics.files_examined, 2);
        assert_eq!(
            catalog
                .metadata_for_track(id, "changing.oga")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Updated Vorbis title after file changed")
        );

        Fixture::file(&changing_path, b"broken Ogg stream");
        let failed = catalog.verify_source(id).unwrap();
        assert_eq!(failed.metrics.files_opened_for_extraction, 1);
        assert_eq!(failed.metrics.extraction_failures, 1);
        let stale = catalog
            .metadata_for_track(id, "changing.oga")
            .unwrap()
            .unwrap();
        assert_eq!(stale.status, MetadataStatus::Failed);
        assert!(!stale.current_for_file);
        assert_eq!(
            stale.last_success.unwrap().title.as_deref(),
            Some("Updated Vorbis title after file changed")
        );

        Fixture::file(
            &changing_path,
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(ItemKey::TrackTitle, "Recovered title")],
            ),
        );
        let recovered = catalog.verify_source(id).unwrap();
        assert_eq!(recovered.metrics.files_opened_for_extraction, 1);
        assert_eq!(recovered.metrics.successful_extractions, 1);
        assert_eq!(
            catalog
                .metadata_for_track(id, "changing.oga")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Recovered title")
        );

        let forced = catalog.rebuild_source(id).unwrap();
        assert_eq!(forced.metrics.files_opened_for_extraction, 2);
        assert_eq!(forced.metrics.files_processed_by_parser, 2);
        assert_eq!(forced.metrics.metadata_records_written, 2);
        assert_database_integrity(&f.db());
    }

    #[test]
    fn old_unsupported_ogg_is_retried_without_reprocessing_other_formats() {
        use lofty::tag::ItemKey;

        let f = Fixture::new();
        let root = f.dir("OldOgg");
        Fixture::file(&root.join("stable.wav"), &pcm_wav(Some("Stable WAV")));
        Fixture::file(
            &root.join("newly-supported.ogg"),
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-opus.opus"),
                &[(ItemKey::TrackTitle, "Previously unsupported")],
            ),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "OldOgg").unwrap().source_id;
        catalog
            .db
            .connection
            .execute(
                "UPDATE track_metadata SET attempt_status=3,file_format='extension:ogg',\
                 attempt_parser_version=2,success_size=NULL,success_mtime_ns=NULL,title=NULL \
                 WHERE source_id=?1 AND relative_path=?2",
                rusqlite::params![
                    id,
                    path_codec::encode_relative(Path::new("newly-supported.ogg")).unwrap()
                ],
            )
            .unwrap();

        let report = catalog.verify_source(id).unwrap();
        assert_eq!(report.metrics.files_examined, 2);
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
        let metadata = catalog
            .metadata_for_track(id, "newly-supported.ogg")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.parser_version, metadata::OGG_PARSER_VERSION);
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Previously unsupported")
        );
    }

    #[test]
    fn dsd_metadata_persists_and_old_unsupported_attempts_retry_selectively() {
        use lofty::tag::ItemKey;

        let fixture = Fixture::new();
        let root = fixture.dir("DSD");
        Fixture::file(
            &root.join("album.dsf"),
            &catalog_dsf_fixture(&[
                (ItemKey::TrackTitle, "Abacab 東京"),
                (ItemKey::TrackArtist, "Genesis"),
                (ItemKey::TrackArtist, "Guest Voice"),
                (ItemKey::AlbumArtist, "Genesis"),
                (ItemKey::AlbumTitle, "Abacab"),
            ]),
        );
        Fixture::file(&root.join("native.dff"), &catalog_dff_fixture());
        Fixture::file(&root.join("stable.wav"), &pcm_wav(Some("Stable WAV")));
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "DSD").unwrap().source_id;

        // Model previous Unsupported results from the old catalog parser.
        for (path, extension, previous_parser_version) in [
            ("album.dsf", "dsf", metadata::PARSER_VERSION),
            ("native.dff", "dff", metadata::PARSER_VERSION),
        ] {
            catalog
                .db
                .connection
                .execute(
                    "UPDATE track_metadata SET attempt_status=3,file_format=?1,\
                     attempt_parser_version=?2,success_size=NULL,success_mtime_ns=NULL,\
                     title=NULL,track_artist=NULL,album_artist=NULL,album=NULL \
                     WHERE source_id=?3 AND relative_path=?4",
                    rusqlite::params![
                        format!("extension:{extension}"),
                        previous_parser_version,
                        source_id,
                        path_codec::encode_relative(Path::new(path)).unwrap(),
                    ],
                )
                .unwrap();
        }

        let report = catalog.verify_source(source_id).unwrap();
        assert_eq!(report.metrics.files_examined, 3);
        assert_eq!(report.metrics.files_opened_for_extraction, 2);
        assert_eq!(report.metrics.files_processed_by_parser, 2);
        assert_eq!(report.metrics.metadata_records_written, 2);

        let dsf = catalog
            .metadata_for_track(source_id, "album.dsf")
            .unwrap()
            .unwrap();
        assert_eq!(dsf.status, MetadataStatus::ParsedWithTags);
        assert_eq!(dsf.file_format, "dsf");
        assert_eq!(dsf.parser_version, metadata::DSF_PARSER_VERSION);
        let values = dsf.last_success.unwrap();
        assert_eq!(values.track_artists, ["Genesis", "Guest Voice"]);
        assert_eq!(values.album_artists, ["Genesis"]);
        assert_eq!(values.title.as_deref(), Some("Abacab 東京"));

        let dff = catalog
            .metadata_for_track(source_id, "native.dff")
            .unwrap()
            .unwrap();
        assert_eq!(dff.status, MetadataStatus::ParsedWithTags);
        assert_eq!(dff.file_format, "dff");
        assert_eq!(dff.parser_version, metadata::DFF_PARSER_VERSION);
        let values = dff.last_success.unwrap();
        assert_eq!(values.track_artists, ["Artist DFF"]);
        assert_eq!(values.title.as_deref(), Some("Native DFF"));
        assert!(values.album_artists.is_empty());
        assert!(values.album.is_none());

        let stable = catalog
            .metadata_for_track(source_id, "stable.wav")
            .unwrap()
            .unwrap();
        assert_eq!(stable.parser_version, metadata::PARSER_VERSION);
        assert_eq!(stable.status, MetadataStatus::ParsedWithTags);

        drop(catalog);
        let reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(
            reopened
                .metadata_for_track(source_id, "album.dsf")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .track_artists,
            ["Genesis", "Guest Voice"]
        );
    }

    #[test]
    fn dsd_parser_version_rechecks_existing_metadata_without_reprocessing_other_formats() {
        use lofty::tag::ItemKey;

        let fixture = Fixture::new();
        let root = fixture.dir("DSD-version-refresh");
        Fixture::file(
            &root.join("album.dsf"),
            &catalog_dsf_fixture(&[
                (ItemKey::TrackTitle, "Abacab"),
                (ItemKey::TrackArtist, "Genesis"),
                (ItemKey::RecordingDate, "2025"),
            ]),
        );
        Fixture::file(&root.join("stable.wav"), &pcm_wav(Some("Stable WAV")));
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog
            .add_source(&root, "DSD version refresh")
            .unwrap()
            .source_id;

        // Simulate the version-2 parser result that predated the DSD date fix.
        // The previous version matched the generic version, so this must now
        // revisit only the DSD and leave the stable WAV untouched.
        catalog
            .db
            .connection
            .execute(
                "UPDATE track_metadata SET attempt_parser_version=?1,date_text=NULL \
                 WHERE source_id=?2 AND relative_path=?3",
                rusqlite::params![
                    metadata::PARSER_VERSION,
                    source_id,
                    path_codec::encode_relative(Path::new("album.dsf")).unwrap(),
                ],
            )
            .unwrap();

        let report = catalog.verify_source(source_id).unwrap();
        assert_eq!(report.metrics.files_examined, 2);
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);

        let dsf = catalog
            .metadata_for_track(source_id, "album.dsf")
            .unwrap()
            .unwrap();
        assert_eq!(dsf.parser_version, metadata::DSF_PARSER_VERSION);
        assert_eq!(dsf.last_success.unwrap().date_text.as_deref(), Some("2025"));
        let wav = catalog
            .metadata_for_track(source_id, "stable.wav")
            .unwrap()
            .unwrap();
        assert_eq!(wav.parser_version, metadata::PARSER_VERSION);
        assert_eq!(wav.status, MetadataStatus::ParsedWithTags);
    }

    #[test]
    fn metadata_date_parser_bumps_reprocess_only_mp3_and_aiff() {
        use lofty::tag::{ItemKey, TagType};

        let fixture = Fixture::new();
        let root = fixture.dir("metadata-date-version-refresh");
        Fixture::file(
            &root.join("updated.mp3"),
            &metadata::synthetic_mp3(
                &[(
                    TagType::Id3v2,
                    &[
                        (ItemKey::TrackTitle, "Updated MP3"),
                        (ItemKey::RecordingDate, "2001"),
                    ],
                )],
                true,
            ),
        );
        Fixture::file(
            &root.join("updated.aiff"),
            &metadata::synthetic_aiff(
                &[
                    (ItemKey::TrackTitle, "Updated AIFF"),
                    (ItemKey::RecordingDate, "2002"),
                ],
                &[],
                false,
            ),
        );
        Fixture::file(&root.join("stable.wav"), &pcm_wav(Some("Stable WAV")));
        Fixture::file(
            &root.join("stable.flac"),
            &metadata::synthetic_flac(&[("TITLE", "Stable FLAC")]),
        );
        Fixture::file(
            &root.join("stable.m4a"),
            &metadata::synthetic_mp4(
                include_bytes!("fixtures/synthetic-aac.m4a"),
                &[(ItemKey::TrackTitle, "Stable MP4")],
            ),
        );
        Fixture::file(
            &root.join("stable.ogg"),
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(ItemKey::TrackTitle, "Stable Ogg")],
            ),
        );
        Fixture::file(
            &root.join("stable.opus"),
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-opus.opus"),
                &[(ItemKey::TrackTitle, "Stable Opus")],
            ),
        );

        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog
            .add_source(&root, "metadata date version refresh")
            .unwrap()
            .source_id;
        for (path, old_version) in [
            ("updated.mp3", metadata::MP3_PARSER_VERSION - 1),
            ("updated.aiff", metadata::AIFF_PARSER_VERSION - 1),
        ] {
            catalog
                .db
                .connection
                .execute(
                    "UPDATE track_metadata SET attempt_parser_version=?1,date_text=NULL \
                     WHERE source_id=?2 AND relative_path=?3",
                    rusqlite::params![
                        old_version,
                        source_id,
                        path_codec::encode_relative(Path::new(path)).unwrap(),
                    ],
                )
                .unwrap();
        }

        assert!(catalog
            .verify_with_interruption(source_id)
            .unwrap_err()
            .contains("simulated interruption"));
        let interrupted_mp3 = catalog
            .metadata_for_track(source_id, "updated.mp3")
            .unwrap()
            .unwrap();
        assert_eq!(
            interrupted_mp3.parser_version,
            metadata::MP3_PARSER_VERSION - 1
        );
        assert!(interrupted_mp3.last_success.unwrap().date_text.is_none());

        let refreshed = catalog.verify_source(source_id).unwrap();
        assert_eq!(refreshed.metrics.files_examined, 7);
        assert_eq!(refreshed.metrics.files_opened_for_extraction, 2);
        assert_eq!(refreshed.metrics.files_processed_by_parser, 2);
        assert_eq!(refreshed.metrics.metadata_records_written, 2);
        assert_eq!(
            catalog
                .metadata_for_track(source_id, "updated.mp3")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .date_text
                .as_deref(),
            Some("2001")
        );
        assert_eq!(
            catalog
                .metadata_for_track(source_id, "updated.aiff")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .date_text
                .as_deref(),
            Some("2002")
        );

        let unchanged = catalog.verify_source(source_id).unwrap();
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
        assert_eq!(unchanged.metrics.metadata_records_written, 0);
        drop(catalog);

        let reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(
            reopened
                .metadata_for_track(source_id, "updated.mp3")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .date_text
                .as_deref(),
            Some("2001")
        );
        assert_eq!(
            reopened
                .metadata_for_track(source_id, "updated.aiff")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .date_text
                .as_deref(),
            Some("2002")
        );
    }

    #[test]
    fn aac_ape_and_aifc_metadata_persist_and_retry_previous_unsupported_only() {
        use lofty::tag::ItemKey;

        let fixture = Fixture::new();
        let root = fixture.dir("metadata-parity");
        Fixture::file(
            &root.join("stream.aac"),
            &catalog_aac_fixture(&[
                (ItemKey::TrackTitle, "AAC title"),
                (ItemKey::TrackArtist, "AAC artist"),
                (ItemKey::AlbumArtist, "AAC album artist"),
                (ItemKey::AlbumTitle, "AAC album"),
                (ItemKey::TrackNumber, "2/9"),
            ]),
        );
        Fixture::file(
            &root.join("record.ape"),
            &catalog_ape_fixture(&[
                ("Title", "APE title"),
                ("Artist", "APE artist"),
                ("Album Artist", "APE album artist"),
                ("Album", "APE album"),
                ("Track", "3/10"),
            ]),
        );
        Fixture::file(
            &root.join("piece.aifc"),
            &metadata::synthetic_aiff(
                &[
                    (ItemKey::TrackTitle, "AIFC title"),
                    (ItemKey::TrackArtist, "AIFC artist"),
                    (ItemKey::AlbumArtist, "AIFC album artist"),
                    (ItemKey::AlbumTitle, "AIFC album"),
                    (ItemKey::TrackNumber, "4"),
                ],
                &[],
                true,
            ),
        );
        Fixture::file(
            &root.join("unsupported.wma"),
            b"ASF/WMA fixture without a parser",
        );
        Fixture::file(&root.join("stable.wav"), &pcm_wav(Some("Stable WAV")));

        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog
            .add_source(&root, "metadata parity")
            .unwrap()
            .source_id;
        for (path, format, parser_version) in [
            ("stream.aac", "aac_adts", metadata::AAC_PARSER_VERSION),
            ("record.ape", "ape", metadata::APE_PARSER_VERSION),
            ("piece.aifc", "aifc_metadata", metadata::AIFC_PARSER_VERSION),
        ] {
            let record = catalog
                .metadata_for_track(source_id, path)
                .unwrap()
                .unwrap();
            assert_eq!(record.status, MetadataStatus::ParsedWithTags);
            assert_eq!(record.file_format, format);
            assert_eq!(record.parser_version, parser_version);
        }
        let unsupported = catalog
            .metadata_for_track(source_id, "unsupported.wma")
            .unwrap()
            .unwrap();
        assert_eq!(unsupported.status, MetadataStatus::Unsupported);
        assert_eq!(unsupported.file_format, "extension:wma");

        for (path, extension) in [
            ("stream.aac", "aac"),
            ("record.ape", "ape"),
            ("piece.aifc", "aifc"),
        ] {
            catalog
                .db
                .connection
                .execute(
                    "UPDATE track_metadata SET attempt_status=3,file_format=?1,\
                     attempt_parser_version=?2,success_size=NULL,success_mtime_ns=NULL,\
                     title=NULL,track_artist=NULL,album_artist=NULL,album=NULL \
                     WHERE source_id=?3 AND relative_path=?4",
                    rusqlite::params![
                        format!("extension:{extension}"),
                        metadata::PARSER_VERSION,
                        source_id,
                        path_codec::encode_relative(Path::new(path)).unwrap(),
                    ],
                )
                .unwrap();
        }
        let report = catalog.verify_source(source_id).unwrap();
        assert_eq!(report.metrics.files_examined, 5);
        assert_eq!(report.metrics.files_opened_for_extraction, 3);
        assert_eq!(report.metrics.files_processed_by_parser, 3);
        assert_eq!(report.metrics.metadata_records_written, 3);

        let aac = catalog
            .metadata_for_track(source_id, "stream.aac")
            .unwrap()
            .unwrap()
            .last_success
            .unwrap();
        assert_eq!(aac.track_artists, ["AAC artist"]);
        assert_eq!(aac.album_artists, ["AAC album artist"]);
        assert_eq!(aac.track_number, Some(2));
        let ape = catalog
            .metadata_for_track(source_id, "record.ape")
            .unwrap()
            .unwrap()
            .last_success
            .unwrap();
        assert_eq!(ape.track_artists, ["APE artist"]);
        assert_eq!(ape.album_artists, ["APE album artist"]);
        assert_eq!(ape.track_number, Some(3));
        let aifc = catalog
            .metadata_for_track(source_id, "piece.aifc")
            .unwrap()
            .unwrap()
            .last_success
            .unwrap();
        assert_eq!(aifc.track_artists, ["AIFC artist"]);
        assert_eq!(aifc.album_artists, ["AIFC album artist"]);
        assert_eq!(aifc.track_number, Some(4));

        let wav = catalog
            .metadata_for_track(source_id, "stable.wav")
            .unwrap()
            .unwrap();
        assert_eq!(wav.parser_version, metadata::PARSER_VERSION);
        assert_eq!(wav.status, MetadataStatus::ParsedWithTags);
        let wma = catalog
            .metadata_for_track(source_id, "unsupported.wma")
            .unwrap()
            .unwrap();
        assert_eq!(wma.status, MetadataStatus::Unsupported);

        drop(catalog);
        let reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(
            reopened
                .metadata_for_track(source_id, "record.ape")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .album_artists,
            ["APE album artist"]
        );
    }

    #[test]
    fn persisted_artist_projection_deduplicates_legacy_rows_without_losing_order() {
        let fixture = Fixture::new();
        let root = fixture.dir("artist-dedup");
        Fixture::file(
            &root.join("track.flac"),
            &metadata::synthetic_flac(&[
                ("ARTIST", "Artist"),
                ("ARTIST", " Artist "),
                ("ARTIST", "Guest; Feature"),
                ("ALBUMARTIST", "Album Artist"),
                ("ALBUMARTIST", " Album Artist "),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "artist dedup").unwrap().source_id;
        let relative = path_codec::encode_relative(Path::new("track.flac")).unwrap();
        let normalized_extra_count: i64 = catalog
            .db
            .connection
            .query_row(
                "SELECT count(*) FROM track_metadata_extra_artists WHERE source_id=? AND relative_path=?",
                rusqlite::params![source_id, relative],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(normalized_extra_count, 1);
        for (role, ordinal, value) in [
            (1_i64, 3_i64, "Artist"),
            (1, 4, "Guest; Feature"),
            (2, 2, "Album Artist"),
        ] {
            catalog
                .db
                .connection
                .execute(
                    "INSERT INTO track_metadata_extra_artists(source_id,relative_path,artist_role,ordinal,value) VALUES(?,?,?,?,?)",
                    rusqlite::params![source_id, relative, role, ordinal, value],
                )
                .unwrap();
        }
        let values = catalog
            .metadata_for_track(source_id, "track.flac")
            .unwrap()
            .unwrap()
            .last_success
            .unwrap();
        assert_eq!(values.track_artists, ["Artist", "Guest; Feature"]);
        assert_eq!(values.album_artists, ["Album Artist"]);
        let view = catalog.source_library_view(source_id).unwrap();
        let projected = view.tracks[0]
            .metadata
            .as_ref()
            .unwrap()
            .last_success
            .as_ref()
            .unwrap();
        assert_eq!(projected.track_artists, ["Artist", "Guest; Feature"]);
        assert_eq!(projected.album_artists, ["Album Artist"]);
    }

    #[test]
    fn adding_ogg_source_b_does_not_visit_source_a_and_stale_ogg_scan_is_discarded() {
        use lofty::tag::ItemKey;

        let f = Fixture::new();
        let a = f.dir("A");
        let b = f.dir("B");
        let a_track = a.join("a.ogg");
        Fixture::file(
            &a_track,
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(ItemKey::TrackTitle, "A original")],
            ),
        );
        Fixture::file(
            &b.join("b.opus"),
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-opus.opus"),
                &[(ItemKey::TrackTitle, "B only")],
            ),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let a_id = catalog.add_source(&a, "A").unwrap().source_id;
        let b_report = catalog.add_source(&b, "B").unwrap();
        assert_eq!(b_report.scan.metrics.sources_examined, 1);
        assert_eq!(b_report.scan.metrics.files_examined, 1);
        assert_eq!(b_report.scan.metrics.files_opened_for_extraction, 1);
        assert_eq!(catalog.track_count_for(a_id).unwrap(), 1);

        Fixture::file(
            &a_track,
            &metadata::synthetic_ogg(
                include_bytes!("fixtures/synthetic-vorbis.ogg"),
                &[(ItemKey::TrackTitle, "Stale scan result")],
            ),
        );
        let stale = catalog.verify_with_hook(a_id, || {
            Fixture::file(
                &a_track,
                &metadata::synthetic_ogg(
                    include_bytes!("fixtures/synthetic-vorbis.ogg"),
                    &[(ItemKey::TrackTitle, "Newer committed result")],
                ),
            );
            let mut newer = Catalog::open(f.db()).unwrap();
            assert_eq!(
                newer
                    .rebuild_source(a_id)
                    .unwrap()
                    .metrics
                    .files_processed_by_parser,
                1
            );
        });
        assert!(stale.unwrap_err().contains("stale verification discarded"));
        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(
            reopened
                .metadata_for_track(a_id, "a.ogg")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Newer committed result")
        );
        assert_database_integrity(&f.db());
    }

    #[test]
    fn flac_without_comments_and_invalid_flac_have_distinct_persisted_states() {
        let f = Fixture::new();
        let root = f.dir("FLAC");
        Fixture::file(&root.join("untagged.flac"), &metadata::synthetic_flac(&[]));
        Fixture::file(&root.join("broken.flac"), b"fLaC truncated metadata");
        let mut catalog = Catalog::open(f.db()).unwrap();
        let report = catalog.add_source(&root, "FLAC").unwrap();
        assert_eq!(report.scan.metrics.files_examined, 2);
        assert_eq!(report.scan.metrics.files_opened_for_extraction, 2);
        assert_eq!(report.scan.metrics.files_processed_by_parser, 2);
        assert_eq!(report.scan.metrics.successful_extractions, 1);
        assert_eq!(report.scan.metrics.files_without_tags, 1);
        assert_eq!(report.scan.metrics.extraction_failures, 1);
        assert_eq!(report.scan.metrics.metadata_records_written, 2);

        let untagged = catalog
            .metadata_for_track(report.source_id, "untagged.flac")
            .unwrap()
            .unwrap();
        assert_eq!(untagged.status, MetadataStatus::ParsedWithoutTags);
        assert_eq!(untagged.last_success.unwrap().sample_rate, Some(44_100));
        let broken = catalog
            .metadata_for_track(report.source_id, "broken.flac")
            .unwrap()
            .unwrap();
        assert_eq!(broken.status, MetadataStatus::Failed);
        assert!(broken.last_success.is_none());
        assert!(broken.error.is_some());
    }

    #[test]
    fn flac_unchanged_modified_failed_and_forced_extractions_are_incremental() {
        let f = Fixture::new();
        let root = f.dir("FLAC");
        let changed_path = root.join("changed.flac");
        let stable_path = root.join("stable.flac");
        Fixture::file(
            &changed_path,
            &metadata::synthetic_flac(&[("TITLE", "First")]),
        );
        Fixture::file(
            &stable_path,
            &metadata::synthetic_flac(&[("TITLE", "Stable")]),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "FLAC").unwrap().source_id;

        let unchanged = catalog.verify_source(source_id).unwrap();
        assert_eq!(unchanged.metrics.files_examined, 2);
        assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
        assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
        assert_eq!(unchanged.metrics.metadata_records_written, 0);

        Fixture::file(
            &changed_path,
            &metadata::synthetic_flac(&[("TITLE", "Updated FLAC title")]),
        );
        let modified = catalog.verify_source(source_id).unwrap();
        assert_eq!(modified.metrics.files_modified, 1);
        assert_eq!(modified.metrics.files_opened_for_extraction, 1);
        assert_eq!(modified.metrics.files_processed_by_parser, 1);
        assert_eq!(modified.metrics.metadata_records_written, 1);
        assert_eq!(
            catalog
                .metadata_for_track(source_id, "changed.flac")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("Updated FLAC title")
        );

        Fixture::file(&changed_path, b"not valid FLAC anymore");
        let failed = catalog.verify_source(source_id).unwrap();
        assert_eq!(failed.metrics.files_opened_for_extraction, 1);
        assert_eq!(failed.metrics.extraction_failures, 1);
        let stale = catalog
            .metadata_for_track(source_id, "changed.flac")
            .unwrap()
            .unwrap();
        assert_eq!(stale.status, MetadataStatus::Failed);
        assert!(!stale.current_for_file);
        assert_eq!(
            stale.last_success.unwrap().title.as_deref(),
            Some("Updated FLAC title")
        );

        let rebuilt = catalog.rebuild_source(source_id).unwrap();
        assert_eq!(rebuilt.metrics.files_opened_for_extraction, 2);
        assert_eq!(rebuilt.metrics.files_processed_by_parser, 2);
        assert_eq!(rebuilt.metrics.metadata_records_written, 2);
    }

    #[test]
    fn aiff_extraction_is_format_versioned_incremental_and_persistent() {
        use lofty::tag::ItemKey;

        let f = Fixture::new();
        let root = f.dir("AIFF");
        let path = root.join("track.aiff");
        Fixture::file(
            &path,
            &metadata::synthetic_aiff(&[(ItemKey::TrackTitle, "Initial AIFF")], &[], false),
        );
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "AIFF").unwrap().source_id;
            let metadata = catalog
                .metadata_for_track(source_id, "track.aiff")
                .unwrap()
                .unwrap();
            assert_eq!(metadata.file_format, "aiff_pcm");
            assert_eq!(metadata.parser_version, metadata::AIFF_PARSER_VERSION);
            assert_eq!(
                metadata.last_success.unwrap().title.as_deref(),
                Some("Initial AIFF")
            );

            let unchanged = catalog.verify_source(source_id).unwrap();
            assert_eq!(unchanged.metrics.files_examined, 1);
            assert_eq!(unchanged.metrics.files_opened_for_extraction, 0);
            assert_eq!(unchanged.metrics.files_processed_by_parser, 0);
            assert_eq!(unchanged.metrics.metadata_records_written, 0);

            Fixture::file(
                &path,
                &metadata::synthetic_aiff(
                    &[(ItemKey::TrackTitle, "Updated AIFF title")],
                    &[],
                    false,
                ),
            );
            let modified = catalog.verify_source(source_id).unwrap();
            assert_eq!(modified.metrics.files_modified, 1);
            assert_eq!(modified.metrics.files_opened_for_extraction, 1);
            assert_eq!(modified.metrics.files_processed_by_parser, 1);
            assert_eq!(modified.metrics.metadata_records_written, 1);

            Fixture::file(&path, b"FORM truncated AIFF");
            let failed = catalog.verify_source(source_id).unwrap();
            assert_eq!(failed.metrics.files_opened_for_extraction, 1);
            assert_eq!(failed.metrics.extraction_failures, 1);
            let stale = catalog
                .metadata_for_track(source_id, "track.aiff")
                .unwrap()
                .unwrap();
            assert_eq!(stale.status, MetadataStatus::Failed);
            assert!(!stale.current_for_file);
            assert_eq!(
                stale.last_success.unwrap().title.as_deref(),
                Some("Updated AIFF title")
            );

            Fixture::file(
                &path,
                &metadata::synthetic_aiff(
                    &[(ItemKey::TrackTitle, "Updated AIFF title")],
                    &[],
                    false,
                ),
            );
            let recovered = catalog.verify_source(source_id).unwrap();
            assert_eq!(recovered.metrics.files_opened_for_extraction, 1);
            assert_eq!(recovered.metrics.successful_extractions, 1);

            let forced = catalog.rebuild_source(source_id).unwrap();
            assert_eq!(forced.metrics.files_opened_for_extraction, 1);
            assert_eq!(forced.metrics.files_processed_by_parser, 1);
            assert_eq!(forced.metrics.metadata_records_written, 1);
        }

        let reopened = Catalog::open(f.db()).unwrap();
        let metadata = reopened
            .metadata_for_track(source_id, "track.aiff")
            .unwrap()
            .unwrap();
        assert!(metadata.current_for_file);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Updated AIFF title")
        );
    }

    #[test]
    fn old_unsupported_flac_attempt_is_retried_after_parser_version_change() {
        let f = Fixture::new();
        let root = f.dir("FLAC");
        Fixture::file(
            &root.join("old-unsupported.flac"),
            &metadata::synthetic_flac(&[("TITLE", "Now supported")]),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "FLAC").unwrap().source_id;
        catalog
            .db
            .connection
            .execute(
                "UPDATE track_metadata SET attempt_status=3,file_format='extension:flac',\
                 attempt_parser_version=1,success_size=NULL,success_mtime_ns=NULL,title=NULL \
                 WHERE source_id=?1",
                [source_id],
            )
            .unwrap();

        let report = catalog.verify_source(source_id).unwrap();
        assert_eq!(report.metrics.files_opened_for_extraction, 1);
        assert_eq!(report.metrics.files_processed_by_parser, 1);
        assert_eq!(report.metrics.successful_extractions, 1);
        assert_eq!(report.metrics.metadata_records_written, 1);
        let metadata = catalog
            .metadata_for_track(source_id, "old-unsupported.flac")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.parser_version, metadata::PARSER_VERSION);
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Now supported")
        );
    }

    #[test]
    fn mixed_wave_flac_mp3_source_and_new_source_b_only_process_the_new_root() {
        use lofty::tag::{ItemKey, TagType};

        let f = Fixture::new();
        let a = f.dir("A");
        let b = f.dir("B");
        Fixture::file(&a.join("a.wav"), &pcm_wav(Some("WAV A")));
        Fixture::file(
            &a.join("a.flac"),
            &metadata::synthetic_flac(&[("TITLE", "FLAC A")]),
        );
        Fixture::file(
            &a.join("a.mp3"),
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "MP3 A")])],
                false,
            ),
        );
        Fixture::file(
            &b.join("b.flac"),
            &metadata::synthetic_flac(&[("TITLE", "FLAC B")]),
        );
        Fixture::file(
            &b.join("b.mp3"),
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "MP3 B")])],
                false,
            ),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let a_id = catalog.add_source(&a, "A").unwrap().source_id;
        let b_report = catalog.add_source(&b, "B").unwrap();
        assert_eq!(b_report.scan.metrics.sources_examined, 1);
        assert_eq!(b_report.scan.metrics.files_examined, 2);
        assert_eq!(b_report.scan.metrics.files_opened_for_extraction, 2);
        assert_eq!(b_report.scan.metrics.files_processed_by_parser, 2);
        assert_eq!(catalog.track_count().unwrap(), 5);
        assert_eq!(catalog.track_count_for(a_id).unwrap(), 3);
        assert_eq!(
            catalog
                .metadata_for_track(a_id, "a.flac")
                .unwrap()
                .unwrap()
                .last_success
                .unwrap()
                .title
                .as_deref(),
            Some("FLAC A")
        );
        assert_database_integrity(&f.db());
    }

    #[test]
    fn obsolete_flac_metadata_verification_cannot_replace_newer_result() {
        let f = Fixture::new();
        let root = f.dir("FLAC");
        let path = root.join("one.flac");
        Fixture::file(&path, &metadata::synthetic_flac(&[("TITLE", "Original")]));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "FLAC").unwrap().source_id;
        Fixture::file(
            &path,
            &metadata::synthetic_flac(&[("TITLE", "Newer committed FLAC result")]),
        );

        let stale = catalog.verify_with_hook(source_id, || {
            let mut newer = Catalog::open(f.db()).unwrap();
            let report = newer.rebuild_source(source_id).unwrap();
            assert_eq!(report.metrics.files_processed_by_parser, 1);
        });
        assert!(stale.unwrap_err().contains("stale verification discarded"));
        let reopened = Catalog::open(f.db()).unwrap();
        let metadata = reopened
            .metadata_for_track(source_id, "one.flac")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.status, MetadataStatus::ParsedWithTags);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Newer committed FLAC result")
        );
    }

    #[test]
    fn obsolete_mp3_metadata_verification_cannot_replace_newer_result() {
        use lofty::tag::{ItemKey, TagType};

        let f = Fixture::new();
        let root = f.dir("MP3");
        let path = root.join("one.mp3");
        Fixture::file(
            &path,
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "Original")])],
                false,
            ),
        );
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "MP3").unwrap().source_id;
        Fixture::file(
            &path,
            &metadata::synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "Newer MP3 result")])],
                false,
            ),
        );

        let stale = catalog.verify_with_hook(source_id, || {
            let mut newer = Catalog::open(f.db()).unwrap();
            let report = newer.rebuild_source(source_id).unwrap();
            assert_eq!(report.metrics.files_processed_by_parser, 1);
        });
        assert!(stale.unwrap_err().contains("stale verification discarded"));
        let reopened = Catalog::open(f.db()).unwrap();
        let track = reopened
            .metadata_for_track(source_id, "one.mp3")
            .unwrap()
            .unwrap();
        assert!(track.current_for_file);
        assert_eq!(
            track.last_success.unwrap().title.as_deref(),
            Some("Newer MP3 result")
        );
    }

    #[test]
    fn stale_metadata_verification_finishing_late_cannot_replace_newer_result() {
        let f = Fixture::new();
        let root = f.dir("A");
        let path = root.join("one.wav");
        Fixture::file(&path, &pcm_wav(Some("Initial")));
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        Fixture::file(&path, &pcm_wav(Some("Stale scan result")));

        let (scanned_tx, scanned_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let database = f.db();
        let stale = thread::spawn(move || {
            let mut stale_catalog = Catalog::open(database).unwrap();
            stale_catalog.verify_with_hook(id, || {
                scanned_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            })
        });
        scanned_rx.recv().unwrap();

        Fixture::file(&path, &pcm_wav(Some("Fresh committed result is longer")));
        let latest = catalog.verify_source(id).unwrap();
        assert_eq!(latest.generation, 2);
        resume_tx.send(()).unwrap();
        assert!(stale
            .join()
            .unwrap()
            .unwrap_err()
            .contains("generation advanced"));

        let metadata = catalog.metadata_for_track(id, "one.wav").unwrap().unwrap();
        assert!(metadata.current_for_file);
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Fresh committed result is longer")
        );
    }

    fn one_track_catalog() -> (Fixture, PathBuf, PathBuf, i64) {
        let fixture = Fixture::new();
        let root = fixture.dir("source");
        let track = root.join("one.flac");
        Fixture::file(&track, b"old");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let id = catalog.add_source(&root, "source").unwrap().source_id;
        drop(catalog);
        (fixture, root, track, id)
    }

    #[test]
    fn source_generation_guard_serializes_independent_sqlite_writers() {
        let (fixture, _root, _track, source_id) = one_track_catalog();
        let mut lease = Catalog::open(fixture.db()).unwrap();
        let value = lease
            .with_source_generation_guard(&[(source_id, 1)], || {
                let writer = Connection::open(fixture.db()).unwrap();
                writer.busy_timeout(Duration::ZERO).unwrap();
                assert!(writer.execute_batch("BEGIN IMMEDIATE").is_err());
                Ok("lease-held")
            })
            .unwrap();
        assert_eq!(value, "lease-held");

        let writer = Connection::open(fixture.db()).unwrap();
        writer.busy_timeout(Duration::ZERO).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn source_generation_guard_rejects_stale_and_conflicting_bindings() {
        let (fixture, _root, track, source_id) = one_track_catalog();
        Fixture::file(&track, b"updated");
        let mut verifier = Catalog::open(fixture.db()).unwrap();
        assert_eq!(verifier.verify_source(source_id).unwrap().generation, 2);

        let mut lease = Catalog::open(fixture.db()).unwrap();
        let mut called = false;
        assert!(lease
            .with_source_generation_guard(&[(source_id, 1)], || {
                called = true;
                Ok(())
            })
            .is_err());
        assert!(!called, "stale generation must not enter the protected effect");
        assert!(lease
            .with_source_generation_guard(&[(source_id, 1), (source_id, 2)], || Ok(()))
            .is_err());
        assert_eq!(Catalog::open(fixture.db()).unwrap().sources().unwrap()[0].generation, 2);
    }

    #[cfg(target_os = "linux")]
    fn child_pause_at(stage: &str) {
        let marker = PathBuf::from(std::env::var_os("SONANTE_SQLITE_B19_CHILD_MARKER").unwrap());
        fs::write(marker, stage).unwrap();
        let mut byte = [0_u8; 1];
        let _ = std::io::stdin().read_exact(&mut byte);
        std::process::abort();
    }

    #[cfg(target_os = "linux")]
    fn spawn_blocked_child(
        action: &str,
        database: &Path,
        source_id: i64,
        expected_stage: &str,
    ) -> Child {
        let marker = database.with_extension(format!("{action}.stage"));
        let _ = fs::remove_file(&marker);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::crash_child_entry",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SONANTE_SQLITE_B19_CHILD_ACTION", action)
            .env(
                "SONANTE_SQLITE_B19_CHILD_DATABASE",
                database.to_str().unwrap(),
            )
            .env("SONANTE_SQLITE_B19_CHILD_SOURCE_ID", source_id.to_string())
            .env("SONANTE_SQLITE_B19_CHILD_MARKER", &marker)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            if marker.exists() {
                let stage = fs::read_to_string(&marker).unwrap();
                assert_eq!(stage, expected_stage);
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("child exited before {expected_stage}: {status}");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child timed out before synchronization point {expected_stage}"
            );
            thread::sleep(Duration::from_millis(5));
        }
        child
    }

    #[cfg(target_os = "linux")]
    fn abruptly_terminate_harness_child(mut child: Child) {
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&[1])
            .expect("send abort token only to the child spawned by this test");
        let status = child.wait().unwrap();
        assert!(
            !status.success(),
            "forced child abort unexpectedly succeeded"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn crash_child_entry() {
        let Ok(action) = std::env::var("SONANTE_SQLITE_B19_CHILD_ACTION") else {
            return;
        };
        let database =
            PathBuf::from(std::env::var_os("SONANTE_SQLITE_B19_CHILD_DATABASE").unwrap());
        let source_id: i64 = std::env::var("SONANTE_SQLITE_B19_CHILD_SOURCE_ID")
            .unwrap()
            .parse()
            .unwrap();
        let mut catalog = Catalog::open(database).unwrap();
        match action.as_str() {
            "during_scan" => {
                let mut paused = false;
                let _ = catalog.verify_with_walk_hook(source_id, |_| {
                    if !paused {
                        paused = true;
                        child_pause_at("during_scan_before_open");
                    }
                });
                panic!("scan child unexpectedly resumed");
            }
            "before_commit" => {
                let _ = catalog.verify_with_transaction_hook(source_id, || {
                    child_pause_at("after_write_before_commit");
                });
                panic!("transaction child unexpectedly resumed");
            }
            "after_commit" => {
                catalog
                    .db
                    .connection
                    .execute_batch("PRAGMA wal_autocheckpoint=0;")
                    .unwrap();
                let report = catalog.verify_source(source_id).unwrap();
                assert_eq!(report.generation, 2);
                child_pause_at("commit_returned");
            }
            _ => panic!("unknown child action"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn abrupt_process_exit_during_scan_preserves_previous_generation_then_allows_retry() {
        let (fixture, _root, track, source_id) = one_track_catalog();
        Fixture::file(&track, b"updated-after-scan-start");

        let child = spawn_blocked_child(
            "during_scan",
            &fixture.db(),
            source_id,
            "during_scan_before_open",
        );
        abruptly_terminate_harness_child(child);

        assert_single_track_state(&fixture.db(), source_id, 1, 3);
        {
            let mut reopened = Catalog::open(fixture.db()).unwrap();
            assert_single_track_state(&fixture.db(), source_id, 1, 3);
            assert_eq!(reopened.verify_source(source_id).unwrap().generation, 2);
        }
        assert_single_track_state(&fixture.db(), source_id, 2, 24);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn abrupt_process_exit_after_transaction_writes_before_commit_rolls_back_completely() {
        let fixture = Fixture::new();
        let root = fixture.dir("source");
        make_audio(&root, 3);
        let mut initial = Catalog::open(fixture.db()).unwrap();
        let source_id = initial.add_source(&root, "source").unwrap().source_id;
        let before = initial.tracks(source_id).unwrap();
        drop(initial);
        Fixture::file(
            &root.join("track-00000.flac"),
            b"updated-before-transaction",
        );
        fs::remove_file(root.join("track-00001.flac")).unwrap();
        Fixture::file(&root.join("new.flac"), b"new-record");

        let child = spawn_blocked_child(
            "before_commit",
            &fixture.db(),
            source_id,
            "after_write_before_commit",
        );
        abruptly_terminate_harness_child(child);

        let reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(reopened.tracks(source_id).unwrap(), before);
        assert_eq!(reopened.sources().unwrap()[0].generation, 1);
        drop(reopened);
        assert_database_integrity(&fixture.db());

        let mut reopened = Catalog::open(fixture.db()).unwrap();
        let report = reopened.verify_source(source_id).unwrap();
        assert_eq!(report.generation, 2);
        assert_eq!(report.metrics.files_new, 1);
        assert_eq!(report.metrics.files_modified, 1);
        assert_eq!(report.metrics.files_removed, 1);
        let tracks = reopened.tracks(source_id).unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(
            tracks
                .iter()
                .map(|track| track.relative_path.as_path())
                .collect::<Vec<_>>(),
            [
                Path::new("new.flac"),
                Path::new("track-00000.flac"),
                Path::new("track-00002.flac")
            ]
        );
        assert_eq!(tracks[0].generation, 2);
        assert_eq!(tracks[1].generation, 2);
        assert_eq!(tracks[2].generation, 1);
        assert_database_integrity(&fixture.db());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn abrupt_process_exit_after_commit_recovers_committed_wal_state() {
        let (fixture, _root, track, source_id) = one_track_catalog();
        Fixture::file(&track, b"committed-before-process-exit");

        let child =
            spawn_blocked_child("after_commit", &fixture.db(), source_id, "commit_returned");
        let wal = PathBuf::from(format!("{}-wal", fixture.db().display()));
        assert!(
            wal.exists(),
            "expected WAL to remain before child termination"
        );
        assert!(fs::metadata(&wal).unwrap().len() > 0);
        abruptly_terminate_harness_child(child);

        assert_single_track_state(&fixture.db(), source_id, 2, 29);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn partial_enumeration_failure_preserves_catalog_and_later_verification_recovers() {
        let fixture = Fixture::new();
        let root = fixture.dir("source");
        make_audio(&root, 3);
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "source").unwrap().source_id;
        let before = catalog.tracks(source_id).unwrap();
        Fixture::file(&root.join("track-00000.flac"), b"changed");
        fs::remove_file(root.join("track-00001.flac")).unwrap();

        let error = catalog
            .verify_with_scan_failure_after(source_id, 2)
            .unwrap_err();
        assert!(
            error.contains("enumeration error after 1 entries"),
            "{error}"
        );
        assert_eq!(catalog.tracks(source_id).unwrap(), before);
        assert_eq!(catalog.sources().unwrap()[0].generation, 1);
        drop(catalog);
        assert_database_integrity(&fixture.db());

        let mut reopened = Catalog::open(fixture.db()).unwrap();
        let report = reopened.verify_source(source_id).unwrap();
        assert_eq!(report.generation, 2);
        assert_eq!(reopened.track_count_for(source_id).unwrap(), 2);
        assert_database_integrity(&fixture.db());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn metadata_read_failure_preserves_catalog_and_later_verification_recovers() {
        let fixture = Fixture::new();
        let root = fixture.dir("source");
        Fixture::file(&root.join("one.flac"), b"old");
        Fixture::file(&root.join("two.flac"), b"kept");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "source").unwrap().source_id;
        let before = catalog.tracks(source_id).unwrap();
        Fixture::file(&root.join("one.flac"), b"new-metadata");

        assert!(catalog
            .verify_with_metadata_failure(source_id, Path::new("one.flac"))
            .unwrap_err()
            .contains("injected file metadata error"));
        assert_eq!(catalog.tracks(source_id).unwrap(), before);
        assert_eq!(catalog.sources().unwrap()[0].generation, 1);
        drop(catalog);
        assert_database_integrity(&fixture.db());

        let mut reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(reopened.verify_source(source_id).unwrap().generation, 2);
        assert_eq!(reopened.tracks(source_id).unwrap()[0].size, 12);
        assert_database_integrity(&fixture.db());
    }

    #[test]
    fn temporarily_unavailable_source_preserves_catalog_and_recovers_when_restored() {
        let (fixture, root, _track, source_id) = one_track_catalog();
        let unavailable = fixture.path("source-offline");
        fs::rename(&root, &unavailable).unwrap();
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        assert!(catalog.verify_source(source_id).is_err());
        assert_single_track_state(&fixture.db(), source_id, 1, 3);
        drop(catalog);

        fs::rename(&unavailable, &root).unwrap();
        let mut reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(reopened.verify_source(source_id).unwrap().generation, 1);
        assert_single_track_state(&fixture.db(), source_id, 1, 3);
    }

    #[test]
    fn empty_available_source_is_distinct_from_unavailable_and_removes_rows() {
        let (fixture, root, _track, source_id) = one_track_catalog();
        fs::remove_file(root.join("one.flac")).unwrap();
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let report = catalog.verify_source(source_id).unwrap();
        assert_eq!(report.generation, 2);
        assert_eq!(report.metrics.files_removed, 1);
        assert_eq!(catalog.track_count_for(source_id).unwrap(), 0);
        assert_eq!(catalog.sources().unwrap()[0].generation, 2);
        drop(catalog);
        assert_database_integrity(&fixture.db());
    }

    #[test]
    fn injected_sqlite_write_failure_rolls_back_and_allows_later_verification() {
        let (fixture, _root, track, source_id) = one_track_catalog();
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        catalog
            .db
            .connection
            .execute_batch(
                "CREATE TEMP TRIGGER injected_write_failure \
                 BEFORE UPDATE ON tracks BEGIN \
                 SELECT RAISE(ABORT, 'injected write failure'); END;",
            )
            .unwrap();
        Fixture::file(&track, b"replacement-size");
        let error = catalog.verify_source(source_id).unwrap_err();
        assert!(error.contains("injected write failure"), "{error}");
        assert_eq!(catalog.sources().unwrap()[0].generation, 1);
        assert_eq!(catalog.tracks(source_id).unwrap()[0].size, 3);
        catalog
            .db
            .connection
            .execute_batch("DROP TRIGGER injected_write_failure;")
            .unwrap();
        drop(catalog);
        assert_database_integrity(&fixture.db());

        let mut reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(reopened.verify_source(source_id).unwrap().generation, 2);
        assert_eq!(reopened.tracks(source_id).unwrap()[0].size, 16);
        assert_database_integrity(&fixture.db());
    }

    #[test]
    fn deferred_foreign_key_commit_failure_rolls_back_and_preserves_catalog() {
        let (fixture, _root, _track, source_id) = one_track_catalog();
        let mut connection = Connection::open(fixture.db()).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TABLE b19_parent(id INTEGER PRIMARY KEY);\
                 CREATE TEMP TABLE b19_child(\
                    parent_id INTEGER REFERENCES b19_parent(id) DEFERRABLE INITIALLY DEFERRED\
                 );",
            )
            .unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        transaction
            .execute("INSERT INTO b19_child(parent_id) VALUES(404)", [])
            .unwrap();
        let error = transaction.commit().unwrap_err();
        assert!(error.to_string().contains("FOREIGN KEY constraint failed"));
        drop(connection);

        assert_single_track_state(&fixture.db(), source_id, 1, 3);
        let mut reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(reopened.verify_source(source_id).unwrap().generation, 1);
        assert_single_track_state(&fixture.db(), source_id, 1, 3);
    }

    #[cfg(target_os = "linux")]
    fn create_v1_database(path: &Path, root: &str, relative: &str) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sources(\
                    id INTEGER PRIMARY KEY AUTOINCREMENT,\
                    canonical_path TEXT NOT NULL UNIQUE,\
                    display_name TEXT NOT NULL,\
                    generation INTEGER NOT NULL CHECK(generation >= 1),\
                    last_checked_ns INTEGER NOT NULL);\
                 CREATE TABLE tracks(\
                    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,\
                    relative_path TEXT NOT NULL,\
                    size INTEGER NOT NULL CHECK(size >= 0),\
                    mtime_ns INTEGER NOT NULL,\
                    generation INTEGER NOT NULL CHECK(generation >= 1),\
                    PRIMARY KEY(source_id, relative_path));\
                 CREATE INDEX tracks_source_generation ON tracks(source_id, generation);\
                 PRAGMA user_version=1;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sources(id,canonical_path,display_name,generation,last_checked_ns) \
                 VALUES(17,?1,'legacy-root',4,123456)",
                [root],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation) \
                 VALUES(17,?1,321,654,4)",
                [relative],
            )
            .unwrap();
    }

    fn downgrade_fixture_to_v2(path: &Path) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections; DROP TABLE cue_track_artists; DROP TABLE cue_tracks; DROP TABLE cue_sheets; DROP TABLE source_tag_edit_pending; DROP TABLE track_metadata_extra_artists; DROP TABLE track_metadata; ALTER TABLE tracks DROP COLUMN first_seen_ns; PRAGMA user_version=2;",
            )
            .unwrap();
    }

    fn downgrade_fixture_to_v3(path: &Path) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch("DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections; DROP TABLE cue_track_artists; DROP TABLE cue_tracks; DROP TABLE cue_sheets; DROP TABLE source_tag_edit_pending; DROP TABLE track_metadata_extra_artists; ALTER TABLE tracks DROP COLUMN first_seen_ns; PRAGMA user_version=3;")
            .unwrap();
    }

    #[test]
    fn collections_migrate_v8_and_persist_crud_order_and_unique_members() {
        use crate::collections::NewMember;
        let fixture = Fixture::new();
        drop(Catalog::open(fixture.db()).unwrap());
        let db = Connection::open(fixture.db()).unwrap();
        db.execute_batch("DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections; PRAGMA user_version=8;").unwrap();
        drop(db);
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let first = catalog.collection_create("Favorites").unwrap();
        let second = catalog.collection_create("Other").unwrap();
        catalog.collection_rename(first, "Renamed").unwrap();
        let member = |source: &str, reference: &str| NewMember { source:source.into(), scope:"server-library".into(), reference:reference.into(), title:"Title".into(), artist:Some("Artist".into()) };
        let a = catalog.collection_add(first, member("local", "sqlite:1:abc")).unwrap();
        catalog.collection_add(first, member("plex", "123")).unwrap();
        catalog.collection_add(first, member("subsonic", "xyz")).unwrap();
        assert_eq!(catalog.collection_add(first, member("plex", "123")).unwrap_err(), "COLLECTION_DUPLICATE_MEMBER");
        catalog.collection_add(second, member("plex", "123")).unwrap();
        assert!(catalog.collection_add(first, member("plex", "https://x/?token=secret")).is_err());
        assert!(catalog.collection_members(9999).is_err());
        drop(catalog);
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        assert_eq!(catalog.collections().unwrap()[0].name, "Renamed");
        let members = catalog.collection_members(first).unwrap();
        assert_eq!(catalog.collection_member(first, a).unwrap().id, a);
        assert!(catalog.collection_member(second, a).is_err());
        assert!(catalog.collection_member(first, i64::MAX).is_err());
        assert_eq!(members.iter().map(|m|m.position).collect::<Vec<_>>(), vec![0,1,2]);
        catalog.collection_remove(first, a).unwrap();
        assert_eq!(catalog.collection_members(first).unwrap().len(),2);
        catalog.collection_delete(first).unwrap();
        assert_eq!(catalog.collection_members(second).unwrap().len(),1);
        assert!(catalog.collection_delete(first).is_err());
    }

    #[test]
    fn collections_v9_migration_preserves_members_and_300_file_evidence() {
        use crate::collections::NewMember;
        let fixture = Fixture::new();
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let collection = catalog.collection_create("Migration").unwrap();
        let member = catalog.collection_add(collection, NewMember {
            source: "local".into(), scope: "local1:1".into(), reference: "local1:legacy".into(),
            title: "Álbum".into(), artist: Some("Artist".into()),
        }).unwrap();
        catalog.collection_add(collection, NewMember {
            source: "plex".into(), scope: "server".into(), reference: "album".into(),
            title: "Plex".into(), artist: None,
        }).unwrap();
        drop(catalog);
        let db = Connection::open(fixture.db()).unwrap();
        db.execute_batch("DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; PRAGMA user_version=9;").unwrap();
        drop(db);
        let catalog = Catalog::open(fixture.db()).unwrap();
        assert_eq!(catalog.collection_members(collection).unwrap().len(), 2);
        assert_eq!(catalog.collection_member(collection, member).unwrap().reference, "local1:legacy");
        drop(catalog);
        let mut db = Connection::open(fixture.db()).unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        assert_eq!(schema_version(&db).unwrap(), 10);
        let tx = db.transaction().unwrap();
        tx.execute("INSERT INTO collection_local_snapshots VALUES(?1,2,'sqlite:fixture')", [member]).unwrap();
        for n in 0..300_i64 {
            let path = format!("música/{:04}-{}.wv", n, "long".repeat(30)).into_bytes();
            tx.execute("INSERT INTO collection_local_files VALUES(?1,?2,?3,123,456,?4)", rusqlite::params![member,n,path,vec![7u8;32]]).unwrap();
        }
        let audio = format!("música/{:04}-{}.wv", 0, "long".repeat(30)).into_bytes();
        tx.execute("INSERT INTO collection_cue_segments VALUES(?1,0,?2,?3,1,?4,NULL,20,21,?4,?4,22)", rusqlite::params![member,b"album.cue".as_slice(),audio,0u64.to_be_bytes().to_vec()]).unwrap();
        tx.commit().unwrap();
        assert!(db.execute("INSERT INTO collection_local_snapshots VALUES(999999,2,'bad')", []).is_err());
        drop(db);
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let count: i64 = catalog.db.connection.query_row("SELECT count(*) FROM collection_local_files WHERE member_id=?1", [member], |r| r.get(0)).unwrap();
        assert_eq!(count, 300);
        for n in 0..300_i64 {
            let (path, size, mtime, digest): (Vec<u8>, i64, i64, Vec<u8>) = catalog.db.connection.query_row(
                "SELECT path,size,mtime_ns,sample_sha256 FROM collection_local_files WHERE member_id=?1 AND position=?2",
                rusqlite::params![member,n], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
            assert_eq!(path, format!("música/{:04}-{}.wv", n, "long".repeat(30)).into_bytes());
            assert_eq!((size, mtime, digest), (123, 456, vec![7u8;32]));
        }
        let segments: i64 = catalog.db.connection.query_row("SELECT count(*) FROM collection_cue_segments WHERE member_id=?1", [member], |r| r.get(0)).unwrap();
        assert_eq!(segments, 1);
        catalog.collection_delete(collection).unwrap();
        for table in ["collection_local_snapshots", "collection_local_files", "collection_cue_segments"] {
            let remaining: i64 = catalog.db.connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
            assert_eq!(remaining, 0);
        }
        drop(catalog);
        assert_eq!(schema_version(&Connection::open(fixture.db()).unwrap()).unwrap(), 10);
    }

    #[test]
    fn new_database_is_initialized_at_current_schema_version() {
        let f = Fixture::new();
        let catalog = Catalog::open(f.db()).unwrap();
        drop(catalog);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v8(&connection).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn v6_migration_creates_cue_schema_atomically_and_preserves_catalog_rows() {
        let f = Fixture::new();
        let root = f.dir("v6-cue-migration");
        make_audio(&root, 2);
        let source_id = Catalog::open(f.db()).unwrap().add_source(&root, "preserve").unwrap().source_id;
        let before_tracks = Catalog::open(f.db()).unwrap().tracks(source_id).unwrap();
        {
            let connection = Connection::open(f.db()).unwrap();
            connection.execute_batch("DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections; DROP TABLE cue_track_artists; DROP TABLE cue_tracks; DROP TABLE cue_sheets; PRAGMA user_version=6;").unwrap();
        }
        let before_schema = schema_signature(&Connection::open(f.db()).unwrap());
        let mut connection = Connection::open(f.db()).unwrap();
        assert!(migrate_v6_to_v7_failing(&mut connection).unwrap_err().contains("injected v6-to-v7"));
        assert_eq!(schema_version(&connection).unwrap(), SCHEMA_V6_VERSION);
        assert_eq!(schema_signature(&connection), before_schema);
        drop(connection);

        let migrated = Catalog::open(f.db()).unwrap();
        assert_eq!(migrated.tracks(source_id).unwrap(), before_tracks);
        assert!(migrated.cue_sheets_for_source(source_id).unwrap().is_empty());
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v8(&connection).unwrap();
    }

    #[test]
    fn v5_migration_is_atomic_and_preserves_historical_dates_as_unknown() {
        let f = Fixture::new();
        let root = f.dir("v5-migration-root");
        make_audio(&root, 2);
        let source_id = Catalog::open(f.db()).unwrap().add_source(&root, "legacy").unwrap().source_id;
        let before_tracks = Catalog::open(f.db()).unwrap().tracks(source_id).unwrap();
        assert!(before_tracks.iter().all(|track| track.first_seen_ns.is_some()));

        let connection = Connection::open(f.db()).unwrap();
        connection.execute_batch("DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections; DROP TABLE cue_track_artists; DROP TABLE cue_tracks; DROP TABLE cue_sheets; ALTER TABLE tracks DROP COLUMN first_seen_ns; PRAGMA user_version=5;").unwrap();
        drop(connection);
        let before_schema = schema_signature(&Connection::open(f.db()).unwrap());
        let mut connection = Connection::open(f.db()).unwrap();
        assert!(migrate_v5_to_v6_failing(&mut connection).unwrap_err().contains("injected v5-to-v6"));
        drop(connection);
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), SCHEMA_V5_VERSION);
        assert_eq!(schema_signature(&connection), before_schema);
        drop(connection);

        let migrated = Catalog::open(f.db()).unwrap();
        assert_eq!(migrated.tracks(source_id).unwrap().len(), before_tracks.len());
        assert!(migrated.tracks(source_id).unwrap().iter().all(|track| track.first_seen_ns.is_none()));
        let view = migrated.source_library_view(source_id).unwrap();
        assert!(view.albums().iter().all(|album| album.first_seen_ns.is_none()));
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v6(&connection).unwrap();
    }

    #[test]
    fn first_seen_survives_reverification_and_only_new_paths_receive_dates() {
        let f = Fixture::new();
        let root = f.dir("first-seen");
        let album = root.join("Album");
        fs::create_dir_all(&album).unwrap();
        fs::write(album.join("01.flac"), b"synthetic audio fixture").unwrap();
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let old_path = PathBuf::from("Album/01.flac");
        let first = catalog.tracks(source_id).unwrap()[0].first_seen_ns.unwrap();
        assert!(catalog.source_library_view(source_id).unwrap().albums()[0].first_seen_ns.is_some());

        fs::write(album.join("02.flac"), b"another synthetic audio fixture").unwrap();
        catalog.verify_source(source_id).unwrap();
        fs::write(album.join("01.flac"), b"modified synthetic audio fixture with changed size").unwrap();
        catalog.verify_source(source_id).unwrap();
        let tracks = catalog.tracks(source_id).unwrap();
        assert_eq!(tracks.iter().find(|track| track.relative_path == old_path).unwrap().first_seen_ns, Some(first));
        assert!(tracks.iter().find(|track| track.relative_path == Path::new("Album/02.flac")).unwrap().first_seen_ns.is_some());
    }

    #[test]
    fn reopening_current_database_preserves_sources_and_tracks() {
        let f = Fixture::new();
        let root = f.dir("A");
        make_audio(&root, 3);
        let source_id;
        let tracks_before;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
            tracks_before = catalog.tracks(source_id).unwrap();
        }

        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(reopened.sources().unwrap().len(), 1);
        assert_eq!(reopened.tracks(source_id).unwrap(), tracks_before);
    }

    #[test]
    fn repeated_open_does_not_change_current_schema() {
        let f = Fixture::new();
        drop(Catalog::open(f.db()).unwrap());
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        drop(Catalog::open(f.db()).unwrap());
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        assert_eq!(schema_signature(&connection), before);
    }

    #[test]
    fn future_schema_is_rejected_without_changing_its_data_or_schema() {
        let f = Fixture::new();
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE future_data(value TEXT NOT NULL);\
                     INSERT INTO future_data(value) VALUES('preserve');\
                     PRAGMA user_version=42;",
                )
                .unwrap();
        }
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let error = Catalog::open(f.db()).err().unwrap();
        assert!(error.contains("future catalog schema version 42"));

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 42);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT value FROM future_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserve"
        );
    }

    #[test]
    fn incompatible_current_schema_is_rejected_without_modification() {
        let f = Fixture::new();
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE sentinel(value TEXT NOT NULL);\
                     INSERT INTO sentinel(value) VALUES('preserve');\
                     PRAGMA user_version=2;",
                )
                .unwrap();
        }
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let error = Catalog::open(f.db()).err().unwrap();
        assert!(
            error.contains("version 2 has incompatible objects"),
            "unexpected schema error: {error}"
        );

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 2);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT value FROM sentinel", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserve"
        );
    }

    #[test]
    fn failed_versioned_schema_change_rolls_back_ddl_and_version() {
        let f = Fixture::new();
        let root = f.dir("A");
        make_audio(&root, 2);
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
        }
        let connection = Connection::open(f.db()).unwrap();
        let before_schema = schema_signature(&connection);
        let before_tracks = connection
            .query_row(
                "SELECT count(*) FROM tracks WHERE source_id=?1",
                [source_id],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        drop(connection);

        let mut connection = Connection::open(f.db()).unwrap();
        let result = apply_schema_change(&mut connection, 3, |transaction| {
            transaction
                .execute_batch("CREATE TABLE migration_probe(value INTEGER);")
                .map_err(|error| error.to_string())?;
            Err("simulated migration failure".into())
        });
        assert!(result.is_err());
        drop(connection);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        assert_eq!(schema_signature(&connection), before_schema);
        let migration_probe_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='migration_probe'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migration_probe_count, 0);
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM tracks WHERE source_id=?1",
                    [source_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            before_tracks
        );
    }

    #[test]
    fn unversioned_database_with_tables_is_classified_as_legacy_and_preserved() {
        let f = Fixture::new();
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE legacy_data(value TEXT NOT NULL);\
                     INSERT INTO legacy_data(value) VALUES('preserve');",
                )
                .unwrap();
        }
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let error = Catalog::open(f.db()).err().unwrap();
        assert!(error.contains("unversioned legacy catalog"));
        assert!(error.contains("explicit migration is required"));

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 0);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT value FROM legacy_data", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "preserve"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn valid_v1_migrates_atomically_preserving_ids_records_and_generation_without_scanning() {
        let f = Fixture::new();
        let root = f.path("root-é-音楽-does-not-exist");
        create_v1_database(&f.db(), root.to_str().unwrap(), "Álbum/曲.flac");
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute(
                    "INSERT INTO sources(id,canonical_path,display_name,generation,last_checked_ns) \
                     VALUES(90,'/deleted-source','deleted',1,0)",
                    [],
                )
                .unwrap();
            connection
                .execute("DELETE FROM sources WHERE id=90", [])
                .unwrap();
        }

        let catalog = Catalog::open(f.db()).unwrap();
        assert_eq!(catalog.track_count().unwrap(), 1);
        assert_eq!(
            catalog.sources().unwrap(),
            [Source {
                id: 17,
                canonical_path: root.clone(),
                display_name: "legacy-root".into(),
                generation: 4,
                last_checked_ns: 123456,
            }]
        );
        assert_eq!(
            catalog.tracks(17).unwrap(),
            [Track {
                source_id: 17,
                relative_path: PathBuf::from("Álbum/曲.flac"),
                size: 321,
                mtime_ns: 654,
                generation: 4,
                first_seen_ns: None,
            }]
        );
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v6(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT seq FROM sqlite_sequence WHERE name='sources'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            90
        );
    }

    #[test]
    fn valid_v2_migrates_sequentially_to_v6_without_scanning_or_changing_records() {
        let f = Fixture::new();
        let root = f.dir("A");
        make_audio(&root, 2);
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
        }
        downgrade_fixture_to_v2(&f.db());

        let before = schema_signature(&Connection::open(f.db()).unwrap());
        assert_eq!(
            schema_version(&Connection::open(f.db()).unwrap()).unwrap(),
            2
        );
        validate_schema_v2(&Connection::open(f.db()).unwrap()).unwrap();
        let migrated = Catalog::open(f.db()).unwrap();
        assert_eq!(migrated.track_count_for(source_id).unwrap(), 2);
        assert_eq!(migrated.tracks(source_id).unwrap().len(), 2);
        drop(migrated);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v6(&connection).unwrap();
        assert_ne!(schema_signature(&connection), before);
        assert_database_integrity(&f.db());
    }

    #[test]
    fn failed_v2_to_v3_migration_preserves_version_schema_and_records() {
        let f = Fixture::new();
        let root = f.dir("A");
        make_audio(&root, 1);
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
        }
        downgrade_fixture_to_v2(&f.db());
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let mut connection = Connection::open(f.db()).unwrap();
        assert!(migrate_v2_to_v3_failing(&mut connection)
            .unwrap_err()
            .contains("injected v2-to-v3 migration failure"));
        drop(connection);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 2);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM tracks WHERE source_id=?1",
                    [source_id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_database_integrity(&f.db());
        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(reopened.track_count_for(source_id).unwrap(), 1);
        assert_eq!(
            schema_version(&Connection::open(f.db()).unwrap()).unwrap(),
            CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn valid_v3_migrates_to_v6_without_scanning_or_losing_metadata() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("Legacy title")));
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
        }
        Connection::open(f.db())
            .unwrap()
            .execute(
                "UPDATE track_metadata SET attempt_parser_version=1 WHERE source_id=?1",
                [source_id],
            )
            .unwrap();
        downgrade_fixture_to_v3(&f.db());
        let before = schema_signature(&Connection::open(f.db()).unwrap());
        validate_schema_v3(&Connection::open(f.db()).unwrap()).unwrap();

        let migrated = Catalog::open(f.db()).unwrap();
        assert_eq!(migrated.track_count_for(source_id).unwrap(), 1);
        let metadata = migrated
            .metadata_for_track(source_id, "one.wav")
            .unwrap()
            .unwrap();
        assert_eq!(
            metadata.last_success.unwrap().title.as_deref(),
            Some("Legacy title")
        );
        assert_eq!(metadata.parser_version, 1);
        drop(migrated);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v6(&connection).unwrap();
        assert_ne!(schema_signature(&connection), before);
        assert_database_integrity(&f.db());
    }

    #[test]
    fn valid_v4_migrates_to_v6_without_scanning_or_changing_metadata() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("Keep metadata")));
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
        }
        Connection::open(f.db())
            .unwrap()
            .execute_batch("DROP TABLE collection_cue_segments; DROP TABLE collection_local_files; DROP TABLE collection_local_snapshots; DROP TABLE personal_collection_members; DROP TABLE personal_collections; DROP TABLE cue_track_artists; DROP TABLE cue_tracks; DROP TABLE cue_sheets; DROP TABLE source_tag_edit_pending; ALTER TABLE tracks DROP COLUMN first_seen_ns; PRAGMA user_version=4;")
            .unwrap();
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let migrated = Catalog::open(f.db()).unwrap();
        assert_eq!(migrated.track_count_for(source_id).unwrap(), 1);
        let metadata = migrated
            .metadata_for_track(source_id, "one.wav")
            .unwrap()
            .unwrap();
        assert_eq!(metadata.last_success.unwrap().title.as_deref(), Some("Keep metadata"));
        drop(migrated);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        validate_schema_v6(&connection).unwrap();
        assert_ne!(schema_signature(&connection), before);
        assert_database_integrity(&f.db());
    }

    #[test]
    fn failed_v3_to_v4_migration_preserves_schema_and_metadata() {
        let f = Fixture::new();
        let root = f.dir("A");
        Fixture::file(&root.join("one.wav"), &pcm_wav(Some("Keep me")));
        let source_id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            source_id = catalog.add_source(&root, "A").unwrap().source_id;
        }
        downgrade_fixture_to_v3(&f.db());
        let before = schema_signature(&Connection::open(f.db()).unwrap());
        let mut connection = Connection::open(f.db()).unwrap();
        assert!(migrate_v3_to_v4_failing(&mut connection)
            .unwrap_err()
            .contains("injected v3-to-v4 migration failure"));
        drop(connection);

        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 3);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT title FROM track_metadata WHERE source_id=?1",
                    [source_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Keep me"
        );
        assert_database_integrity(&f.db());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn incompatible_v1_schema_is_rejected_without_modification() {
        let f = Fixture::new();
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE sources(id INTEGER PRIMARY KEY, canonical_path TEXT);\
                     INSERT INTO sources VALUES(1,'/preserve');\
                     PRAGMA user_version=1;",
                )
                .unwrap();
        }
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let error = Catalog::open(f.db()).err().unwrap();
        assert!(error.contains("version 1 has incompatible objects"));
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), 1);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT canonical_path FROM sources", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "/preserve"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ambiguous_v1_replacement_character_paths_are_rejected_without_mutation() {
        for ambiguous_root in [true, false] {
            let f = Fixture::new();
            let root = if ambiguous_root {
                format!("{}/root-\u{fffd}", f.0.display())
            } else {
                format!("{}/root", f.0.display())
            };
            let relative = if ambiguous_root {
                "Album/track.flac"
            } else {
                "Album/track-\u{fffd}.flac"
            };
            create_v1_database(&f.db(), &root, relative);
            let before_schema = schema_signature(&Connection::open(f.db()).unwrap());

            let error = Catalog::open(f.db()).err().unwrap();
            assert!(error.contains("U+FFFD"), "unexpected error: {error}");

            let connection = Connection::open(f.db()).unwrap();
            assert_eq!(schema_version(&connection).unwrap(), 1);
            assert_eq!(schema_signature(&connection), before_schema);
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM sources", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                connection
                    .query_row("SELECT count(*) FROM tracks", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failure_after_v1_table_replacement_rolls_back_original_schema_and_version() {
        let f = Fixture::new();
        let root = f.path("root");
        create_v1_database(&f.db(), root.to_str().unwrap(), "one.flac");
        let mut connection = Connection::open(f.db()).unwrap();
        let before = schema_signature(&connection);

        let result = migrate_v1_to_v2_with_hook(&mut connection, |_| {
            Err("injected failure after replacement".into())
        });
        assert!(result
            .unwrap_err()
            .contains("injected failure after replacement"));
        assert_eq!(schema_version(&connection).unwrap(), 1);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT id FROM sources", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            17
        );
        assert_eq!(
            connection
                .query_row("SELECT relative_path FROM tracks", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "one.flac"
        );
    }

    #[test]
    fn unknown_path_format_is_rejected_without_modifying_current_database() {
        let f = Fixture::new();
        drop(Catalog::open(f.db()).unwrap());
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute_batch("PRAGMA ignore_check_constraints=ON;")
                .unwrap();
            connection
                .execute(
                    "INSERT INTO sources(path_format,canonical_path,display_name,generation,last_checked_ns) \
                     VALUES(99,?1,'unknown',1,0)",
                    [b"/tmp/unknown".as_slice()],
                )
                .unwrap();
        }
        let before = schema_signature(&Connection::open(f.db()).unwrap());

        let error = Catalog::open(f.db()).err().unwrap();
        assert!(error.contains("unknown path format 99"));
        let connection = Connection::open(f.db()).unwrap();
        assert_eq!(schema_version(&connection).unwrap(), CURRENT_SCHEMA_VERSION);
        assert_eq!(schema_signature(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT path_format FROM sources", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            99
        );
    }

    #[test]
    fn unsafe_persisted_relative_path_is_rejected_when_tracks_are_read() {
        let f = Fixture::new();
        drop(Catalog::open(f.db()).unwrap());
        let root = f.dir("root");
        {
            let connection = Connection::open(f.db()).unwrap();
            connection
                .execute(
                    "INSERT INTO sources(path_format,canonical_path,display_name,generation,last_checked_ns) \
                     VALUES(1,?1,'root',1,0)",
                    [path_codec::encode_root(&root).unwrap()],
                )
                .unwrap();
            let source_id = connection.last_insert_rowid();
            connection
                .execute(
                    "INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation) \
                     VALUES(?1,?2,1,0,1)",
                    rusqlite::params![source_id, b"../escape.flac".as_slice()],
                )
                .unwrap();
        }
        let catalog = Catalog::open(f.db()).unwrap();
        assert!(catalog.tracks(1).unwrap_err().contains("unsafe components"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_invalid_utf8_names_remain_distinct_across_scan_and_reverification() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let f = Fixture::new();
        let root =
            f.0.join(std::ffi::OsString::from_vec(b"root-\xfd".to_vec()));
        fs::create_dir_all(&root).unwrap();
        let names = [
            b"bad-\xff.flac".as_slice(),
            b"bad-\xfe.flac",
            "bad-\u{fffd}.flac".as_bytes(),
        ];
        for name in names {
            Fixture::file(
                &root.join(std::ffi::OsString::from_vec(name.to_vec())),
                b"x",
            );
        }

        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "root").unwrap().source_id;
        let canonical_root = fs::canonicalize(&root).unwrap();
        assert_eq!(catalog.sources().unwrap()[0].canonical_path, canonical_root);
        let tracks = catalog.tracks(source_id).unwrap();
        assert_eq!(tracks.len(), 3);
        let mut stored_names = tracks
            .iter()
            .map(|track| track.relative_path.as_os_str().as_bytes().to_vec())
            .collect::<Vec<_>>();
        stored_names.sort();
        let mut expected = names.iter().map(|name| name.to_vec()).collect::<Vec<_>>();
        expected.sort();
        assert_eq!(stored_names, expected);

        let report = catalog.verify_source(source_id).unwrap();
        assert_eq!(report.metrics.files_unchanged, 3);
        assert_eq!(catalog.tracks(source_id).unwrap().len(), 3);
        drop(catalog);
        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(
            reopened.sources().unwrap()[0].canonical_path,
            canonical_root
        );
        assert_eq!(reopened.tracks(source_id).unwrap().len(), 3);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rename_is_reconciled_as_removal_plus_addition_without_identity_aliasing() {
        let f = Fixture::new();
        let root = f.dir("root");
        Fixture::file(&root.join("before.flac"), b"same");
        let mut catalog = Catalog::open(f.db()).unwrap();
        let source_id = catalog.add_source(&root, "root").unwrap().source_id;

        fs::rename(root.join("before.flac"), root.join("after.flac")).unwrap();
        let report = catalog.verify_source(source_id).unwrap();

        assert_eq!(report.metrics.files_new, 1);
        assert_eq!(report.metrics.files_removed, 1);
        let tracks = catalog.tracks(source_id).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].relative_path, Path::new("after.flac"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn scanner_refuses_platforms_without_a_secure_scanner() {
        let f = Fixture::new();
        let root = f.dir("root");
        let mut catalog = Catalog::open(f.db()).unwrap();
        assert!(catalog
            .add_source(&root, "root")
            .unwrap_err()
            .contains("only on Linux"));
        assert!(catalog.sources().unwrap().is_empty());
    }
    #[test]
    fn b1_1_reopen_persists_without_scanning() {
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 7);
        let id;
        {
            let mut c = Catalog::open(f.db()).unwrap();
            c.add_source(&a, "A").unwrap();
            id = c.sources().unwrap()[0].id;
        }
        let c = Catalog::open(f.db()).unwrap();
        assert_eq!(c.track_count().unwrap(), 7);
        assert_eq!(c.tracks(id).unwrap().len(), 7);
    }
    #[test]
    fn database_open_failure_is_reported_without_panicking() {
        let f = Fixture::new();
        let database_path_is_directory = f.dir("not-a-database.sqlite3");

        let error = match Catalog::open(&database_path_is_directory) {
            Ok(_) => panic!("opening a directory as a SQLite database must fail"),
            Err(error) => error,
        };
        assert!(
            error.starts_with("sqlite open:"),
            "unexpected error: {error}"
        );
    }
    #[test]
    fn explicit_connection_close_allows_clean_reopen() {
        let f = Fixture::new();
        let root = f.dir("A");
        make_audio(&root, 2);
        let id;
        {
            let mut catalog = Catalog::open(f.db()).unwrap();
            id = catalog.add_source(&root, "A").unwrap().source_id;
            catalog.close().unwrap();
        }

        let reopened = Catalog::open(f.db()).unwrap();
        assert_eq!(reopened.tracks(id).unwrap().len(), 2);
        assert_eq!(reopened.track_count().unwrap(), 2);
    }
    #[test]
    fn b1_2_addition_scans_only_new_source_5000_plus_100() {
        let f = Fixture::new();
        let a = f.dir("A");
        let b = f.dir("B");
        make_audio(&a, 5000);
        make_audio(&b, 100);
        let mut c = Catalog::open(f.db()).unwrap();
        let ar = c.add_source(&a, "A").unwrap();
        let br = c.add_source(&b, "B").unwrap();
        assert_eq!(c.track_count().unwrap(), 5100);
        assert_eq!(ar.scan.metrics.files_new, 5000);
        assert_eq!(br.scan.metrics.sources_examined, 1);
        assert_eq!(br.scan.metrics.files_examined, 100);
        assert_eq!(br.scan.metrics.records_written, 101);
        assert_eq!(
            br.scan.metrics.files_examined + ar.scan.metrics.files_examined,
            5100
        );
        eprintln!("B1.2 A={:?}; B={:?}", ar.scan.metrics, br.scan.metrics);
    }
    #[test]
    fn b1_3_unchanged_verification_does_not_reprocess_or_write_track_rows() {
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 5);
        let mut c = Catalog::open(f.db()).unwrap();
        let added = c.add_source(&a, "A").unwrap();
        let id = added.source_id;
        let before = c.tracks(id).unwrap();
        let r = c.verify_source(id).unwrap();
        assert_eq!(r.metrics.files_examined, 5);
        assert_eq!(r.metrics.files_unchanged, 5);
        assert_eq!(
            r.metrics.files_new + r.metrics.files_modified + r.metrics.files_removed,
            0
        );
        assert_eq!(r.metrics.records_written, 1);
        assert_eq!(c.track_count().unwrap(), 5);
        assert_eq!(c.tracks(id).unwrap(), before);
    }
    #[test]
    fn b1_4_single_file_change_updates_only_that_record() {
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 4);
        let mut c = Catalog::open(f.db()).unwrap();
        c.add_source(&a, "A").unwrap();
        Fixture::file(&a.join("track-00002.flac"), b"changed-size");
        let r = c.verify_source(1).unwrap();
        assert_eq!(r.metrics.files_modified, 1);
        assert_eq!(r.metrics.records_written, 2);
        assert_eq!(c.track_count().unwrap(), 4);
    }
    #[test]
    fn b1_5_remove_source_keeps_other_source_and_files() {
        let f = Fixture::new();
        let a = f.dir("A");
        let b = f.dir("B");
        make_audio(&a, 2);
        make_audio(&b, 3);
        let mut c = Catalog::open(f.db()).unwrap();
        let ia = c.add_source(&a, "A").unwrap();
        let ib = c.add_source(&b, "B").unwrap();
        c.remove_source(ib.source_id).unwrap();
        let aid = ia.source_id;
        assert_eq!(c.track_count_for(aid).unwrap(), 2);
        assert!(a.join("track-00000.flac").exists());
        assert!(b.join("track-00000.flac").exists());
    }
    #[test]
    fn b1_6_unavailable_source_preserves_rows() {
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 3);
        let mut c = Catalog::open(f.db()).unwrap();
        c.add_source(&a, "A").unwrap();
        fs::rename(&a, f.path("A-away")).unwrap();
        assert!(c.verify_source(1).is_err());
        assert_eq!(c.track_count().unwrap(), 3);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn directory_disappearing_mid_scan_preserves_committed_catalog_rows() {
        let f = Fixture::new();
        let root = f.dir("A");
        let disappearing = f.dir("A/child");
        make_audio(&disappearing, 2);
        Fixture::file(&root.join("kept.flac"), b"committed");
        let mut catalog = Catalog::open(f.db()).unwrap();
        let id = catalog.add_source(&root, "A").unwrap().source_id;
        let before = catalog.tracks(id).unwrap();

        let result = catalog.verify_with_walk_hook(id, |relative| {
            if relative == Path::new("child") && disappearing.exists() {
                fs::remove_dir_all(&disappearing).unwrap();
            }
        });

        assert!(result.is_err());
        assert_eq!(catalog.tracks(id).unwrap(), before);
        assert_eq!(catalog.sources().unwrap()[0].generation, 1);
    }
    #[test]
    fn b1_7_interruption_rolls_back_update_and_delete() {
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 3);
        let mut c = Catalog::open(f.db()).unwrap();
        c.add_source(&a, "A").unwrap();
        Fixture::file(&a.join("track-00000.flac"), b"changed-size");
        fs::remove_file(a.join("track-00001.flac")).unwrap();
        assert!(c.verify_with_interruption(1).is_err());
        drop(c);
        let c = Catalog::open(f.db()).unwrap();
        let tracks = c.tracks(1).unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(
            tracks
                .iter()
                .find(|t| t.relative_path == Path::new("track-00000.flac"))
                .unwrap()
                .size,
            9
        );
        assert_eq!(
            tracks
                .iter()
                .find(|t| t.relative_path == Path::new("track-00001.flac"))
                .unwrap()
                .size,
            9
        );
    }
    #[test]
    fn b1_8_repeated_verification_is_idempotent() {
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 4);
        let mut c = Catalog::open(f.db()).unwrap();
        c.add_source(&a, "A").unwrap();
        let before = c.tracks(1).unwrap();
        for _ in 0..3 {
            let r = c.verify_source(1).unwrap();
            assert_eq!(r.metrics.records_written, 1); // source verification timestamp only
            assert_eq!(r.metrics.files_unchanged, 4);
        }
        assert_eq!(before, c.tracks(1).unwrap());
    }
    #[test]
    fn stale_verification_finishing_after_newer_generation_is_discarded_and_reopens_cleanly() {
        use std::sync::mpsc;
        use std::{thread, time::Duration};
        let f = Fixture::new();
        let a = f.dir("A");
        Fixture::file(&a.join("one.flac"), b"old");
        let db_path = f.db();
        let mut initial = Catalog::open(&db_path).unwrap();
        let id = initial.add_source(&a, "A").unwrap().source_id;
        drop(initial);

        let (scanned_tx, scanned_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let stale_db = db_path.clone();
        let stale = thread::spawn(move || {
            let mut c = Catalog::open(stale_db).unwrap();
            c.verify_with_hook(id, || {
                scanned_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            })
        });
        scanned_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("stale scan did not reach pre-commit point"); // no write lock is held
        Fixture::file(&a.join("one.flac"), b"newer-content");
        let mut fresh = Catalog::open(&db_path).unwrap();
        let fresh_report = fresh.verify_source(id).unwrap();
        assert_eq!(fresh_report.generation, 2);
        resume_tx.send(()).unwrap();
        let stale_result = stale.join().unwrap();
        assert!(stale_result.unwrap_err().contains("generation advanced"));
        drop(fresh);

        let reopened = Catalog::open(&db_path).unwrap();
        assert_eq!(reopened.sources().unwrap()[0].generation, 2);
        assert_eq!(reopened.tracks(id).unwrap()[0].size, 13);
        assert_eq!(reopened.track_count().unwrap(), 1);
    }

    #[test]
    fn removal_during_verification_discards_scan_without_recreating_source() {
        use std::sync::mpsc;
        use std::{thread, time::Duration};
        let f = Fixture::new();
        let a = f.dir("A");
        make_audio(&a, 2);
        let db_path = f.db();
        let mut initial = Catalog::open(&db_path).unwrap();
        let id = initial.add_source(&a, "A").unwrap().source_id;
        drop(initial);

        let (scanned_tx, scanned_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let scan_db = db_path.clone();
        let scan = thread::spawn(move || {
            let mut c = Catalog::open(scan_db).unwrap();
            c.verify_with_hook(id, || {
                scanned_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            })
        });
        scanned_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("scan did not reach pre-commit point");
        let mut remover = Catalog::open(&db_path).unwrap();
        assert_eq!(remover.remove_source(id).unwrap(), 2);
        resume_tx.send(()).unwrap();
        assert!(scan
            .join()
            .unwrap()
            .unwrap_err()
            .contains("source was removed"));
        drop(remover);
        let reopened = Catalog::open(&db_path).unwrap();
        assert!(reopened.sources().unwrap().is_empty());
        assert_eq!(reopened.track_count().unwrap(), 0);
    }

    #[test]
    fn simultaneous_verifications_of_distinct_sources_both_commit() {
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;
        let f = Fixture::new();
        let a = f.dir("A");
        let b = f.dir("B");
        make_audio(&a, 1);
        make_audio(&b, 1);
        let db_path = f.db();
        let mut initial = Catalog::open(&db_path).unwrap();
        let a_id = initial.add_source(&a, "A").unwrap().source_id;
        let b_id = initial.add_source(&b, "B").unwrap().source_id;
        drop(initial);

        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_a_tx, release_a_rx) = mpsc::channel();
        let (release_b_tx, release_b_rx) = mpsc::channel();
        let start = |id, ready: mpsc::Sender<()>, release: mpsc::Receiver<()>, db_path: PathBuf| {
            thread::spawn(move || {
                let mut c = Catalog::open(db_path).unwrap();
                c.verify_with_hook(id, || {
                    ready.send(()).unwrap();
                    release.recv().unwrap();
                })
            })
        };
        let ta = start(a_id, ready_tx.clone(), release_a_rx, db_path.clone());
        let tb = start(b_id, ready_tx, release_b_rx, db_path.clone());
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("source A scan did not reach pre-commit point");
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("source B scan did not reach pre-commit point");
        // Both scans are paused outside a write transaction before either is released.
        release_a_tx.send(()).unwrap();
        release_b_tx.send(()).unwrap();
        assert!(ta.join().unwrap().is_ok());
        assert!(tb.join().unwrap().is_ok());
        let reopened = Catalog::open(&db_path).unwrap();
        assert_eq!(reopened.sources().unwrap().len(), 2);
        assert_eq!(reopened.track_count().unwrap(), 2);
    }

    #[test]
    fn unicode_paths_round_trip() {
        let f = Fixture::new();
        let a = f.dir("音楽-é");
        Fixture::file(&a.join("曲.flac"), b"x");
        let mut c = Catalog::open(f.db()).unwrap();
        let r = c.add_source(&a, "音楽").unwrap();
        assert_eq!(c.tracks(r.source_id).unwrap().len(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let a = f.dir("A");
        let outside = f.dir("outside");
        Fixture::file(&outside.join("escape.flac"), b"x");
        symlink(&outside, a.join("linked-dir")).unwrap();
        symlink(outside.join("escape.flac"), a.join("linked-file.flac")).unwrap();
        let mut c = Catalog::open(f.db()).unwrap();
        let r = c.add_source(&a, "A").unwrap();
        assert!(c.tracks(r.source_id).unwrap().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn canonical_symlink_alias_is_rejected_as_same_source_root() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let a = f.dir("A");
        let alias = f.path("A-alias");
        symlink(&a, &alias).unwrap();
        let mut c = Catalog::open(f.db()).unwrap();
        c.add_source(&a, "A").unwrap();
        assert!(c.add_source(&alias, "alias").is_err());
        assert_eq!(c.sources().unwrap().len(), 1);
    }
    #[test]
    fn overlapping_roots_rejected() {
        let f = Fixture::new();
        let p = f.dir("parent");
        let child = p.join("child");
        fs::create_dir_all(&child).unwrap();
        let mut c = Catalog::open(f.db()).unwrap();
        c.add_source(&p, "parent").unwrap();
        assert!(c.add_source(&child, "child").is_err());
        assert_eq!(c.sources().unwrap().len(), 1);
    }

    #[test]
    fn browsing_groups_multidisc_album_and_orders_missing_numbers_last() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (path, disc, track, title) in [
            ("Artist/Album/CD 1/01.flac", Some("1"), Some("1"), "First"),
            ("Artist/Album/CD 2/01.flac", Some("2"), Some("1"), "Second"),
            ("Artist/Album/CD 2/02.flac", Some("2"), Some("2"), "Third"),
            ("Artist/Album/CD 3/unknown.flac", None, None, "Unnumbered"),
        ] {
            let mut tags = vec![
                ("TITLE", title),
                ("ALBUM", "A Long Album"),
                ("ALBUMARTIST", "The Group"),
                ("DATE", "2001"),
            ];
            if let Some(disc) = disc {
                tags.push(("DISCNUMBER", disc));
            }
            if let Some(track) = track {
                tags.push(("TRACKNUMBER", track));
            }
            Fixture::file(&root.join(path), &metadata::synthetic_flac(&tags));
        }

        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(
            albums.len(),
            1,
            "disc subfolders should resolve to one album context"
        );
        assert_eq!(albums[0].track_count, 4);
        assert_eq!(albums[0].album_artists, ["The Group"]);
        assert_eq!(albums[0].directory_hint, Path::new("Artist/Album"));
        assert!(albums[0]
            .grouping_notes
            .contains(&"validated_sibling_disc_folders_consolidated".to_owned()));

        let tracks = catalog.tracks_for_album(&albums[0].id).unwrap();
        let ordered = tracks
            .iter()
            .map(|track| track.track.relative_path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            [
                "Artist/Album/CD 1/01.flac",
                "Artist/Album/CD 2/01.flac",
                "Artist/Album/CD 2/02.flac",
                "Artist/Album/CD 3/unknown.flac",
            ]
        );
    }

    #[test]
    fn parenthesized_multidisc_tags_merge_only_when_folder_and_suffix_agree() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (path, album) in [
            ("release/CD1/01.flac", "Saudações (CD1)"),
            ("release/CD2/01.flac", "Saudações (CD2)"),
        ] {
            Fixture::file(
                &root.join(path),
                &metadata::synthetic_flac(&[
                    ("TITLE", "Track"),
                    ("ALBUM", album),
                    ("ALBUMARTIST", "Egberto Gismonti"),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Saudações");
        assert_eq!(albums[0].directory_hint, Path::new("release"));
        assert_eq!(albums[0].track_count, 2);
    }

    #[test]
    fn dominant_title_variant_is_consolidated_only_at_the_historical_threshold() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for index in 0..6 {
            let album = if index < 5 {
                "Acústico Lobão"
            } else {
                "Acústico MTV"
            };
            Fixture::file(
                &root.join(format!("Lobão/Acústico/{index:02}.flac")),
                &metadata::synthetic_flac(&[
                    ("TITLE", "Track"),
                    ("ALBUM", album),
                    ("ARTIST", "Lobão"),
                ]),
            );
        }
        // A separate folder with a 3/2 split stays separate: neither value
        // reaches the old 80% dominance threshold.
        for (index, album) in [
            (0, "First Title"),
            (1, "First Title"),
            (2, "First Title"),
            (3, "Second Title"),
            (4, "Second Title"),
        ] {
            Fixture::file(
                &root.join(format!("Ambiguous/{index:02}.flac")),
                &metadata::synthetic_flac(&[("TITLE", "Track"), ("ALBUM", album)]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        let lobao = albums
            .iter()
            .find(|album| album.directory_hint == Path::new("Lobão/Acústico"))
            .unwrap();
        assert_eq!(lobao.title, "Acústico Lobão");
        assert_eq!(lobao.track_count, 6);
        assert!(lobao
            .grouping_notes
            .contains(&"dominant_album_title_variant_selected".to_owned()));
        assert_eq!(
            albums
                .iter()
                .filter(|album| album.directory_hint == Path::new("Ambiguous"))
                .count(),
            2
        );
    }

    #[test]
    fn inconsistent_album_artist_does_not_split_one_physical_release() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (file, album_artist) in [
            ("one.flac", "The Group"),
            ("two.flac", "The Group"),
            ("three.flac", "Guest Project"),
        ] {
            Fixture::file(
                &root.join(file),
                &metadata::synthetic_flac(&[
                    ("TITLE", file),
                    ("ALBUM", "Shared Release"),
                    ("ALBUMARTIST", album_artist),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 3);
        assert!(albums[0].ambiguous);
        assert_eq!(albums[0].album_artists, ["The Group", "Guest Project"]);
        assert!(albums[0]
            .grouping_notes
            .contains(&"album_artist_values_inconsistent".to_owned()));
    }

    #[test]
    fn same_title_releases_and_artists_remain_separate_and_source_scoped() {
        let fixture = Fixture::new();
        let root_a = fixture.dir("A");
        for (path, artist, date) in [
            ("edition-1980/01.flac", "Artist One", "1980"),
            ("edition-2000/01.flac", "Artist One", "2000"),
            ("other-artist/01.flac", "Artist Two", "1980"),
        ] {
            Fixture::file(
                &root_a.join(path),
                &metadata::synthetic_flac(&[
                    ("TITLE", "Track"),
                    ("ALBUM", "Shared Name"),
                    ("ALBUMARTIST", artist),
                    ("DATE", date),
                ]),
            );
        }
        let root_b = fixture.dir("B");
        Fixture::file(
            &root_b.join("01.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "Shared Name"),
                ("ALBUMARTIST", "Artist One"),
            ]),
        );

        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_a = catalog.add_source(&root_a, "A").unwrap().source_id;
        let source_b = catalog.add_source(&root_b, "B").unwrap().source_id;
        let albums_a = catalog.albums_for_source(source_a).unwrap();
        let albums_b = catalog.albums_for_source(source_b).unwrap();
        assert_eq!(albums_a.len(), 3);
        assert_eq!(albums_b.len(), 1);
        assert_eq!(
            albums_a
                .iter()
                .map(|album| album.id.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            albums_a.len(),
            "different same-title releases must not collide in derived identity"
        );
        assert!(albums_a.iter().all(|album| album.source_id == source_a));
        assert!(albums_b.iter().all(|album| album.source_id == source_b));
        assert_ne!(albums_a[0].id, albums_b[0].id);
        assert_eq!(
            catalog
                .albums_for_artist(source_a, "artist one")
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            catalog
                .albums_for_artist(source_b, "Artist One")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn explicit_compilation_album_artist_and_multiple_artists_are_preserved() {
        let fixture = Fixture::new();
        let root = fixture.dir("compilation");
        Fixture::file(
            &root.join("01.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Opening"),
                ("ARTIST", "Singer A"),
                ("ARTIST", "Guest A"),
                ("ALBUMARTIST", "Various Artists"),
                ("ALBUMARTIST", "Curated by X"),
                ("ALBUM", "A Compilation"),
            ]),
        );
        Fixture::file(
            &root.join("02.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Next"),
                ("ARTIST", "Singer B"),
                ("ALBUMARTIST", "Various Artists"),
                ("ALBUMARTIST", "Curated by X"),
                ("ALBUM", "A Compilation"),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "compilation").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album_artists, ["Various Artists", "Curated by X"]);
        let artists = catalog.artists_for_source(source_id).unwrap();
        let by_name = |name: &str| artists.iter().find(|artist| artist.name == name).unwrap();
        assert!(by_name("Various Artists").appears_as_album_artist);
        assert!(!by_name("Various Artists").appears_as_track_artist);
        assert_eq!(by_name("Singer A").track_count, 1);
        assert_eq!(by_name("Singer A").album_count, 1);
        assert_eq!(
            catalog
                .albums_for_artist(source_id, "Guest A")
                .unwrap()
                .len(),
            1
        );
        let first = catalog.tracks_for_album(&albums[0].id).unwrap();
        assert_eq!(
            first[0]
                .metadata
                .as_ref()
                .unwrap()
                .last_success
                .as_ref()
                .unwrap()
                .track_artists,
            ["Singer A", "Guest A"]
        );
    }

    #[test]
    fn missing_album_artist_and_dates_are_reported_as_ambiguous_without_inference() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (path, artist, date) in [
            ("album/01.flac", "Track Artist One", "1990"),
            ("album/02.flac", "Track Artist Two", "2000"),
            ("unreleased/01.flac", "Another Artist", ""),
        ] {
            let mut tags = vec![
                ("TITLE", "Title"),
                ("ALBUM", "Unknown Artist Album"),
                ("ARTIST", artist),
            ];
            if !date.is_empty() {
                tags.push(("DATE", date));
            }
            Fixture::file(&root.join(path), &metadata::synthetic_flac(&tags));
        }
        Fixture::file(
            &root.join("no-album.flac"),
            &metadata::synthetic_flac(&[("TITLE", "Loose Track"), ("ARTIST", "Solo")]),
        );

        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 3, "folder-only releases keep Local parity");
        let dated = albums
            .iter()
            .find(|album| album.directory_hint == Path::new("album"))
            .unwrap();
        assert!(dated.album_artists.is_empty());
        assert!(dated.ambiguous);
        assert_eq!(dated.date_values, ["1990", "2000"]);
        assert_eq!(dated.track_count, 2);
        assert!(albums.iter().any(|album| {
            album.directory_hint == Path::new("unreleased") && album.title == "Unknown Artist Album"
        }));
        assert!(albums.iter().any(|album| {
            album.directory_hint == Path::new("") && album.title == "Álbum Desconhecido"
        }));
    }

    #[test]
    fn stale_successful_metadata_is_returned_but_explicitly_marked_stale() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        let path = root.join("album/01.flac");
        Fixture::file(
            &path,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "Persisted Album"),
                ("ALBUMARTIST", "Group"),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        Fixture::file(&path, b"corrupt FLAC contents");
        catalog.verify_source(source_id).unwrap();
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].stale_track_count, 1);
        assert!(albums[0].ambiguous);
        let tracks = catalog.tracks_for_album(&albums[0].id).unwrap();
        assert!(tracks[0].metadata_stale);
        assert_eq!(
            tracks[0]
                .metadata
                .as_ref()
                .unwrap()
                .last_success
                .as_ref()
                .unwrap()
                .album
                .as_deref(),
            Some("Persisted Album")
        );
    }

    #[test]
    fn metadata_update_and_track_removal_immediately_change_derived_groups() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        let first = root.join("album/01.flac");
        let second = root.join("album/02.flac");
        Fixture::file(
            &first,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track one"),
                ("ALBUM", "Original"),
                ("ALBUMARTIST", "Group"),
            ]),
        );
        Fixture::file(
            &second,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track two"),
                ("ALBUM", "Original"),
                ("ALBUMARTIST", "Group"),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        assert_eq!(
            catalog.albums_for_source(source_id).unwrap()[0].track_count,
            2
        );

        Fixture::file(
            &second,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track two"),
                ("ALBUM", "Updated"),
                ("ALBUMARTIST", "Group"),
            ]),
        );
        catalog.verify_source(source_id).unwrap();
        let changed = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(changed.len(), 2);
        assert!(changed.iter().any(|album| album.title == "Updated"));
        assert!(changed.iter().all(|album| album.track_count == 1));

        fs::remove_file(&second).unwrap();
        catalog.verify_source(source_id).unwrap();
        let after_remove = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].title, "Original");
        assert_eq!(after_remove[0].track_count, 1);
    }

    #[test]
    fn browsing_survives_reopen_and_unavailable_source_without_filesystem_access() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        Fixture::file(
            &root.join("album/01.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "Persisted"),
                ("ALBUMARTIST", "Group"),
            ]),
        );
        let source_id;
        let stable_id;
        {
            let mut catalog = Catalog::open(fixture.db()).unwrap();
            source_id = catalog.add_source(&root, "library").unwrap().source_id;
            stable_id = catalog.albums_for_source(source_id).unwrap()[0].id.clone();
        }
        fs::remove_dir_all(&root).unwrap();
        let reopened = Catalog::open(fixture.db()).unwrap();
        let albums = reopened.albums_for_source(source_id).unwrap();
        assert_eq!(albums[0].id, stable_id);
        assert_eq!(reopened.tracks_for_album(&stable_id).unwrap().len(), 1);
    }

    #[test]
    fn source_view_search_uses_persisted_metadata_without_filesystem_access() {
        let fixture = Fixture::new();
        let root = fixture.dir("search-view");
        Fixture::file(
            &root.join("album/01.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Night Train"),
                ("ALBUM", "Blue Window"),
                ("ARTIST", "Ada Example"),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "search").unwrap().source_id;
        let view = catalog.source_library_view(source_id).unwrap();
        fs::remove_dir_all(&root).unwrap();

        let matches = view.search_tracks("ada blue", 10);
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].metadata.as_ref().unwrap().status,
            MetadataStatus::ParsedWithTags
        );
        assert_eq!(
            matches[0].track.relative_path,
            PathBuf::from("album/01.flac")
        );
        assert!(view.search_tracks("no match", 10).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn browse_queries_preserve_unicode_and_non_utf8_relative_paths() {
        use std::os::unix::ffi::OsStringExt;

        let fixture = Fixture::new();
        let root = fixture.dir("library");
        let relative = PathBuf::from(std::ffi::OsString::from_vec(
            b"album/track-\xff.flac".to_vec(),
        ));
        let path = root.join(&relative);
        Fixture::file(
            &path,
            &metadata::synthetic_flac(&[
                ("TITLE", "曲名 – Café"),
                ("ALBUM", "音楽"),
                ("ALBUMARTIST", "演奏者"),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "音楽");
        let tracks = catalog.tracks_for_album(&albums[0].id).unwrap();
        assert_eq!(tracks[0].track.relative_path, relative);
        assert_eq!(albums[0].directory_hint, Path::new("album"));
    }

    #[test]
    fn derived_queries_have_stable_order_and_recompute_after_reopen() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (path, album, title) in [
            ("z/01.flac", "Zulu", "z"),
            ("a/01.flac", "Alpha", "a"),
            ("a/02.flac", "Alpha", "b"),
        ] {
            Fixture::file(
                &root.join(path),
                &metadata::synthetic_flac(&[
                    ("TITLE", title),
                    ("ALBUM", album),
                    ("ALBUMARTIST", "Artist"),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let first = catalog.albums_for_source(source_id).unwrap();
        let second = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|album| album.title.as_str())
                .collect::<Vec<_>>(),
            ["Alpha", "Zulu"]
        );
        let artists = catalog.artists_for_source(source_id).unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].name, "Artist");
        drop(catalog);
        let reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(reopened.albums_for_source(source_id).unwrap(), first);
    }

    #[test]
    fn source_view_matches_legacy_navigation_and_survives_catalog_drop() {
        let fixture = Fixture::new();
        let root = fixture.dir("view-library");
        for (path, album, track, artist, album_artist, disc, number) in [
            (
                "multi/one.flac",
                "Multidisc",
                "One",
                "Singer A",
                "Group",
                1,
                1,
            ),
            (
                "multi/two.flac",
                "Multidisc",
                "Two",
                "Singer B",
                "Group",
                2,
                1,
            ),
            (
                "edition-a/one.flac",
                "Same title",
                "A",
                "Singer A",
                "Group",
                1,
                1,
            ),
            (
                "edition-b/one.flac",
                "Same title",
                "B",
                "Singer A",
                "Group",
                1,
                1,
            ),
            ("comp/one.flac", "Compilation", "C1", "Guest One", "", 1, 1),
            ("comp/two.flac", "Compilation", "C2", "Guest Two", "", 1, 2),
        ] {
            let mut tags = vec![
                ("TITLE", track),
                ("ALBUM", album),
                ("ARTIST", artist),
                ("DISCNUMBER", "1"),
                ("TRACKNUMBER", "1"),
            ];
            tags[3].1 = match disc {
                1 => "1",
                _ => "2",
            };
            tags[4].1 = match number {
                1 => "1",
                _ => "2",
            };
            if !album_artist.is_empty() {
                tags.push(("ALBUMARTIST", album_artist));
            }
            Fixture::file(&root.join(path), &metadata::synthetic_flac(&tags));
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "view").unwrap().source_id;
        let expected_albums = catalog.albums_for_source(source_id).unwrap();
        let expected_artists = catalog.artists_for_source(source_id).unwrap();
        let expected_by_artist = catalog.albums_for_artist(source_id, "singer a").unwrap();
        let expected_tracks = expected_albums
            .iter()
            .map(|album| {
                (
                    album.id.clone(),
                    catalog.tracks_for_album(&album.id).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        let view = catalog.source_library_view(source_id).unwrap();
        assert_eq!(view.source_id(), source_id);
        assert_eq!(view.generation(), catalog.sources().unwrap()[0].generation);
        assert_eq!(view.albums(), expected_albums);
        assert_eq!(view.artists(), expected_artists);
        assert_eq!(
            view.albums_for_artist("SINGER A")
                .cloned()
                .collect::<Vec<_>>(),
            expected_by_artist
        );
        assert_eq!(
            view.albums().len(),
            4,
            "editions and compilation stay distinct"
        );
        let multidisc = view
            .albums()
            .iter()
            .find(|album| album.title == "Multidisc")
            .unwrap();
        assert_eq!(multidisc.track_count, 2);
        assert!(!multidisc.ambiguous);
        let compilation = view
            .albums()
            .iter()
            .find(|album| album.title == "Compilation")
            .unwrap();
        assert!(
            compilation.ambiguous,
            "missing album artist remains ambiguous"
        );
        for album in view.albums() {
            assert_eq!(
                view.tracks_for_album(&album.id)
                    .unwrap()
                    .cloned()
                    .collect::<Vec<_>>(),
                expected_tracks[&album.id]
            );
        }
        assert!(catalog.is_source_library_view_current(&view).unwrap());

        // The view owns a coherent snapshot and its navigation needs neither
        // the catalog connection nor the source files after construction.
        drop(catalog);
        fs::remove_dir_all(&root).unwrap();
        for _ in 0..5 {
            assert_eq!(view.albums(), expected_albums);
            assert_eq!(view.artists(), expected_artists);
            assert_eq!(
                view.tracks_for_album(&expected_albums[0].id).unwrap().len(),
                expected_tracks[&expected_albums[0].id].len()
            );
        }
    }

    #[test]
    fn source_view_is_invalidated_by_update_on_an_independent_connection_but_remains_a_snapshot() {
        let fixture = Fixture::new();
        let root = fixture.dir("view-update");
        let file = root.join("album/01.flac");
        Fixture::file(
            &file,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "Before"),
                ("ALBUMARTIST", "Artist"),
            ]),
        );
        let mut first = Catalog::open(fixture.db()).unwrap();
        let source_id = first.add_source(&root, "view").unwrap().source_id;
        let view = first.source_library_view(source_id).unwrap();
        let old_album = view.albums()[0].clone();
        let mut independent = Catalog::open(fixture.db()).unwrap();
        Fixture::file(
            &file,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "After"),
                ("ALBUMARTIST", "Artist"),
            ]),
        );
        independent.verify_source(source_id).unwrap();
        assert!(!first.is_source_library_view_current(&view).unwrap());
        assert_eq!(view.albums(), &[old_album]);
        assert_eq!(view.albums()[0].title, "Before");
        let refreshed = independent.source_library_view(source_id).unwrap();
        assert_eq!(refreshed.albums()[0].title, "After");
        assert!(independent
            .is_source_library_view_current(&refreshed)
            .unwrap());
    }

    #[test]
    fn source_view_build_is_rejected_if_generation_changes_during_its_sqlite_snapshot() {
        let fixture = Fixture::new();
        let root = fixture.dir("view-build-race");
        let file = root.join("album/01.flac");
        Fixture::file(
            &file,
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "Before"),
                ("ALBUMARTIST", "Artist"),
            ]),
        );
        let mut reader = Catalog::open(fixture.db()).unwrap();
        let source_id = reader.add_source(&root, "view").unwrap().source_id;
        let mut writer = Catalog::open(fixture.db()).unwrap();
        let result = reader.source_library_view_with_row_hook(source_id, || {
            Fixture::file(
                &file,
                &metadata::synthetic_flac(&[
                    ("TITLE", "Track"),
                    ("ALBUM", "After"),
                    ("ALBUMARTIST", "Artist"),
                ]),
            );
            writer.verify_source(source_id).unwrap();
        });
        assert!(result
            .unwrap_err()
            .contains("source changed while building library view"));
    }

    #[test]
    fn source_view_becomes_stale_after_source_removal_and_reopens_from_committed_data() {
        let fixture = Fixture::new();
        let root = fixture.dir("view-remove");
        Fixture::file(
            &root.join("album/01.flac"),
            &metadata::synthetic_flac(&[
                ("TITLE", "Track"),
                ("ALBUM", "Persisted"),
                ("ALBUMARTIST", "Artist"),
            ]),
        );
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "view").unwrap().source_id;
        let view = catalog.source_library_view(source_id).unwrap();
        let expected = view.albums().to_vec();
        let mut reopened = Catalog::open(fixture.db()).unwrap();
        assert_eq!(
            reopened.source_library_view(source_id).unwrap().albums(),
            expected
        );
        reopened.remove_source(source_id).unwrap();
        assert!(!catalog.is_source_library_view_current(&view).unwrap());
        assert_eq!(view.albums(), expected, "old snapshot remains readable");
        assert!(catalog.source_library_view(source_id).is_err());
    }

    #[test]
    fn multidisc_album_without_disc_subfolders_remains_one_group() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (name, disc, number) in [("one.flac", "1", "1"), ("two.flac", "2", "1")] {
            Fixture::file(
                &root.join(name),
                &metadata::synthetic_flac(&[
                    ("TITLE", name),
                    ("ALBUM", "No Disc Folders"),
                    ("ALBUMARTIST", "The Group"),
                    ("DISCNUMBER", disc),
                    ("TRACKNUMBER", number),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 2);
        assert_eq!(albums[0].directory_hint, Path::new(""));
    }

    #[test]
    fn unicode_case_variants_group_conservatively_without_erasing_accents() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (file, title, artist) in [
            ("one.flac", "CAFÉ", "MÚSICO"),
            ("two.flac", "café", "músico"),
            ("three.flac", "cafe", "musico"),
        ] {
            Fixture::file(
                &root.join(file),
                &metadata::synthetic_flac(&[
                    ("TITLE", file),
                    ("ALBUM", title),
                    ("ALBUMARTIST", artist),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 2);
        assert!(albums.iter().any(|album| album.track_count == 2));
        assert!(albums.iter().any(|album| album.track_count == 1));
        let artists = catalog.artists_for_source(source_id).unwrap();
        assert_eq!(artists.len(), 2);
    }

    #[test]
    fn compilation_without_explicit_album_artist_groups_tracks_without_inference() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for (file, artist) in [("one.flac", "Singer A"), ("two.flac", "Singer B")] {
            Fixture::file(
                &root.join(file),
                &metadata::synthetic_flac(&[
                    ("TITLE", file),
                    ("ARTIST", artist),
                    ("ALBUM", "Various Voices"),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let albums = catalog.albums_for_source(source_id).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 2);
        assert!(albums[0].album_artists.is_empty());
        assert!(albums[0].ambiguous);
        assert_eq!(
            catalog
                .albums_for_artist(source_id, "Singer A")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            catalog
                .albums_for_artist(source_id, "Singer B")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn removing_every_track_removes_the_derived_album() {
        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for name in ["one.flac", "two.flac"] {
            Fixture::file(
                &root.join(name),
                &metadata::synthetic_flac(&[
                    ("TITLE", name),
                    ("ALBUM", "Removed Album"),
                    ("ALBUMARTIST", "Artist"),
                ]),
            );
        }
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "library").unwrap().source_id;
        let album_id = catalog.albums_for_source(source_id).unwrap()[0].id.clone();
        fs::remove_file(root.join("one.flac")).unwrap();
        fs::remove_file(root.join("two.flac")).unwrap();
        catalog.verify_source(source_id).unwrap();
        assert!(catalog.albums_for_source(source_id).unwrap().is_empty());
        assert!(catalog.tracks_for_album(&album_id).unwrap().is_empty());
        assert!(catalog
            .albums_for_artist(source_id, "Artist")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn album_browse_keeps_one_sqlite_snapshot_during_a_confirmed_concurrent_update() {
        use std::sync::mpsc;

        let fixture = Fixture::new();
        let root = fixture.dir("library");
        for name in ["one.flac", "two.flac"] {
            Fixture::file(
                &root.join(name),
                &metadata::synthetic_flac(&[
                    ("TITLE", name),
                    ("ALBUM", "Before Update"),
                    ("ALBUMARTIST", "Artist"),
                ]),
            );
        }
        let db_path = fixture.db();
        let mut initial = Catalog::open(&db_path).unwrap();
        let source_id = initial.add_source(&root, "library").unwrap().source_id;
        drop(initial);

        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let reader_db = db_path.clone();
        let reader = thread::spawn(move || {
            let catalog = Catalog::open(reader_db).unwrap();
            catalog
                .albums_for_source_with_row_hook(source_id, || {
                    paused_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                })
                .unwrap()
        });
        paused_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("browse did not pause after its first SQL row");

        for name in ["one.flac", "two.flac"] {
            Fixture::file(
                &root.join(name),
                &metadata::synthetic_flac(&[
                    ("TITLE", name),
                    ("ALBUM", "After Confirmed Update"),
                    ("ALBUMARTIST", "Artist"),
                ]),
            );
        }
        let mut writer = Catalog::open(&db_path).unwrap();
        let update = writer.verify_source(source_id).unwrap();
        assert_eq!(update.metrics.files_modified, 2);
        resume_tx.send(()).unwrap();

        let browsed = reader.join().unwrap();
        assert_eq!(browsed.len(), 1);
        assert_eq!(browsed[0].title, "Before Update");
        assert_eq!(browsed[0].track_count, 2);
        let latest = writer.albums_for_source(source_id).unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].title, "After Confirmed Update");
        assert_eq!(latest[0].track_count, 2);
    }

    #[test]
    fn browse_query_plan_uses_source_and_metadata_primary_keys() {
        let fixture = Fixture::new();
        let root = fixture.dir("empty");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "empty").unwrap().source_id;
        let plan = query(
            &catalog.db.connection,
            "EXPLAIN QUERY PLAN SELECT tracks.source_id,sources.path_format,tracks.relative_path,tracks.size,tracks.mtime_ns,tracks.generation,track_metadata.attempt_status,track_metadata.file_format,track_metadata.attempt_parser_version,track_metadata.attempt_size,track_metadata.attempt_mtime_ns,track_metadata.error,track_metadata.success_size,track_metadata.success_mtime_ns,track_metadata.title,track_metadata.track_artist,track_metadata.album_artist,track_metadata.album,track_metadata.genre,track_metadata.date_text,track_metadata.track_number,track_metadata.disc_number,track_metadata.duration_ms,track_metadata.sample_rate,track_metadata.channels,extra.artist_role,extra.ordinal,extra.value FROM tracks JOIN sources ON sources.id=tracks.source_id LEFT JOIN track_metadata ON track_metadata.source_id=tracks.source_id AND track_metadata.relative_path=tracks.relative_path LEFT JOIN track_metadata_extra_artists AS extra ON extra.source_id=tracks.source_id AND extra.relative_path=tracks.relative_path WHERE tracks.source_id=? ORDER BY tracks.relative_path,extra.artist_role,extra.ordinal",
            &[val_i(source_id)],
        )
        .unwrap()
        .into_iter()
        .map(|row| text(&row, 3))
        .collect::<Vec<_>>();
        eprintln!("B2C-3 browse query plan: {plan:?}");
        assert!(plan
            .iter()
            .any(|line| line.contains("tracks_source_generation")
                || line.contains("sqlite_autoindex_tracks_1")));
        assert!(plan
            .iter()
            .any(|line| line.contains("sqlite_autoindex_track_metadata_1")));
        assert!(plan
            .iter()
            .any(|line| line.contains("sqlite_autoindex_track_metadata_extra_artists_1")));
    }

    #[test]
    fn browse_batch_scales_to_thousands_of_persisted_tracks_with_one_query() {
        let fixture = Fixture::new();
        let root = fixture.dir("synthetic-empty-root");
        let mut catalog = Catalog::open(fixture.db()).unwrap();
        let source_id = catalog.add_source(&root, "synthetic").unwrap().source_id;
        let transaction = Tx::begin(&mut catalog.db).unwrap();
        let track_count = 5_000_i64;
        for index in 0..track_count {
            let album_number = index / 50;
            let relative = PathBuf::from(format!("album-{album_number:03}/track-{index:05}.flac"));
            let encoded = path_codec::encode_relative(&relative).unwrap();
            transaction
                .execute(
                    "INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation) VALUES(?,?,?,?,1)",
                    &[val_i(source_id), val_b(encoded.clone()), val_i(128), val_i(1)],
                )
                .unwrap();
            transaction
                .execute(
                    "INSERT INTO track_metadata(source_id,relative_path,attempt_status,attempt_size,attempt_mtime_ns,attempt_parser_version,file_format,error,success_size,success_mtime_ns,title,track_artist,album_artist,album,genre,date_text,track_number,disc_number,duration_ms,sample_rate,channels) \
                     VALUES(?,?,1,128,1,2,'flac',NULL,128,1,?,?,?,?,?,?,?,1,180000,44100,2)",
                    &[
                        val_i(source_id),
                        val_b(encoded),
                        val_t(format!("Track {index}")),
                        val_t(format!("Track Artist {:02}", index % 25)),
                        val_t(format!("Album Artist {album_number:03}")),
                        val_t(format!("Album {album_number:03}")),
                        val_t("Synthetic".to_owned()),
                        val_t("2020".to_owned()),
                        val_i((index % 50) + 1),
                    ],
                )
                .unwrap();
        }
        transaction.commit().unwrap();

        // The source directory is empty while all 5,000 catalog rows are
        // present, proving browsing does not open or inspect audio files.
        let batch = catalog.browse_batch(source_id).unwrap();
        assert_eq!(batch.tracks.len(), track_count as usize);
        assert_eq!(batch.metrics.sql_queries, 1);
        assert_eq!(batch.metrics.rows_read, track_count as usize);
        let grouping_started = Instant::now();
        let albums = group_albums(&batch.tracks).unwrap();
        let grouping_us = grouping_started.elapsed().as_micros();
        assert_eq!(albums.len(), 100);
        let artists = catalog.artists_for_source(source_id).unwrap();
        assert_eq!(artists.len(), 125);
        eprintln!(
            "B2C-2 synthetic browse: tracks={} albums={} artists={} read_us={} grouping_us={} SQL_queries=2 audio_files_opened=0",
            batch.tracks.len(),
            albums.len(),
            artists.len(),
            batch.metrics.read_duration_us,
            grouping_us,
        );
    }

    #[test]
    #[ignore = "synthetic scale benchmark; run explicitly with --ignored --nocapture"]
    fn b2c3_synthetic_scale_benchmark() {
        use std::hint::black_box;

        fn summarize(mut values: Vec<u128>) -> (u128, u128, u128) {
            values.sort_unstable();
            (
                values[0],
                values[values.len() / 2],
                values[values.len() - 1],
            )
        }

        fn measure<T>(
            mut operation: impl FnMut() -> T,
            repetitions: usize,
        ) -> (u128, u128, u128, T) {
            let mut timings = Vec::with_capacity(repetitions);
            let mut last = None;
            for _ in 0..repetitions {
                let started = Instant::now();
                last = Some(black_box(operation()));
                timings.push(started.elapsed().as_micros());
            }
            let (minimum, middle, maximum) = summarize(timings);
            (
                minimum,
                middle,
                maximum,
                last.expect("at least one repetition"),
            )
        }

        for track_count in [5_000_i64, 25_000, 50_000, 100_000] {
            let fixture = Fixture::new();
            let root = fixture.dir("synthetic-empty-root");
            let mut catalog = Catalog::open(fixture.db()).unwrap();
            let source_id = catalog.add_source(&root, "synthetic").unwrap().source_id;
            let transaction = Tx::begin(&mut catalog.db).unwrap();
            let mut track_insert = transaction
                .transaction
                .prepare(
                    "INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation) VALUES(?,?,128,1,1)",
                )
                .unwrap();
            let mut metadata_insert = transaction
                .transaction
                .prepare(
                    "INSERT INTO track_metadata(source_id,relative_path,attempt_status,attempt_size,attempt_mtime_ns,attempt_parser_version,file_format,error,success_size,success_mtime_ns,title,track_artist,album_artist,album,genre,date_text,track_number,disc_number,duration_ms,sample_rate,channels) VALUES(?,?,1,128,1,2,'flac',NULL,128,1,?,?,?,?,?,?,?, ?,180000,44100,2)",
                )
                .unwrap();
            for index in 0..track_count {
                let album_number = index / 50;
                let disc_number = (index % 50) / 25 + 1;
                let compilation = album_number % 5 == 0;
                let directory = format!(
                    "artist-set-{}/album-{album_number:04}/CD {disc_number}",
                    album_number % 16
                );
                let relative = PathBuf::from(format!("{directory}/track-{index:06}.flac"));
                let encoded = path_codec::encode_relative(&relative).unwrap();
                let artist = if compilation {
                    format!("Compilation Singer {:02}", index % 70)
                } else {
                    format!("Artist {:02}", album_number % 80)
                };
                let album_artist = if compilation {
                    "Various Artists".to_owned()
                } else {
                    format!("Artist {:02}", album_number % 80)
                };
                track_insert
                    .execute(rusqlite::params![source_id, encoded.clone()])
                    .unwrap();
                metadata_insert
                    .execute(rusqlite::params![
                        source_id,
                        encoded,
                        format!("Track {index:06}"),
                        artist,
                        album_artist,
                        format!("Album {album_number:04}"),
                        "Synthetic",
                        "2020",
                        (index % 25) + 1,
                        disc_number,
                    ])
                    .unwrap();
            }
            drop(track_insert);
            drop(metadata_insert);
            transaction.commit().unwrap();

            // Warm the SQLite page cache before repeated-query medians. The
            // separate reopen measurement below captures a newly opened handle,
            // but deliberately does not claim to flush the operating-system cache.
            let warm_batch = catalog.browse_batch(source_id).unwrap();
            let albums = group_albums(&warm_batch.tracks)
                .unwrap()
                .into_iter()
                .map(|group| group.album)
                .collect::<Vec<_>>();
            let artists = catalog.artists_for_source(source_id).unwrap();
            assert_eq!(warm_batch.tracks.len(), track_count as usize);
            assert_eq!(albums.len(), track_count as usize / 50);
            assert!(!artists.is_empty());
            let small_album = albums
                .iter()
                .find(|album| album.track_count == 50)
                .expect("fixture contains a 50-track album");
            let artist_name = artists[0].name.clone();

            let (read_min_us, read_us, read_max_us) = summarize(
                (0..3)
                    .map(|_| {
                        catalog
                            .browse_batch(source_id)
                            .unwrap()
                            .metrics
                            .read_duration_us
                    })
                    .collect(),
            );
            let (grouping_min_us, grouping_us, grouping_max_us) = summarize(
                (0..3)
                    .map(|_| {
                        let batch = catalog.browse_batch(source_id).unwrap();
                        let started = Instant::now();
                        black_box(group_albums(&batch.tracks).unwrap());
                        started.elapsed().as_micros()
                    })
                    .collect(),
            );
            let (albums_min_us, albums_total_us, albums_max_us, albums_result) =
                measure(|| catalog.albums_for_source(source_id).unwrap(), 3);
            let (artists_min_us, artists_total_us, artists_max_us, artist_result) =
                measure(|| catalog.artists_for_source(source_id).unwrap(), 3);
            let (tracks_min_us, tracks_total_us, tracks_max_us, tracks_result) =
                measure(|| catalog.tracks_for_album(&small_album.id).unwrap(), 3);
            let (
                artist_albums_min_us,
                artist_albums_total_us,
                artist_albums_max_us,
                artist_albums_result,
            ) = measure(
                || catalog.albums_for_artist(source_id, &artist_name).unwrap(),
                3,
            );
            let repeated_navigation = || {
                for album in albums.iter().take(5) {
                    black_box(catalog.tracks_for_album(&album.id).unwrap());
                }
            };
            let (navigation_five_min_us, navigation_five_total_us, navigation_five_max_us, ()) =
                measure(repeated_navigation, 3);

            drop(catalog);
            let reopen_started = Instant::now();
            let reopened = Catalog::open(fixture.db()).unwrap();
            let reopen_us = reopen_started.elapsed().as_micros();
            let (_, reopen_first_query_us, _, first_query) =
                measure(|| reopened.albums_for_source(source_id).unwrap(), 1);
            assert_eq!(first_query, albums_result);
            let (
                repeated_open_query_min_us,
                repeated_open_query_us,
                repeated_open_query_max_us,
                repeated_query,
            ) = measure(|| reopened.albums_for_source(source_id).unwrap(), 3);
            assert_eq!(repeated_query, albums_result);
            assert_eq!(tracks_result.len(), small_album.track_count);
            assert_eq!(artist_result.len(), artists.len());

            eprintln!(
                "B2C-3 scale tracks={track_count} albums={} artists={} sql_queries_per_api=1 batch_rows={} batch_read_us={read_min_us}/{read_us}/{read_max_us} album_group_us={grouping_min_us}/{grouping_us}/{grouping_max_us} albums_api_us={albums_min_us}/{albums_total_us}/{albums_max_us} artists_api_us={artists_min_us}/{artists_total_us}/{artists_max_us} tracks_for_small_album_us={tracks_min_us}/{tracks_total_us}/{tracks_max_us} tracks_returned={} tracks_loaded_by_album_api={} albums_for_artist_us={artist_albums_min_us}/{artist_albums_total_us}/{artist_albums_max_us} albums_returned={} album_navigation_5_total_us={navigation_five_min_us}/{navigation_five_total_us}/{navigation_five_max_us} per_album_navigation_us={} reopen_us={reopen_us} reopen_plus_first_query_us={reopen_first_query_us} repeated_open_query_us={repeated_open_query_min_us}/{repeated_open_query_us}/{repeated_open_query_max_us} audio_files_opened=0",
                albums.len(),
                artists.len(),
                warm_batch.metrics.rows_read,
                tracks_result.len(),
                warm_batch.metrics.rows_read,
                artist_albums_result.len(),
                navigation_five_total_us / 5,
            );
        }
    }

    #[test]
    #[ignore = "synthetic release benchmark for immutable source navigation views"]
    fn b2c4_source_library_view_release_benchmark() {
        use std::hint::black_box;

        fn summarize(mut values: Vec<u128>) -> (u128, u128, u128) {
            values.sort_unstable();
            (
                values[0],
                values[values.len() / 2],
                values[values.len() - 1],
            )
        }

        fn measure<T>(
            mut operation: impl FnMut() -> T,
            repetitions: usize,
        ) -> (u128, u128, u128, T) {
            let mut timings = Vec::with_capacity(repetitions);
            let mut last = None;
            for _ in 0..repetitions {
                let started = Instant::now();
                last = Some(black_box(operation()));
                timings.push(started.elapsed().as_nanos());
            }
            let (minimum, median, maximum) = summarize(timings);
            (
                minimum,
                median,
                maximum,
                last.expect("non-empty repetitions"),
            )
        }

        fn measure_many<T>(
            mut operation: impl FnMut() -> T,
            samples: usize,
            iterations_per_sample: usize,
        ) -> (u128, u128, u128, T) {
            let mut timings = Vec::with_capacity(samples);
            let mut last = None;
            for _ in 0..samples {
                let started = Instant::now();
                for _ in 0..iterations_per_sample {
                    last = Some(black_box(operation()));
                }
                timings.push(started.elapsed().as_nanos() / iterations_per_sample as u128);
            }
            let (minimum, median, maximum) = summarize(timings);
            (
                minimum,
                median,
                maximum,
                last.expect("non-empty repetitions"),
            )
        }

        for track_count in [5_000_i64, 25_000, 50_000, 100_000] {
            let fixture = Fixture::new();
            let root = fixture.dir("synthetic-empty-root");
            let mut catalog = Catalog::open(fixture.db()).unwrap();
            let source_id = catalog.add_source(&root, "synthetic").unwrap().source_id;
            let transaction = Tx::begin(&mut catalog.db).unwrap();
            let mut track_insert = transaction
                .transaction
                .prepare(
                    "INSERT INTO tracks(source_id,relative_path,size,mtime_ns,generation) VALUES(?,?,128,1,1)",
                )
                .unwrap();
            let mut metadata_insert = transaction
                .transaction
                .prepare(
                    "INSERT INTO track_metadata(source_id,relative_path,attempt_status,attempt_size,attempt_mtime_ns,attempt_parser_version,file_format,error,success_size,success_mtime_ns,title,track_artist,album_artist,album,genre,date_text,track_number,disc_number,duration_ms,sample_rate,channels) VALUES(?,?,1,128,1,2,'flac',NULL,128,1,?,?,?,?,?,?,?, ?,180000,44100,2)",
                )
                .unwrap();
            for index in 0..track_count {
                let album_number = index / 50;
                let disc_number = (index % 50) / 25 + 1;
                let compilation = album_number % 5 == 0;
                let directory = format!(
                    "artist-set-{}/album-{album_number:04}/CD {disc_number}",
                    album_number % 16
                );
                let relative = PathBuf::from(format!("{directory}/track-{index:06}.flac"));
                let encoded = path_codec::encode_relative(&relative).unwrap();
                let artist = if compilation {
                    format!("Compilation Singer {:02}", index % 70)
                } else {
                    format!("Artist {:02}", album_number % 80)
                };
                let album_artist = if compilation {
                    "Various Artists".to_owned()
                } else {
                    format!("Artist {:02}", album_number % 80)
                };
                track_insert
                    .execute(rusqlite::params![source_id, encoded.clone()])
                    .unwrap();
                metadata_insert
                    .execute(rusqlite::params![
                        source_id,
                        encoded,
                        format!("Track {index:06}"),
                        artist,
                        album_artist,
                        format!("Album {album_number:04}"),
                        "Synthetic",
                        "2020",
                        (index % 25) + 1,
                        disc_number,
                    ])
                    .unwrap();
            }
            drop(track_insert);
            drop(metadata_insert);
            transaction.commit().unwrap();

            // This one joined SELECT is the view's consistent SQLite snapshot;
            // grouping/index construction happens after its statement closes.
            let build_started = Instant::now();
            let view = catalog.source_library_view(source_id).unwrap();
            let initial_build_ns = build_started.elapsed().as_nanos();
            assert_eq!(view.albums().len(), track_count as usize / 50);
            assert_eq!(view.tracks_by_album.len(), view.albums().len());
            assert!(catalog.is_source_library_view_current(&view).unwrap());

            let album_read = || {
                view.albums().iter().for_each(|album| {
                    black_box(&album.id);
                });
                view.albums().len()
            };
            let artist_read = || {
                view.artists().iter().for_each(|artist| {
                    black_box(&artist.name);
                });
                view.artists().len()
            };
            let small_album = view
                .albums()
                .iter()
                .find(|album| album.track_count == 50)
                .expect("fixture includes 50-track albums");
            let small_album_id = small_album.id.clone();
            let artist_name = view.artists()[0].name.clone();
            let (albums_min, albums_median, albums_max, album_count) =
                measure_many(album_read, 7, 1_000);
            let (artists_min, artists_median, artists_max, artist_count) =
                measure_many(artist_read, 7, 1_000);
            let (tracks_min, tracks_median, tracks_max, track_count_result) = measure_many(
                || view.tracks_for_album(&small_album_id).unwrap().count(),
                7,
                1_000,
            );
            let (artist_albums_min, artist_albums_median, artist_albums_max, artist_album_count) =
                measure_many(|| view.albums_for_artist(&artist_name).count(), 7, 1_000);
            let navigation = || {
                view.albums()
                    .iter()
                    .take(5)
                    .map(|album| view.tracks_for_album(&album.id).unwrap().count())
                    .sum::<usize>()
            };
            let (five_min, five_median, five_max, five_tracks) = measure_many(navigation, 7, 1_000);
            let (validity_min, validity_median, validity_max, all_current) = measure_many(
                || catalog.is_source_library_view_current(&view).unwrap(),
                7,
                100,
            );
            assert_eq!(album_count, view.albums().len());
            assert_eq!(artist_count, view.artists().len());
            assert_eq!(track_count_result, 50);
            assert!(artist_album_count > 0);
            assert!(five_tracks > 0);
            assert!(all_current);

            let (build_min, build_median, build_max, rebuilt) =
                measure(|| catalog.source_library_view(source_id).unwrap(), 3);
            assert_eq!(rebuilt.albums(), view.albums());
            assert_eq!(rebuilt.artists(), view.artists());
            drop(rebuilt);

            drop(catalog);
            let reopen_started = Instant::now();
            let reopened = Catalog::open(fixture.db()).unwrap();
            let reopen_ns = reopen_started.elapsed().as_nanos();
            let first_view_started = Instant::now();
            let reopened_view = reopened.source_library_view(source_id).unwrap();
            let reopen_build_ns = first_view_started.elapsed().as_nanos();
            assert_eq!(reopened_view.albums(), view.albums());
            assert_eq!(reopened_view.artists(), view.artists());

            eprintln!(
                "B2C-4 view tracks={track_count} albums={} artists={} initial_build_ns={initial_build_ns} repeat_build_ns={build_min}/{build_median}/{build_max} albums_list_ns={albums_min}/{albums_median}/{albums_max} artists_list_ns={artists_min}/{artists_median}/{artists_max} tracks_for_50_ns={tracks_min}/{tracks_median}/{tracks_max} albums_for_artist_ns={artist_albums_min}/{artist_albums_median}/{artist_albums_max} five_navigations_ns={five_min}/{five_median}/{five_max} generation_check_ns={validity_min}/{validity_median}/{validity_max} reopen_ns={reopen_ns} reopen_plus_first_view_ns={reopen_build_ns} audio_files_opened=0",
                view.albums().len(),
                view.artists().len(),
            );
        }
    }
}
