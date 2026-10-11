use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::picture::Picture;
use lofty::probe::Probe;
use lofty::properties::FileProperties;
use lofty::tag::{Accessor, ItemKey, Tag, TagItem, TagType};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufReader, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;

use crate::local_media::{resolve_local_path, LocalPathResolveError};

const MAX_TAG_STRING_BYTES: usize = 4 * 1024;
const MAX_DATE_BYTES: usize = 128;
const MAX_XATTR_BYTES: usize = 1024 * 1024;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);
static TAG_WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalTrackTagsErrorCode {
    InvalidLocalPath,
    OutsideConfiguredRoots,
    FileNotFound,
    CueVirtual,
    UnsupportedFormat,
    ReadFailed,
    InvalidTagUpdate,
    RevisionConflict,
    PermissionDenied,
    TransactionFailed,
    MetadataPreservationFailed,
    DurabilityFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalTrackTagsError {
    pub code: LocalTrackTagsErrorCode,
    pub message: String,
    pub format: Option<String>,
    pub editable: bool,
    pub unsupported_reason: Option<String>,
    pub committed: bool,
}

impl LocalTrackTagsError {
    fn new(code: LocalTrackTagsErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.to_string(),
            format: None,
            editable: false,
            unsupported_reason: None,
            committed: false,
        }
    }

    fn unsupported(code: LocalTrackTagsErrorCode, format: &str, reason: &str) -> Self {
        Self {
            code,
            message: reason.to_string(),
            format: Some(format.to_string()),
            editable: false,
            unsupported_reason: Some(reason.to_string()),
            committed: false,
        }
    }

    pub(crate) fn state_unavailable() -> Self {
        Self::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "A configuração das pastas locais não está disponível.",
        )
    }

    pub(crate) fn task_failed() -> Self {
        Self::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "A tarefa de inspeção das tags locais falhou.",
        )
    }

    pub(crate) fn album_catalog_failed() -> Self {
        Self::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "Não foi possível consultar o álbum local.",
        )
    }

    pub(crate) fn album_not_found() -> Self {
        Self::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "O álbum local não foi encontrado no catálogo atual.",
        )
    }

    fn committed(code: LocalTrackTagsErrorCode, message: &str) -> Self {
        let mut error = Self::new(code, message);
        error.committed = true;
        error
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalTrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_total: Option<u32>,
    pub format: String,
    pub readable: bool,
    pub writable: bool,
    pub editable: bool,
    pub unsupported_reason: Option<String>,
    pub revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAlbumTrackKind {
    Physical,
    CueVirtual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTrackInspection {
    pub uri: String,
    pub kind: LocalAlbumTrackKind,
    pub tags: Option<LocalTrackTags>,
    pub issue: Option<LocalTrackTagsError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAlbumFieldState {
    Common,
    Absent,
    Mixed,
    Indeterminate,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumFieldSummary {
    pub state: LocalAlbumFieldState,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagFields {
    pub album: LocalAlbumFieldSummary,
    pub album_artist: LocalAlbumFieldSummary,
    pub date: LocalAlbumFieldSummary,
    pub genre: LocalAlbumFieldSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagInspection {
    pub album_id: String,
    pub tracks: Vec<LocalAlbumTrackInspection>,
    pub fields: LocalAlbumTagFields,
    pub physical_track_count: usize,
    pub cue_virtual_count: usize,
    pub inspection_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LocalAlbumTagBatchExpectedTrack {
    pub uri: String,
    pub kind: LocalAlbumTrackKind,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct LocalAlbumTagBatchUpdate {
    #[serde(default)]
    pub album: TagFieldUpdate<String>,
    #[serde(default)]
    pub album_artist: TagFieldUpdate<String>,
    #[serde(default)]
    pub date: TagFieldUpdate<String>,
    #[serde(default)]
    pub genre: TagFieldUpdate<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LocalAlbumTagBatchPreflightRequest {
    pub album_id: String,
    pub tracks: Vec<LocalAlbumTagBatchExpectedTrack>,
    pub update: LocalAlbumTagBatchUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAlbumTagBatchBlockedReason {
    TrackInspectionFailed,
    CueVirtual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagBatchTrackPlan {
    pub uri: String,
    pub expected_revision: String,
    pub update: LocalTrackTagUpdate,
    pub needs_write: bool,
    pub issue: Option<LocalTrackTagsError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagBatchPlan {
    pub album_id: String,
    pub applicable: bool,
    pub blocked_reason: Option<LocalAlbumTagBatchBlockedReason>,
    pub tracks: Vec<LocalAlbumTagBatchTrackPlan>,
    pub change_count: usize,
    pub unchanged_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAlbumTagBatchTrackOutcomeState {
    Unchanged,
    Committed,
    Failed,
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagBatchTrackOutcome {
    pub uri: String,
    pub state: LocalAlbumTagBatchTrackOutcomeState,
    pub expected_revision: String,
    pub tags: Option<LocalTrackTags>,
    pub error: Option<LocalTrackTagsError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagBatchApplyResult {
    pub album_id: String,
    pub applicable: bool,
    pub blocked_reason: Option<LocalAlbumTagBatchBlockedReason>,
    pub tracks: Vec<LocalAlbumTagBatchTrackOutcome>,
    pub committed_count: usize,
    pub unchanged_count: usize,
    pub failed_count: usize,
    pub not_attempted_count: usize,
    pub partial_commit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalAlbumTagBatchPreflightErrorCode {
    AlbumCatalogFailure,
    AlbumNotFound,
    MembershipChanged,
    DuplicateExpectedUri,
    InvalidSnapshot,
    RevisionConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalAlbumTagBatchPreflightError {
    pub code: LocalAlbumTagBatchPreflightErrorCode,
    pub message: String,
}

impl LocalAlbumTagBatchPreflightError {
    fn new(code: LocalAlbumTagBatchPreflightErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    pub(crate) fn album_catalog_failed() -> Self {
        Self::new(
            LocalAlbumTagBatchPreflightErrorCode::AlbumCatalogFailure,
            "Não foi possível consultar o álbum local.",
        )
    }

    pub(crate) fn album_not_found() -> Self {
        Self::new(
            LocalAlbumTagBatchPreflightErrorCode::AlbumNotFound,
            "O álbum local não foi encontrado no catálogo atual.",
        )
    }

    pub(crate) fn state_unavailable() -> Self {
        Self::new(
            LocalAlbumTagBatchPreflightErrorCode::AlbumCatalogFailure,
            "A configuração das pastas locais não está disponível.",
        )
    }

    pub(crate) fn task_failed() -> Self {
        Self::new(
            LocalAlbumTagBatchPreflightErrorCode::AlbumCatalogFailure,
            "A tarefa de preflight das tags do álbum falhou.",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTrackTagWriteResult {
    pub tags: LocalTrackTags,
    pub committed: bool,
}

struct ResolvedLocalFile {
    file: File,
    extension: Option<String>,
    canonical_path: PathBuf,
    selected_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
pub enum TagFieldUpdate<T> {
    Unchanged,
    Set(T),
    Remove,
}

impl<T> Default for TagFieldUpdate<T> {
    fn default() -> Self {
        Self::Unchanged
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct LocalTrackTagUpdate {
    #[serde(default)]
    pub title: TagFieldUpdate<String>,
    #[serde(default)]
    pub artist: TagFieldUpdate<String>,
    #[serde(default)]
    pub album: TagFieldUpdate<String>,
    #[serde(default)]
    pub album_artist: TagFieldUpdate<String>,
    #[serde(default)]
    pub genre: TagFieldUpdate<String>,
    #[serde(default)]
    pub date: TagFieldUpdate<String>,
    #[serde(default)]
    pub track_number: TagFieldUpdate<u32>,
    #[serde(default)]
    pub track_total: TagFieldUpdate<u32>,
    #[serde(default)]
    pub disc_number: TagFieldUpdate<u32>,
    #[serde(default)]
    pub disc_total: TagFieldUpdate<u32>,
}

pub fn inspect_local_track_tags(
    library_dir: &Path,
    relative_path: &str,
    local_folders: &[String],
) -> Result<LocalTrackTags, LocalTrackTagsError> {
    if is_virtual_cue_track(relative_path) {
        return Err(LocalTrackTagsError::unsupported(
            LocalTrackTagsErrorCode::CueVirtual,
            "cue_virtual",
            "Faixas virtuais CUE não possuem um arquivo individual editável.",
        ));
    }

    let mut resolved = resolve_local_track_file(library_dir, relative_path, local_folders)?;
    let revision = revision_for_file(&resolved.file)?;
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false);
    let probe = Probe::new(BufReader::new(&mut resolved.file))
        .options(options)
        .guess_file_type()
        .map_err(|_| {
            LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::ReadFailed,
                "Não foi possível identificar o arquivo local.",
            )
        })?;

    if probe.file_type().is_none() {
        let format = resolved.extension.as_deref().unwrap_or("unknown");
        return Err(LocalTrackTagsError::unsupported(
            LocalTrackTagsErrorCode::UnsupportedFormat,
            format,
            "O formato do arquivo não é suportado para inspeção de tags.",
        ));
    }

    let tagged_file = probe.read().map_err(|_| {
        LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "Não foi possível ler as tags do arquivo local.",
        )
    })?;
    Ok(local_track_tags_from_file(&tagged_file, revision))
}

pub fn inspect_local_album_tags(
    library_dir: &Path,
    album_id: &str,
    track_uris: Vec<String>,
    local_folders: &[String],
) -> LocalAlbumTagInspection {
    let tracks = track_uris
        .into_iter()
        .map(|uri| {
            let kind = if is_virtual_cue_track(&uri) {
                LocalAlbumTrackKind::CueVirtual
            } else {
                LocalAlbumTrackKind::Physical
            };
            match inspect_local_track_tags(library_dir, &uri, local_folders) {
                Ok(tags) => LocalAlbumTrackInspection {
                    uri,
                    kind,
                    tags: Some(tags),
                    issue: None,
                },
                Err(issue) => LocalAlbumTrackInspection {
                    uri,
                    kind,
                    tags: None,
                    issue: Some(issue),
                },
            }
        })
        .collect::<Vec<_>>();
    let physical_track_count = tracks
        .iter()
        .filter(|track| track.kind == LocalAlbumTrackKind::Physical)
        .count();
    let cue_virtual_count = tracks.len() - physical_track_count;
    let inspection_complete = tracks
        .iter()
        .all(|track| track.kind == LocalAlbumTrackKind::CueVirtual || track.tags.is_some());
    let fields = LocalAlbumTagFields {
        album: summarize_album_field(&tracks, |tags| tags.album.as_deref()),
        album_artist: summarize_album_field(&tracks, |tags| tags.album_artist.as_deref()),
        date: summarize_album_field(&tracks, |tags| tags.date.as_deref()),
        genre: summarize_album_field(&tracks, |tags| tags.genre.as_deref()),
    };

    LocalAlbumTagInspection {
        album_id: album_id.to_string(),
        tracks,
        fields,
        physical_track_count,
        cue_virtual_count,
        inspection_complete,
    }
}

pub fn preflight_local_album_tag_batch(
    library_dir: &Path,
    request: LocalAlbumTagBatchPreflightRequest,
    current_track_uris: Vec<String>,
    local_folders: &[String],
) -> Result<LocalAlbumTagBatchPlan, LocalAlbumTagBatchPreflightError> {
    let mut expected_tracks = HashMap::with_capacity(request.tracks.len());
    for track in &request.tracks {
        if expected_tracks.insert(track.uri.as_str(), track).is_some() {
            return Err(LocalAlbumTagBatchPreflightError::new(
                LocalAlbumTagBatchPreflightErrorCode::DuplicateExpectedUri,
                "O snapshot esperado contém uma faixa duplicada.",
            ));
        }
        let valid_revision = match track.kind {
            LocalAlbumTrackKind::Physical => track
                .revision
                .as_deref()
                .is_some_and(|revision| !revision.is_empty()),
            LocalAlbumTrackKind::CueVirtual => track.revision.is_none(),
        };
        if !valid_revision {
            return Err(LocalAlbumTagBatchPreflightError::new(
                LocalAlbumTagBatchPreflightErrorCode::InvalidSnapshot,
                "O snapshot esperado do álbum é inválido.",
            ));
        }
    }

    let mut current_uris = HashSet::with_capacity(current_track_uris.len());
    for uri in &current_track_uris {
        if !current_uris.insert(uri.as_str()) {
            return Err(membership_changed());
        }
        let current_kind = track_kind(uri);
        if expected_tracks
            .get(uri.as_str())
            .is_none_or(|expected| expected.kind != current_kind)
        {
            return Err(membership_changed());
        }
    }
    if current_track_uris.len() != expected_tracks.len() {
        return Err(membership_changed());
    }

    let requested_change = request.update.has_changes();
    let has_virtual_cue = current_track_uris
        .iter()
        .any(|uri| track_kind(uri) == LocalAlbumTrackKind::CueVirtual);
    let mut tracks = Vec::new();
    let mut inspection_failed = false;

    // Only the freshly resolved backend membership is used for filesystem access.
    for uri in current_track_uris {
        if track_kind(&uri) == LocalAlbumTrackKind::CueVirtual {
            continue;
        }
        let Some(expected_revision) = expected_tracks
            .get(uri.as_str())
            .and_then(|track| track.revision.clone())
        else {
            return Err(LocalAlbumTagBatchPreflightError::new(
                LocalAlbumTagBatchPreflightErrorCode::InvalidSnapshot,
                "O snapshot esperado do álbum é inválido.",
            ));
        };
        match inspect_local_track_tags(library_dir, &uri, local_folders) {
            Ok(tags) => {
                if tags.revision != expected_revision {
                    return Err(LocalAlbumTagBatchPreflightError::new(
                        LocalAlbumTagBatchPreflightErrorCode::RevisionConflict,
                        "Uma faixa do álbum mudou desde a inspeção anterior.",
                    ));
                }
                let update = request.update.for_track(&tags);
                let needs_write = !update.is_unchanged();
                tracks.push(LocalAlbumTagBatchTrackPlan {
                    uri,
                    expected_revision,
                    update,
                    needs_write,
                    issue: None,
                });
            }
            Err(issue) => {
                inspection_failed = true;
                tracks.push(LocalAlbumTagBatchTrackPlan {
                    uri,
                    expected_revision,
                    update: LocalTrackTagUpdate::default(),
                    needs_write: false,
                    issue: Some(issue),
                });
            }
        }
    }

    let blocked_reason = if inspection_failed {
        Some(LocalAlbumTagBatchBlockedReason::TrackInspectionFailed)
    } else if has_virtual_cue && requested_change {
        Some(LocalAlbumTagBatchBlockedReason::CueVirtual)
    } else {
        None
    };
    let change_count = tracks.iter().filter(|track| track.needs_write).count();
    let unchanged_count = tracks
        .iter()
        .filter(|track| !track.needs_write && track.issue.is_none())
        .count();

    Ok(LocalAlbumTagBatchPlan {
        album_id: request.album_id,
        applicable: blocked_reason.is_none(),
        blocked_reason,
        tracks,
        change_count,
        unchanged_count,
    })
}

pub fn apply_local_album_tag_batch(
    library_dir: &Path,
    request: LocalAlbumTagBatchPreflightRequest,
    current_track_uris: Vec<String>,
    local_folders: &[String],
) -> Result<LocalAlbumTagBatchApplyResult, LocalAlbumTagBatchPreflightError> {
    let plan = preflight_local_album_tag_batch(
        library_dir,
        request,
        current_track_uris,
        local_folders,
    )?;
    if !plan.applicable {
        return Ok(batch_result_without_writes(plan));
    }
    Ok(execute_local_album_tag_batch_plan(
        library_dir,
        plan,
        local_folders,
    ))
}

fn batch_result_without_writes(
    mut plan: LocalAlbumTagBatchPlan,
) -> LocalAlbumTagBatchApplyResult {
    plan.tracks.sort_by(|left, right| left.uri.cmp(&right.uri));
    let tracks = plan
        .tracks
        .into_iter()
        .map(|track| LocalAlbumTagBatchTrackOutcome {
            uri: track.uri,
            state: if track.needs_write || track.issue.is_some() {
                LocalAlbumTagBatchTrackOutcomeState::NotAttempted
            } else {
                LocalAlbumTagBatchTrackOutcomeState::Unchanged
            },
            expected_revision: track.expected_revision,
            tags: None,
            error: track.issue,
        })
        .collect::<Vec<_>>();
    finish_batch_result(plan.album_id, false, plan.blocked_reason, tracks)
}

fn execute_local_album_tag_batch_plan(
    library_dir: &Path,
    plan: LocalAlbumTagBatchPlan,
    local_folders: &[String],
) -> LocalAlbumTagBatchApplyResult {
    execute_local_album_tag_batch_plan_with(plan, |track| {
        update_local_track_tags(
            library_dir,
            &track.uri,
            local_folders,
            &track.expected_revision,
            &track.update,
        )
    })
}

fn execute_local_album_tag_batch_plan_with<F>(
    mut plan: LocalAlbumTagBatchPlan,
    mut writer: F,
) -> LocalAlbumTagBatchApplyResult
where
    F: FnMut(
        &LocalAlbumTagBatchTrackPlan,
    ) -> Result<LocalTrackTagWriteResult, LocalTrackTagsError>,
{
    plan.tracks.sort_by(|left, right| left.uri.cmp(&right.uri));
    let mut stopped = false;
    let mut outcomes = Vec::with_capacity(plan.tracks.len());

    for track in plan.tracks {
        if !track.needs_write {
            outcomes.push(LocalAlbumTagBatchTrackOutcome {
                uri: track.uri,
                state: LocalAlbumTagBatchTrackOutcomeState::Unchanged,
                expected_revision: track.expected_revision,
                tags: None,
                error: None,
            });
            continue;
        }
        if stopped {
            outcomes.push(LocalAlbumTagBatchTrackOutcome {
                uri: track.uri,
                state: LocalAlbumTagBatchTrackOutcomeState::NotAttempted,
                expected_revision: track.expected_revision,
                tags: None,
                error: None,
            });
            continue;
        }

        match writer(&track) {
            Ok(result) if result.committed => {
                outcomes.push(LocalAlbumTagBatchTrackOutcome {
                    uri: track.uri,
                    state: LocalAlbumTagBatchTrackOutcomeState::Committed,
                    expected_revision: track.expected_revision,
                    tags: Some(result.tags),
                    error: None,
                });
            }
            Ok(result) => {
                outcomes.push(LocalAlbumTagBatchTrackOutcome {
                    uri: track.uri,
                    state: LocalAlbumTagBatchTrackOutcomeState::Unchanged,
                    expected_revision: track.expected_revision,
                    tags: Some(result.tags),
                    error: None,
                });
            }
            Err(error) => {
                let state = if error.committed {
                    LocalAlbumTagBatchTrackOutcomeState::Committed
                } else {
                    LocalAlbumTagBatchTrackOutcomeState::Failed
                };
                outcomes.push(LocalAlbumTagBatchTrackOutcome {
                    uri: track.uri,
                    state,
                    expected_revision: track.expected_revision,
                    tags: None,
                    error: Some(error),
                });
                stopped = true;
            }
        }
    }

    finish_batch_result(plan.album_id, true, None, outcomes)
}

fn finish_batch_result(
    album_id: String,
    applicable: bool,
    blocked_reason: Option<LocalAlbumTagBatchBlockedReason>,
    tracks: Vec<LocalAlbumTagBatchTrackOutcome>,
) -> LocalAlbumTagBatchApplyResult {
    let committed_count = count_batch_outcomes(
        &tracks,
        LocalAlbumTagBatchTrackOutcomeState::Committed,
    );
    let unchanged_count = count_batch_outcomes(
        &tracks,
        LocalAlbumTagBatchTrackOutcomeState::Unchanged,
    );
    let failed_count =
        count_batch_outcomes(&tracks, LocalAlbumTagBatchTrackOutcomeState::Failed);
    let not_attempted_count = count_batch_outcomes(
        &tracks,
        LocalAlbumTagBatchTrackOutcomeState::NotAttempted,
    );
    let partial_commit = committed_count > 0 && (failed_count > 0 || not_attempted_count > 0);

    LocalAlbumTagBatchApplyResult {
        album_id,
        applicable,
        blocked_reason,
        tracks,
        committed_count,
        unchanged_count,
        failed_count,
        not_attempted_count,
        partial_commit,
    }
}

fn count_batch_outcomes(
    tracks: &[LocalAlbumTagBatchTrackOutcome],
    expected: LocalAlbumTagBatchTrackOutcomeState,
) -> usize {
    tracks
        .iter()
        .filter(|track| track.state == expected)
        .count()
}

fn membership_changed() -> LocalAlbumTagBatchPreflightError {
    LocalAlbumTagBatchPreflightError::new(
        LocalAlbumTagBatchPreflightErrorCode::MembershipChanged,
        "As faixas do álbum mudaram desde a inspeção anterior.",
    )
}

fn track_kind(uri: &str) -> LocalAlbumTrackKind {
    if is_virtual_cue_track(uri) {
        LocalAlbumTrackKind::CueVirtual
    } else {
        LocalAlbumTrackKind::Physical
    }
}

impl LocalAlbumTagBatchUpdate {
    fn has_changes(&self) -> bool {
        !matches!(self.album, TagFieldUpdate::Unchanged)
            || !matches!(self.album_artist, TagFieldUpdate::Unchanged)
            || !matches!(self.date, TagFieldUpdate::Unchanged)
            || !matches!(self.genre, TagFieldUpdate::Unchanged)
    }

    fn for_track(&self, tags: &LocalTrackTags) -> LocalTrackTagUpdate {
        LocalTrackTagUpdate {
            album: minimize_text_update(&tags.album, &self.album),
            album_artist: minimize_text_update(&tags.album_artist, &self.album_artist),
            date: minimize_text_update(&tags.date, &self.date),
            genre: minimize_text_update(&tags.genre, &self.genre),
            ..LocalTrackTagUpdate::default()
        }
    }
}

fn minimize_text_update(
    current: &Option<String>,
    requested: &TagFieldUpdate<String>,
) -> TagFieldUpdate<String> {
    match requested {
        TagFieldUpdate::Unchanged => TagFieldUpdate::Unchanged,
        TagFieldUpdate::Set(value) if current.as_ref() == Some(value) => TagFieldUpdate::Unchanged,
        TagFieldUpdate::Set(value) => TagFieldUpdate::Set(value.clone()),
        TagFieldUpdate::Remove if current.is_none() => TagFieldUpdate::Unchanged,
        TagFieldUpdate::Remove => TagFieldUpdate::Remove,
    }
}

fn summarize_album_field(
    tracks: &[LocalAlbumTrackInspection],
    value: for<'a> fn(&'a LocalTrackTags) -> Option<&'a str>,
) -> LocalAlbumFieldSummary {
    let physical = tracks
        .iter()
        .filter(|track| track.kind == LocalAlbumTrackKind::Physical)
        .collect::<Vec<_>>();
    if physical.is_empty() {
        return LocalAlbumFieldSummary {
            state: LocalAlbumFieldState::NotApplicable,
            value: None,
        };
    }
    if physical.iter().any(|track| track.tags.is_none()) {
        return LocalAlbumFieldSummary {
            state: LocalAlbumFieldState::Indeterminate,
            value: None,
        };
    }

    let mut values = physical
        .into_iter()
        .filter_map(|track| track.tags.as_ref())
        .map(value);
    let first = values.next();
    let (state, common_value) = match first {
        Some(None) if values.all(|candidate| candidate.is_none()) => {
            (LocalAlbumFieldState::Absent, None)
        }
        Some(Some(expected)) if values.all(|candidate| candidate == Some(expected)) => {
            (LocalAlbumFieldState::Common, Some(expected.to_string()))
        }
        _ => (LocalAlbumFieldState::Mixed, None),
    };
    LocalAlbumFieldSummary {
        state,
        value: common_value,
    }
}

fn local_track_tags_from_file(tagged_file: &TaggedFile, revision: String) -> LocalTrackTags {
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());
    let file_type = tagged_file.file_type();
    let writable = file_type == FileType::Flac;

    LocalTrackTags {
        title: tag.and_then(|tag| tag.title().map(|value| value.into_owned())),
        artist: tag.and_then(|tag| tag.artist().map(|value| value.into_owned())),
        album: tag.and_then(|tag| tag.album().map(|value| value.into_owned())),
        album_artist: tag.and_then(|tag| tag.get_string(ItemKey::AlbumArtist).map(str::to_string)),
        genre: tag.and_then(|tag| tag.genre().map(|value| value.into_owned())),
        date: tag.and_then(|tag| {
            tag.get_string(ItemKey::RecordingDate)
                .or_else(|| tag.get_string(ItemKey::Year))
                .map(str::to_string)
        }),
        track_number: tag.and_then(|tag| tag.track()),
        track_total: tag.and_then(|tag| tag.track_total()),
        disc_number: tag.and_then(|tag| tag.disk()),
        disc_total: tag.and_then(|tag| tag.disk_total()),
        format: format_name(file_type).to_string(),
        readable: true,
        writable,
        editable: writable,
        unsupported_reason: (!writable).then(|| {
            "A escrita transacional ainda não foi validada para este formato.".to_string()
        }),
        revision,
    }
}

fn resolve_local_track_file(
    library_dir: &Path,
    relative_path: &str,
    local_folders: &[String],
) -> Result<ResolvedLocalFile, LocalTrackTagsError> {
    validate_relative_path(relative_path)?;
    let resolved = resolve_local_path(library_dir, relative_path, local_folders)
        .map_err(local_path_resolve_error)?;
    let selected_root = resolved.root;
    let canonical_file = resolved.physical_path;
    if !fs::metadata(&canonical_file)
        .map_err(|_| {
            LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::ReadFailed,
                "Não foi possível examinar o arquivo local.",
            )
        })?
        .is_file()
    {
        return Err(invalid_path());
    }

    let extension = canonical_file
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let file = File::open(&canonical_file).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::FileNotFound,
                "O arquivo local não foi encontrado.",
            )
        } else {
            LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::ReadFailed,
                "Não foi possível abrir o arquivo local para leitura.",
            )
        }
    })?;
    if !file
        .metadata()
        .map_err(|_| {
            LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::ReadFailed,
                "Não foi possível examinar o arquivo local aberto.",
            )
        })?
        .is_file()
    {
        return Err(invalid_path());
    }

    // Validate the opened descriptor too, closing the symlink-swap window between
    // canonicalization and open without ever exposing the physical path to IPC.
    let opened_path = fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|_| outside_roots())?;
    if !opened_path.starts_with(&selected_root) {
        return Err(outside_roots());
    }

    Ok(ResolvedLocalFile {
        file,
        extension,
        canonical_path: canonical_file,
        selected_root,
    })
}

fn local_path_resolve_error(error: LocalPathResolveError) -> LocalTrackTagsError {
    match error {
        LocalPathResolveError::InvalidPath => invalid_path(),
        LocalPathResolveError::SourceNotFound | LocalPathResolveError::OutsideRoot => {
            outside_roots()
        }
        LocalPathResolveError::NotFound => LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::FileNotFound,
            "O arquivo local não foi encontrado.",
        ),
        LocalPathResolveError::Unavailable(_) => LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::ReadFailed,
            "Não foi possível resolver o arquivo local.",
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransactionFailPoint {
    CreateTemp,
    Copy,
    Save,
    Validate,
    #[cfg(test)]
    ConcurrentModification,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreservedTagState {
    tag_type: TagType,
    items: Vec<TagItem>,
    pictures: Vec<Picture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AudioPropertiesSnapshot {
    duration: std::time::Duration,
    audio_bitrate: Option<u32>,
    sample_rate: Option<u32>,
    bit_depth: Option<u8>,
    channels: Option<u8>,
    channel_mask: Option<lofty::properties::ChannelMask>,
}

impl AudioPropertiesSnapshot {
    fn from_properties(properties: &FileProperties) -> Self {
        Self {
            duration: properties.duration(),
            audio_bitrate: properties.audio_bitrate(),
            sample_rate: properties.sample_rate(),
            bit_depth: properties.bit_depth(),
            channels: properties.channels(),
            channel_mask: properties.channel_mask(),
        }
    }
}

struct TempFileGuard {
    directory: File,
    original_name: CString,
    name: CString,
    file: Option<File>,
    committed: bool,
}

impl TempFileGuard {
    fn file_mut(&mut self) -> Result<&mut File, LocalTrackTagsError> {
        self.file.as_mut().ok_or_else(|| {
            transaction_error("O arquivo temporário da transação não está disponível.")
        })
    }

    fn mark_committed(&mut self) {
        self.committed = true;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        self.file.take();
        if !self.committed {
            unsafe {
                // SAFETY: the directory descriptor and NUL-terminated name belong to this guard.
                libc::unlinkat(self.directory.as_raw_fd(), self.name.as_ptr(), 0);
            }
        }
    }
}

pub fn update_local_track_tags(
    library_dir: &Path,
    relative_path: &str,
    local_folders: &[String],
    expected_revision: &str,
    patch: &LocalTrackTagUpdate,
) -> Result<LocalTrackTagWriteResult, LocalTrackTagsError> {
    update_local_track_tags_with_failure(
        library_dir,
        relative_path,
        local_folders,
        expected_revision,
        patch,
        None,
    )
}

fn update_local_track_tags_with_failure(
    library_dir: &Path,
    relative_path: &str,
    local_folders: &[String],
    expected_revision: &str,
    patch: &LocalTrackTagUpdate,
    fail_at: Option<TransactionFailPoint>,
) -> Result<LocalTrackTagWriteResult, LocalTrackTagsError> {
    if is_virtual_cue_track(relative_path) {
        return Err(LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::CueVirtual,
            "Faixas virtuais CUE não podem ter tags alteradas diretamente.",
        ));
    }
    validate_patch_values(patch)?;
    let _write_guard = TAG_WRITE_LOCK.lock().map_err(|_| {
        transaction_error("Outra transação de tags deixou o estado de escrita indisponível.")
    })?;
    let mut original = resolve_local_track_file(library_dir, relative_path, local_folders)?;
    let original_metadata = original.file.metadata().map_err(|_| {
        transaction_error("Não foi possível examinar o arquivo original para a transação.")
    })?;
    let initial_revision = revision_for_metadata(&original_metadata);
    if expected_revision != initial_revision {
        return Err(revision_conflict());
    }
    if original_metadata.nlink() != 1 {
        return Err(LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::MetadataPreservationFailed,
            "Arquivos com múltiplos hard links não podem ser substituídos com segurança.",
        ));
    }
    if original_metadata.mode() & 0o222 == 0 {
        return Err(LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::PermissionDenied,
            "O arquivo local não possui permissão de escrita.",
        ));
    }

    let mut tagged_file = read_tagged_file_for_update(
        &mut original.file,
        original.extension.as_deref().unwrap_or("unknown"),
    )?;
    if tagged_file.file_type() != FileType::Flac {
        return Err(LocalTrackTagsError::unsupported(
            LocalTrackTagsErrorCode::UnsupportedFormat,
            format_name(tagged_file.file_type()),
            "A escrita transacional está habilitada somente para FLAC nesta versão.",
        ));
    }
    ensure_primary_flac_tag(&mut tagged_file);
    validate_effective_numbers(tagged_file.primary_tag(), patch)?;

    if patch.is_unchanged() {
        return Ok(LocalTrackTagWriteResult {
            tags: local_track_tags_from_file(&tagged_file, initial_revision),
            committed: false,
        });
    }

    let touched_keys = patch.touched_keys();
    let preserved_tags = preserved_tag_state(&tagged_file, &touched_keys);
    let original_properties = AudioPropertiesSnapshot::from_properties(tagged_file.properties());
    apply_patch(
        tagged_file
            .primary_tag_mut()
            .ok_or_else(|| transaction_error("A tag FLAC primária não está disponível."))?,
        patch,
    );

    if fail_at == Some(TransactionFailPoint::CreateTemp) {
        return Err(transaction_error(
            "Falha simulada antes de criar o arquivo temporário.",
        ));
    }
    let mut temp = create_temp_file(&original)?;

    original.file.seek(SeekFrom::Start(0)).map_err(|_| {
        transaction_error("Não foi possível reposicionar o arquivo original para cópia.")
    })?;
    if fail_at == Some(TransactionFailPoint::Copy) {
        temp.file_mut()?
            .write_all(b"partial-copy")
            .map_err(|_| transaction_error("Falha simulada durante a cópia."))?;
        return Err(transaction_error("Falha simulada durante a cópia."));
    }
    io::copy(&mut original.file, temp.file_mut()?)
        .map_err(|_| transaction_error("Não foi possível copiar o arquivo original."))?;

    if fail_at == Some(TransactionFailPoint::Save) {
        return Err(transaction_error(
            "Falha simulada antes de salvar tags no temporário.",
        ));
    }
    temp.file_mut()?
        .seek(SeekFrom::Start(0))
        .map_err(|_| transaction_error("Não foi possível preparar o temporário para escrita."))?;
    tagged_file
        .save_to(temp.file_mut()?, WriteOptions::new())
        .map_err(|_| transaction_error("O Lofty não conseguiu salvar as tags no temporário."))?;
    temp.file_mut()?
        .flush()
        .map_err(|_| transaction_error("Não foi possível descarregar o arquivo temporário."))?;

    preserve_filesystem_metadata(&original.file, temp.file_mut()?, &original_metadata)?;
    temp.file_mut()?
        .sync_all()
        .map_err(|_| transaction_error("Não foi possível sincronizar o arquivo temporário."))?;

    if fail_at == Some(TransactionFailPoint::Validate) {
        return Err(transaction_error(
            "Falha simulada antes de validar o arquivo temporário.",
        ));
    }
    let mut validated_file = reopen_same_temp(&temp)?;
    let validated_tagged = read_tagged_file(&mut validated_file, true)?;
    if validated_tagged.file_type() != FileType::Flac {
        return Err(transaction_error(
            "O arquivo temporário deixou de ser reconhecido como FLAC.",
        ));
    }
    validate_patch_result(validated_tagged.primary_tag(), patch)?;
    if preserved_tag_state(&validated_tagged, &touched_keys) != preserved_tags {
        return Err(LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::MetadataPreservationFailed,
            "Tags não editadas ou artwork não foram preservados no temporário.",
        ));
    }
    if AudioPropertiesSnapshot::from_properties(validated_tagged.properties())
        != original_properties
    {
        return Err(transaction_error(
            "As propriedades de áudio mudaram durante a escrita das tags.",
        ));
    }

    #[cfg(test)]
    if fail_at == Some(TransactionFailPoint::ConcurrentModification) {
        let mut changed = OpenOptions::new()
            .append(true)
            .open(&original.canonical_path)
            .map_err(|_| transaction_error("Falha ao simular alteração concorrente."))?;
        changed
            .write_all(b"external-change")
            .map_err(|_| transaction_error("Falha ao simular alteração concorrente."))?;
        changed
            .sync_all()
            .map_err(|_| transaction_error("Falha ao simular alteração concorrente."))?;
    }

    revalidate_original(
        library_dir,
        relative_path,
        local_folders,
        &original,
        &initial_revision,
    )?;
    verify_original_in_directory(&temp.directory, &temp.original_name, &original.file)?;

    temp.directory.sync_all().map_err(|_| {
        transaction_error("Não foi possível sincronizar o temporário no diretório.")
    })?;

    let rename_result = unsafe {
        // SAFETY: both names are NUL-terminated and resolved relative to the same open directory.
        libc::renameat(
            temp.directory.as_raw_fd(),
            temp.name.as_ptr(),
            temp.directory.as_raw_fd(),
            temp.original_name.as_ptr(),
        )
    };
    if rename_result != 0 {
        return Err(transaction_error(
            "Não foi possível substituir atomicamente o arquivo original.",
        ));
    }
    temp.mark_committed();

    if let Err(_error) = temp.directory.sync_all() {
        return Err(LocalTrackTagsError::committed(
            LocalTrackTagsErrorCode::DurabilityFailed,
            "As tags foram substituídas, mas a durabilidade do diretório não pôde ser confirmada.",
        ));
    }

    let new_revision = revision_for_file(temp.file_mut()?)?;
    Ok(LocalTrackTagWriteResult {
        tags: local_track_tags_from_file(&validated_tagged, new_revision),
        committed: true,
    })
}

impl LocalTrackTagUpdate {
    fn is_unchanged(&self) -> bool {
        matches!(self.title, TagFieldUpdate::Unchanged)
            && matches!(self.artist, TagFieldUpdate::Unchanged)
            && matches!(self.album, TagFieldUpdate::Unchanged)
            && matches!(self.album_artist, TagFieldUpdate::Unchanged)
            && matches!(self.genre, TagFieldUpdate::Unchanged)
            && matches!(self.date, TagFieldUpdate::Unchanged)
            && matches!(self.track_number, TagFieldUpdate::Unchanged)
            && matches!(self.track_total, TagFieldUpdate::Unchanged)
            && matches!(self.disc_number, TagFieldUpdate::Unchanged)
            && matches!(self.disc_total, TagFieldUpdate::Unchanged)
    }

    fn touched_keys(&self) -> HashSet<ItemKey> {
        let mut keys = HashSet::new();
        add_touched_key(&mut keys, &self.title, ItemKey::TrackTitle);
        add_touched_key(&mut keys, &self.artist, ItemKey::TrackArtist);
        add_touched_key(&mut keys, &self.album, ItemKey::AlbumTitle);
        add_touched_key(&mut keys, &self.album_artist, ItemKey::AlbumArtist);
        add_touched_key(&mut keys, &self.genre, ItemKey::Genre);
        if !matches!(self.date, TagFieldUpdate::Unchanged) {
            keys.insert(ItemKey::RecordingDate);
            keys.insert(ItemKey::Year);
        }
        add_touched_key(&mut keys, &self.track_number, ItemKey::TrackNumber);
        add_touched_key(&mut keys, &self.track_total, ItemKey::TrackTotal);
        add_touched_key(&mut keys, &self.disc_number, ItemKey::DiscNumber);
        add_touched_key(&mut keys, &self.disc_total, ItemKey::DiscTotal);
        keys
    }
}

fn add_touched_key<T>(keys: &mut HashSet<ItemKey>, update: &TagFieldUpdate<T>, key: ItemKey) {
    if !matches!(update, TagFieldUpdate::Unchanged) {
        keys.insert(key);
    }
}

fn validate_patch_values(patch: &LocalTrackTagUpdate) -> Result<(), LocalTrackTagsError> {
    for (name, update, limit) in [
        ("title", &patch.title, MAX_TAG_STRING_BYTES),
        ("artist", &patch.artist, MAX_TAG_STRING_BYTES),
        ("album", &patch.album, MAX_TAG_STRING_BYTES),
        ("album_artist", &patch.album_artist, MAX_TAG_STRING_BYTES),
        ("genre", &patch.genre, MAX_TAG_STRING_BYTES),
        ("date", &patch.date, MAX_DATE_BYTES),
    ] {
        if let TagFieldUpdate::Set(value) = update {
            if value.len() > limit
                || value
                    .chars()
                    .any(|character| matches!(character, '\0' | '\r' | '\n'))
            {
                return Err(LocalTrackTagsError::new(
                    LocalTrackTagsErrorCode::InvalidTagUpdate,
                    &format!("O valor de {name} é inválido."),
                ));
            }
        }
    }
    for update in [
        &patch.track_number,
        &patch.track_total,
        &patch.disc_number,
        &patch.disc_total,
    ] {
        if matches!(update, TagFieldUpdate::Set(0)) {
            return Err(LocalTrackTagsError::new(
                LocalTrackTagsErrorCode::InvalidTagUpdate,
                "Números e totais de faixa/disco devem ser maiores que zero.",
            ));
        }
    }
    Ok(())
}

fn validate_effective_numbers(
    tag: Option<&Tag>,
    patch: &LocalTrackTagUpdate,
) -> Result<(), LocalTrackTagsError> {
    let track = effective_number(tag.and_then(|value| value.track()), &patch.track_number);
    let track_total = effective_number(
        tag.and_then(|value| value.track_total()),
        &patch.track_total,
    );
    let disc = effective_number(tag.and_then(|value| value.disk()), &patch.disc_number);
    let disc_total = effective_number(tag.and_then(|value| value.disk_total()), &patch.disc_total);
    if [track, track_total, disc, disc_total]
        .into_iter()
        .flatten()
        .any(|value| value == 0)
        || matches!((track, track_total), (Some(number), Some(total)) if total < number)
        || matches!((disc, disc_total), (Some(number), Some(total)) if total < number)
    {
        return Err(LocalTrackTagsError::new(
            LocalTrackTagsErrorCode::InvalidTagUpdate,
            "Os números e totais de faixa/disco são inconsistentes.",
        ));
    }
    Ok(())
}

fn effective_number(current: Option<u32>, update: &TagFieldUpdate<u32>) -> Option<u32> {
    match update {
        TagFieldUpdate::Unchanged => current,
        TagFieldUpdate::Set(value) => Some(*value),
        TagFieldUpdate::Remove => None,
    }
}

fn ensure_primary_flac_tag(tagged_file: &mut TaggedFile) {
    if tagged_file.primary_tag().is_none() {
        tagged_file.insert_tag(Tag::new(TagType::VorbisComments));
    }
}

fn apply_patch(tag: &mut Tag, patch: &LocalTrackTagUpdate) {
    apply_text_update(tag, ItemKey::TrackTitle, &patch.title);
    apply_text_update(tag, ItemKey::TrackArtist, &patch.artist);
    apply_text_update(tag, ItemKey::AlbumTitle, &patch.album);
    apply_text_update(tag, ItemKey::AlbumArtist, &patch.album_artist);
    apply_text_update(tag, ItemKey::Genre, &patch.genre);
    if !matches!(patch.date, TagFieldUpdate::Unchanged) {
        tag.remove_key(ItemKey::Year);
        apply_text_update(tag, ItemKey::RecordingDate, &patch.date);
    }
    apply_number_update(tag, ItemKey::TrackNumber, &patch.track_number);
    apply_number_update(tag, ItemKey::TrackTotal, &patch.track_total);
    apply_number_update(tag, ItemKey::DiscNumber, &patch.disc_number);
    apply_number_update(tag, ItemKey::DiscTotal, &patch.disc_total);
}

fn apply_text_update(tag: &mut Tag, key: ItemKey, update: &TagFieldUpdate<String>) {
    match update {
        TagFieldUpdate::Unchanged => {}
        TagFieldUpdate::Set(value) => {
            tag.insert_text(key, value.clone());
        }
        TagFieldUpdate::Remove => tag.remove_key(key),
    }
}

fn apply_number_update(tag: &mut Tag, key: ItemKey, update: &TagFieldUpdate<u32>) {
    match update {
        TagFieldUpdate::Unchanged => {}
        TagFieldUpdate::Set(value) => {
            tag.insert_text(key, value.to_string());
        }
        TagFieldUpdate::Remove => tag.remove_key(key),
    }
}

fn validate_patch_result(
    tag: Option<&Tag>,
    patch: &LocalTrackTagUpdate,
) -> Result<(), LocalTrackTagsError> {
    let tag = tag.ok_or_else(|| transaction_error("A tag salva não pôde ser reaberta."))?;
    validate_text_result(tag, ItemKey::TrackTitle, &patch.title)?;
    validate_text_result(tag, ItemKey::TrackArtist, &patch.artist)?;
    validate_text_result(tag, ItemKey::AlbumTitle, &patch.album)?;
    validate_text_result(tag, ItemKey::AlbumArtist, &patch.album_artist)?;
    validate_text_result(tag, ItemKey::Genre, &patch.genre)?;
    validate_text_result(tag, ItemKey::RecordingDate, &patch.date)?;
    validate_number_result(tag, ItemKey::TrackNumber, &patch.track_number)?;
    validate_number_result(tag, ItemKey::TrackTotal, &patch.track_total)?;
    validate_number_result(tag, ItemKey::DiscNumber, &patch.disc_number)?;
    validate_number_result(tag, ItemKey::DiscTotal, &patch.disc_total)?;
    Ok(())
}

fn validate_text_result(
    tag: &Tag,
    key: ItemKey,
    update: &TagFieldUpdate<String>,
) -> Result<(), LocalTrackTagsError> {
    let valid = match update {
        TagFieldUpdate::Unchanged => true,
        TagFieldUpdate::Set(value) => tag.get_string(key) == Some(value.as_str()),
        TagFieldUpdate::Remove => tag.get(key).is_none(),
    };
    valid.then_some(()).ok_or_else(|| {
        transaction_error("A validação das tags salvas não corresponde ao patch solicitado.")
    })
}

fn validate_number_result(
    tag: &Tag,
    key: ItemKey,
    update: &TagFieldUpdate<u32>,
) -> Result<(), LocalTrackTagsError> {
    let actual = tag
        .get_string(key)
        .and_then(|value| value.parse::<u32>().ok());
    let valid = match update {
        TagFieldUpdate::Unchanged => true,
        TagFieldUpdate::Set(value) => actual == Some(*value),
        TagFieldUpdate::Remove => tag.get(key).is_none(),
    };
    valid.then_some(()).ok_or_else(|| {
        transaction_error("A validação dos números salvos não corresponde ao patch solicitado.")
    })
}

fn preserved_tag_state(
    tagged_file: &TaggedFile,
    touched_keys: &HashSet<ItemKey>,
) -> Vec<PreservedTagState> {
    tagged_file
        .tags()
        .iter()
        .map(|tag| PreservedTagState {
            tag_type: tag.tag_type(),
            items: tag
                .items()
                .filter(|item| !touched_keys.contains(&item.key()))
                .cloned()
                .collect(),
            pictures: tag.pictures().to_vec(),
        })
        .collect()
}

fn read_tagged_file(
    file: &mut File,
    read_properties: bool,
) -> Result<TaggedFile, LocalTrackTagsError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        transaction_error("Não foi possível reposicionar o arquivo para leitura de tags.")
    })?;
    let options = ParseOptions::new()
        .read_properties(read_properties)
        .read_cover_art(true);
    Probe::new(BufReader::new(file))
        .options(options)
        .guess_file_type()
        .map_err(|_| transaction_error("Não foi possível identificar o formato do arquivo."))?
        .read()
        .map_err(|_| transaction_error("Não foi possível ler o arquivo para a transação."))
}

fn read_tagged_file_for_update(
    file: &mut File,
    format_hint: &str,
) -> Result<TaggedFile, LocalTrackTagsError> {
    file.seek(SeekFrom::Start(0)).map_err(|_| {
        transaction_error("Não foi possível reposicionar o arquivo para leitura de tags.")
    })?;
    let options = ParseOptions::new()
        .read_properties(true)
        .read_cover_art(true);
    let probe = Probe::new(BufReader::new(file))
        .options(options)
        .guess_file_type()
        .map_err(|_| transaction_error("Não foi possível identificar o formato do arquivo."))?;
    if probe.file_type().is_none() {
        return Err(LocalTrackTagsError::unsupported(
            LocalTrackTagsErrorCode::UnsupportedFormat,
            format_hint,
            "O formato do arquivo não é suportado para escrita de tags.",
        ));
    }
    probe
        .read()
        .map_err(|_| transaction_error("Não foi possível ler o arquivo para a transação."))
}

fn create_temp_file(original: &ResolvedLocalFile) -> Result<TempFileGuard, LocalTrackTagsError> {
    let parent = original.canonical_path.parent().ok_or_else(|| {
        transaction_error("Não foi possível localizar o diretório físico do arquivo.")
    })?;
    let original_name = original
        .canonical_path
        .file_name()
        .ok_or_else(|| transaction_error("O nome do arquivo original é inválido."))?;
    let original_name = CString::new(original_name.as_bytes())
        .map_err(|_| transaction_error("O nome do arquivo original é inválido."))?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)
        .map_err(|_| transaction_error("Não foi possível abrir o diretório da transação."))?;
    verify_original_in_directory(&directory, &original_name, &original.file)?;

    for _ in 0..16 {
        let id = NEXT_TEMP_ID.fetch_add(1, AtomicOrdering::Relaxed);
        let mut random = [0_u8; 16];
        let read = unsafe {
            // SAFETY: `random` is a valid writable buffer for exactly its own length.
            libc::getrandom(random.as_mut_ptr().cast::<libc::c_void>(), random.len(), 0)
        };
        if read != random.len() as isize {
            return Err(transaction_error(
                "Não foi possível obter aleatoriedade para o arquivo temporário.",
            ));
        }
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let name = CString::new(format!(
            ".sonante-tags-{}-{id}-{suffix}.tmp",
            std::process::id()
        ))
        .map_err(|_| transaction_error("O nome temporário gerado é inválido."))?;
        let fd = unsafe {
            // SAFETY: directory and name are valid; O_EXCL prevents replacing an existing entry.
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd >= 0 {
            let file = unsafe {
                // SAFETY: `openat` returned a new owned file descriptor.
                File::from_raw_fd(fd)
            };
            return Ok(TempFileGuard {
                directory,
                original_name,
                name,
                file: Some(file),
                committed: false,
            });
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EEXIST) => continue,
            _ => {
                return Err(transaction_error(
                    "Não foi possível criar o arquivo temporário no diretório do original.",
                ))
            }
        }
    }
    Err(transaction_error(
        "Não foi possível reservar um nome temporário seguro.",
    ))
}

fn reopen_same_temp(temp: &TempFileGuard) -> Result<File, LocalTrackTagsError> {
    let fd = unsafe {
        // SAFETY: directory and temporary name remain owned by the transaction guard.
        libc::openat(
            temp.directory.as_raw_fd(),
            temp.name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(transaction_error(
            "Não foi possível reabrir o arquivo temporário.",
        ));
    }
    let reopened = unsafe {
        // SAFETY: `openat` returned a new owned file descriptor.
        File::from_raw_fd(fd)
    };
    let expected = temp
        .file
        .as_ref()
        .ok_or_else(|| transaction_error("O descritor temporário não está disponível."))?
        .metadata()
        .map_err(|_| transaction_error("Não foi possível examinar o temporário aberto."))?;
    let actual = reopened
        .metadata()
        .map_err(|_| transaction_error("Não foi possível examinar o temporário reaberto."))?;
    if !actual.is_file() || actual.dev() != expected.dev() || actual.ino() != expected.ino() {
        return Err(transaction_error(
            "O arquivo temporário mudou antes da validação.",
        ));
    }
    Ok(reopened)
}

fn verify_original_in_directory(
    directory: &File,
    original_name: &CString,
    original: &File,
) -> Result<(), LocalTrackTagsError> {
    let fd = unsafe {
        // SAFETY: directory and name are valid; O_NOFOLLOW rejects an unexpected final symlink.
        libc::openat(
            directory.as_raw_fd(),
            original_name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(revision_conflict());
    }
    let opened = unsafe {
        // SAFETY: `openat` returned a new owned file descriptor.
        File::from_raw_fd(fd)
    };
    let expected = original.metadata().map_err(|_| revision_conflict())?;
    let actual = opened.metadata().map_err(|_| revision_conflict())?;
    if !actual.is_file() || actual.dev() != expected.dev() || actual.ino() != expected.ino() {
        return Err(revision_conflict());
    }
    Ok(())
}

fn revalidate_original(
    library_dir: &Path,
    relative_path: &str,
    local_folders: &[String],
    original: &ResolvedLocalFile,
    initial_revision: &str,
) -> Result<(), LocalTrackTagsError> {
    if revision_for_file(&original.file)? != initial_revision {
        return Err(revision_conflict());
    }
    let fresh = resolve_local_track_file(library_dir, relative_path, local_folders)
        .map_err(|_| revision_conflict())?;
    if fresh.canonical_path != original.canonical_path
        || fresh.selected_root != original.selected_root
        || revision_for_file(&fresh.file)? != initial_revision
    {
        return Err(revision_conflict());
    }
    Ok(())
}

fn revision_for_file(file: &File) -> Result<String, LocalTrackTagsError> {
    file.metadata()
        .map(|metadata| revision_for_metadata(&metadata))
        .map_err(|_| transaction_error("Não foi possível calcular a revisão do arquivo local."))
}

fn revision_for_metadata(metadata: &Metadata) -> String {
    let mut bytes = b"sonante-local-tags-v1\0".to_vec();
    for value in [
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    format!("v1.{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn preserve_filesystem_metadata(
    original: &File,
    temp: &File,
    original_metadata: &Metadata,
) -> Result<(), LocalTrackTagsError> {
    let temp_metadata = temp.metadata().map_err(|_| metadata_preservation_error())?;
    if temp_metadata.uid() != original_metadata.uid()
        || temp_metadata.gid() != original_metadata.gid()
    {
        let result = unsafe {
            // SAFETY: `temp` owns a valid file descriptor; uid/gid come from kernel metadata.
            libc::fchown(
                temp.as_raw_fd(),
                original_metadata.uid(),
                original_metadata.gid(),
            )
        };
        if result != 0 {
            return Err(metadata_preservation_error());
        }
    }
    temp.set_permissions(fs::Permissions::from_mode(
        original_metadata.mode() & 0o7777,
    ))
    .map_err(|_| metadata_preservation_error())?;
    copy_xattrs(original.as_raw_fd(), temp.as_raw_fd())?;

    let final_metadata = temp.metadata().map_err(|_| metadata_preservation_error())?;
    if final_metadata.uid() != original_metadata.uid()
        || final_metadata.gid() != original_metadata.gid()
        || final_metadata.mode() & 0o7777 != original_metadata.mode() & 0o7777
    {
        return Err(metadata_preservation_error());
    }
    Ok(())
}

fn copy_xattrs(source_fd: RawFd, destination_fd: RawFd) -> Result<(), LocalTrackTagsError> {
    let source = read_xattrs(source_fd).map_err(|_| metadata_preservation_error())?;
    let destination = read_xattrs(destination_fd).map_err(|_| metadata_preservation_error())?;
    let source_names = source
        .iter()
        .map(|(name, _)| name.as_slice())
        .collect::<HashSet<_>>();

    for (name, _) in &destination {
        if !source_names.contains(name.as_slice()) {
            let name = CString::new(name.as_slice()).map_err(|_| metadata_preservation_error())?;
            let result = unsafe {
                // SAFETY: fd is valid and `name` is a NUL-terminated xattr name.
                libc::fremovexattr(destination_fd, name.as_ptr())
            };
            if result != 0 {
                return Err(metadata_preservation_error());
            }
        }
    }
    for (name, value) in &source {
        let current = destination
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_slice());
        if current == Some(value.as_slice()) {
            continue;
        }
        let name = CString::new(name.as_slice()).map_err(|_| metadata_preservation_error())?;
        let value_ptr = if value.is_empty() {
            std::ptr::null()
        } else {
            value.as_ptr().cast::<libc::c_void>()
        };
        let result = unsafe {
            // SAFETY: fd and name are valid; value points to `value.len()` readable bytes.
            libc::fsetxattr(destination_fd, name.as_ptr(), value_ptr, value.len(), 0)
        };
        if result != 0 {
            return Err(metadata_preservation_error());
        }
    }
    if read_xattrs(destination_fd).map_err(|_| metadata_preservation_error())? != source {
        return Err(metadata_preservation_error());
    }
    Ok(())
}

fn read_xattrs(fd: RawFd) -> io::Result<Vec<(Vec<u8>, Vec<u8>)>> {
    for _ in 0..3 {
        let size = unsafe {
            // SAFETY: null buffer with size zero asks the kernel for the required length.
            libc::flistxattr(fd, std::ptr::null_mut(), 0)
        };
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let size = usize::try_from(size).map_err(|_| io::Error::other("xattr size overflow"))?;
        if size > MAX_XATTR_BYTES {
            return Err(io::Error::other("xattr list too large"));
        }
        let mut names = vec![0_u8; size];
        let actual = unsafe {
            // SAFETY: `names` is writable for exactly `names.len()` bytes.
            libc::flistxattr(fd, names.as_mut_ptr().cast::<libc::c_char>(), names.len())
        };
        if actual < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(error);
        }
        names.truncate(actual as usize);
        let mut result = Vec::new();
        for name in names
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
        {
            let c_name = CString::new(name).map_err(|_| io::Error::other("invalid xattr name"))?;
            let value = read_xattr_value(fd, &c_name)?;
            result.push((name.to_vec(), value));
        }
        result.sort_by(|left, right| left.0.cmp(&right.0));
        let total = result
            .iter()
            .try_fold(0_usize, |total, (name, value)| {
                total.checked_add(name.len())?.checked_add(value.len())
            })
            .ok_or_else(|| io::Error::other("xattr size overflow"))?;
        if total > MAX_XATTR_BYTES {
            return Err(io::Error::other("xattr data too large"));
        }
        return Ok(result);
    }
    Err(io::Error::other("xattr list changed repeatedly"))
}

fn read_xattr_value(fd: RawFd, name: &CString) -> io::Result<Vec<u8>> {
    for _ in 0..3 {
        let size = unsafe {
            // SAFETY: null buffer with size zero asks the kernel for the required length.
            libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0)
        };
        if size < 0 {
            return Err(io::Error::last_os_error());
        }
        let size = usize::try_from(size).map_err(|_| io::Error::other("xattr size overflow"))?;
        if size > MAX_XATTR_BYTES {
            return Err(io::Error::other("xattr value too large"));
        }
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut value = vec![0_u8; size];
        let actual = unsafe {
            // SAFETY: `value` is writable for exactly `value.len()` bytes.
            libc::fgetxattr(
                fd,
                name.as_ptr(),
                value.as_mut_ptr().cast::<libc::c_void>(),
                value.len(),
            )
        };
        if actual < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ERANGE) {
                continue;
            }
            return Err(error);
        }
        value.truncate(actual as usize);
        return Ok(value);
    }
    Err(io::Error::other("xattr value changed repeatedly"))
}

fn transaction_error(message: &str) -> LocalTrackTagsError {
    LocalTrackTagsError::new(LocalTrackTagsErrorCode::TransactionFailed, message)
}

fn revision_conflict() -> LocalTrackTagsError {
    LocalTrackTagsError::new(
        LocalTrackTagsErrorCode::RevisionConflict,
        "O arquivo local mudou desde a leitura das tags.",
    )
}

fn metadata_preservation_error() -> LocalTrackTagsError {
    LocalTrackTagsError::new(
        LocalTrackTagsErrorCode::MetadataPreservationFailed,
        "Não foi possível preservar permissões, ownership, xattrs e ACLs do arquivo.",
    )
}

fn validate_relative_path(relative_path: &str) -> Result<PathBuf, LocalTrackTagsError> {
    let lowercase_path = relative_path.to_ascii_lowercase();
    if relative_path.is_empty()
        || relative_path
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n'))
        || relative_path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        || relative_path.contains("://")
        || lowercase_path.starts_with("http:")
        || lowercase_path.starts_with("https:")
        || lowercase_path.contains("x-plex-token")
    {
        return Err(invalid_path());
    }
    let path = Path::new(relative_path);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_path());
    }
    Ok(path.to_path_buf())
}

fn is_virtual_cue_track(relative_path: &str) -> bool {
    let path = Path::new(relative_path);
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

fn invalid_path() -> LocalTrackTagsError {
    LocalTrackTagsError::new(
        LocalTrackTagsErrorCode::InvalidLocalPath,
        "O caminho local informado é inválido.",
    )
}

fn outside_roots() -> LocalTrackTagsError {
    LocalTrackTagsError::new(
        LocalTrackTagsErrorCode::OutsideConfiguredRoots,
        "O arquivo não pertence às pastas locais configuradas.",
    )
}

fn format_name(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Aac => "aac",
        FileType::Aiff => "aiff",
        FileType::Ape => "ape",
        FileType::Flac => "flac",
        FileType::Mpeg => "mpeg",
        FileType::Mp4 => "mp4",
        FileType::Mpc => "musepack",
        FileType::Opus => "opus",
        FileType::Vorbis => "ogg_vorbis",
        FileType::Speex => "speex",
        FileType::Wav => "wav",
        FileType::WavPack => "wavpack",
        FileType::Custom(_) => "custom",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::picture::{MimeType, PictureType};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        base: PathBuf,
        library: PathBuf,
        root: PathBuf,
    }

    impl Fixture {
        fn new(root_name: &str) -> Self {
            let id = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let base = std::env::temp_dir().join(format!(
                "sonante-tag-inspection-{}-{id}",
                std::process::id()
            ));
            let library = base.join("library");
            let root = base.join(root_name);
            fs::create_dir_all(&library).unwrap();
            fs::create_dir_all(&root).unwrap();
            symlink(&root, library.join(root_name)).unwrap();
            Self {
                base,
                library,
                root,
            }
        }

        fn folders(&self) -> Vec<String> {
            vec![self.root.to_string_lossy().into_owned()]
        }

        fn inspect(&self, path: &str) -> Result<LocalTrackTags, LocalTrackTagsError> {
            inspect_local_track_tags(&self.library, path, &self.folders())
        }

        fn publish_managed(&self) {
            let staging = self.base.join("library.next");
            crate::local_media::mirror::build_filtered_staging(
                &staging,
                &self.library,
                std::slice::from_ref(&self.root),
                "tags-managed-test",
            )
            .unwrap();
            fs::remove_file(self.library.join(self.root.file_name().unwrap())).unwrap();
            fs::remove_dir(&self.library).unwrap();
            fs::rename(staging, &self.library).unwrap();
        }

        fn update(
            &self,
            path: &str,
            revision: &str,
            patch: &LocalTrackTagUpdate,
        ) -> Result<LocalTrackTags, LocalTrackTagsError> {
            update_local_track_tags(&self.library, path, &self.folders(), revision, patch)
                .map(|result| result.tags)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base).unwrap();
        }
    }

    fn expected_physical(fixture: &Fixture, uri: &str) -> LocalAlbumTagBatchExpectedTrack {
        LocalAlbumTagBatchExpectedTrack {
            uri: uri.to_string(),
            kind: LocalAlbumTrackKind::Physical,
            revision: Some(fixture.inspect(uri).unwrap().revision),
        }
    }

    fn expected_cue(uri: &str) -> LocalAlbumTagBatchExpectedTrack {
        LocalAlbumTagBatchExpectedTrack {
            uri: uri.to_string(),
            kind: LocalAlbumTrackKind::CueVirtual,
            revision: None,
        }
    }

    fn batch_request(
        tracks: Vec<LocalAlbumTagBatchExpectedTrack>,
        update: LocalAlbumTagBatchUpdate,
    ) -> LocalAlbumTagBatchPreflightRequest {
        LocalAlbumTagBatchPreflightRequest {
            album_id: "album-id".to_string(),
            tracks,
            update,
        }
    }

    fn write_minimal_flac(path: &Path, comments: &[&str]) {
        let mut bytes = b"fLaC".to_vec();
        let stream_info = {
            let mut value = Vec::with_capacity(34);
            value.extend_from_slice(&4096_u16.to_be_bytes());
            value.extend_from_slice(&4096_u16.to_be_bytes());
            value.extend_from_slice(&[0; 6]);
            let packed = (44_100_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | 44_100_u64;
            value.extend_from_slice(&packed.to_be_bytes());
            value.extend_from_slice(&[0; 16]);
            value
        };
        bytes.push(if comments.is_empty() { 0x80 } else { 0x00 });
        bytes.extend_from_slice(&[0, 0, stream_info.len() as u8]);
        bytes.extend_from_slice(&stream_info);
        if !comments.is_empty() {
            let vendor = b"Sonante test";
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
            bytes.extend_from_slice(&block);
        }
        bytes.extend_from_slice(b"\xff\xf8SONANTE_TEST_AUDIO_PAYLOAD");
        let mut file = File::create(path).unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn flac_audio_payload(path: &Path) -> Vec<u8> {
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        let mut offset = 4;
        loop {
            let header = bytes[offset];
            let length = ((bytes[offset + 1] as usize) << 16)
                | ((bytes[offset + 2] as usize) << 8)
                | bytes[offset + 3] as usize;
            offset += 4 + length;
            if header & 0x80 != 0 {
                break;
            }
        }
        bytes[offset..].to_vec()
    }

    fn add_preserved_metadata(path: &Path) -> (Vec<TagItem>, Vec<Picture>) {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut tagged = read_tagged_file(&mut file, true).unwrap();
        let tag = tagged.primary_tag_mut().unwrap();
        tag.insert_text(ItemKey::Composer, "Compositora".to_string());
        tag.insert_text(ItemKey::ReplayGainTrackGain, "-4.50 dB".to_string());
        tag.insert_text(
            ItemKey::MusicBrainzRecordingId,
            "00000000-0000-0000-0000-000000000001".to_string(),
        );
        tag.insert_text(ItemKey::Comment, "Comentário preservado".to_string());
        tag.push_picture(
            Picture::unchecked(vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .description("Capa mínima")
                .build(),
        );
        tagged.save_to(&mut file, WriteOptions::new()).unwrap();
        file.sync_all().unwrap();

        file.seek(SeekFrom::Start(0)).unwrap();
        let tagged = read_tagged_file(&mut file, true).unwrap();
        let tag = tagged.primary_tag().unwrap();
        (tag.items().cloned().collect(), tag.pictures().to_vec())
    }

    fn temp_files_below(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".sonante-tags-")
                {
                    found.push(entry.path());
                }
            }
        }
        found
    }

    fn set_user_xattr(file: &File, name: &str, value: &[u8]) {
        let name = CString::new(name).unwrap();
        let result = unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast::<libc::c_void>(),
                value.len(),
                0,
            )
        };
        assert_eq!(result, 0, "{}", io::Error::last_os_error());
    }

    #[test]
    fn reads_tagged_flac_from_valid_path_with_spaces_and_unicode() {
        let fixture = Fixture::new("Músicas locais");
        let album = fixture.root.join("Artista/Álbum com espaços");
        fs::create_dir_all(&album).unwrap();
        let file = album.join("Faixa 01.flac");
        write_minimal_flac(
            &file,
            &[
                "TITLE=Canção",
                "ARTIST=Artista",
                "ALBUM=Álbum",
                "ALBUMARTIST=Artista do álbum",
                "GENRE=MPB",
                "DATE=2026-10-06",
                "TRACKNUMBER=2",
                "TRACKTOTAL=12",
                "DISCNUMBER=1",
                "DISCTOTAL=2",
            ],
        );

        let tags = fixture
            .inspect("Músicas locais/Artista/Álbum com espaços/Faixa 01.flac")
            .unwrap();
        assert_eq!(tags.format, "flac");
        assert!(tags.editable);
        assert_eq!(tags.title.as_deref(), Some("Canção"));
        assert_eq!(tags.artist.as_deref(), Some("Artista"));
        assert_eq!(tags.album.as_deref(), Some("Álbum"));
        assert_eq!(tags.album_artist.as_deref(), Some("Artista do álbum"));
        assert_eq!(tags.genre.as_deref(), Some("MPB"));
        assert_eq!(tags.date.as_deref(), Some("2026-10-06"));
        assert_eq!(tags.track_number, Some(2));
        assert_eq!(tags.track_total, Some(12));
        assert_eq!(tags.disc_number, Some(1));
        assert_eq!(tags.disc_total, Some(2));
    }

    #[test]
    fn missing_tags_remain_none() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("untagged.flac");
        write_minimal_flac(&file, &[]);

        let tags = fixture.inspect("Music/untagged.flac").unwrap();
        assert_eq!(tags.title, None);
        assert_eq!(tags.artist, None);
        assert_eq!(tags.album, None);
        assert_eq!(tags.album_artist, None);
        assert_eq!(tags.genre, None);
        assert_eq!(tags.date, None);
        assert_eq!(tags.track_number, None);
        assert_eq!(tags.track_total, None);
        assert_eq!(tags.disc_number, None);
        assert_eq!(tags.disc_total, None);
    }

    #[test]
    fn rejects_absolute_parent_and_remote_paths() {
        let fixture = Fixture::new("Music");
        for path in [
            "/Music/song.flac",
            "../Music/song.flac",
            "Music/../song.flac",
            "Music/./song.flac",
            "Music//song.flac",
            "https://example.invalid/song.flac?X-Plex-Token=secret",
            "HTTP:remote-track",
            "Music/X-Plex-Token=secret.flac",
        ] {
            assert_eq!(
                fixture.inspect(path).unwrap_err().code,
                LocalTrackTagsErrorCode::InvalidLocalPath,
                "{path}"
            );
        }
    }

    #[test]
    fn rejects_unconfigured_root_and_symlink_escape() {
        let fixture = Fixture::new("Music");
        let other = fixture.base.join("Other");
        let outside = fixture.base.join("Outside");
        fs::create_dir_all(&other).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("song.flac"), b"not audio").unwrap();
        symlink(&other, fixture.library.join("Other")).unwrap();
        symlink(&outside, fixture.root.join("Escape")).unwrap();

        assert_eq!(
            fixture.inspect("Other/song.flac").unwrap_err().code,
            LocalTrackTagsErrorCode::OutsideConfiguredRoots
        );
        assert_eq!(
            fixture.inspect("Music/Escape/song.flac").unwrap_err().code,
            LocalTrackTagsErrorCode::OutsideConfiguredRoots
        );
    }

    #[test]
    fn reports_missing_file_without_exposing_physical_path() {
        let fixture = Fixture::new("Music");
        let error = fixture.inspect("Music/missing.flac").unwrap_err();
        assert_eq!(error.code, LocalTrackTagsErrorCode::FileNotFound);
        assert!(!error
            .message
            .contains(&fixture.base.to_string_lossy().as_ref()));
    }

    #[test]
    fn rejects_virtual_cue_without_opening_an_invented_path() {
        let fixture = Fixture::new("Music");
        let error = fixture
            .inspect("Music/Artist/Album/Album.cue/track0001")
            .unwrap_err();
        assert_eq!(error.code, LocalTrackTagsErrorCode::CueVirtual);
        assert_eq!(error.format.as_deref(), Some("cue_virtual"));
        assert!(!error.editable);
    }

    #[test]
    fn managed_topology_preserves_inspection_transaction_and_revision_safety() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("managed.flac");
        write_minimal_flac(&file, &["TITLE=Before"]);
        fixture.publish_managed();

        let before = fixture.inspect("Music/managed.flac").unwrap();
        let updated = fixture
            .update(
                "Music/managed.flac",
                &before.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("After".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.title.as_deref(), Some("After"));
        assert_ne!(updated.revision, before.revision);

        let stale = fixture
            .update(
                "Music/managed.flac",
                &before.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("Stale".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(stale.code, LocalTrackTagsErrorCode::RevisionConflict);
    }

    #[test]
    fn managed_tags_fail_closed_for_unknown_source_config_mismatch_and_escape() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("track.flac");
        let outside = fixture.base.join("Outside");
        write_minimal_flac(&file, &["TITLE=Before"]);
        fs::create_dir(&outside).unwrap();
        write_minimal_flac(&outside.join("escape.flac"), &["TITLE=Outside"]);
        symlink(&outside, fixture.root.join("Escape")).unwrap();
        fixture.publish_managed();

        assert_eq!(
            fixture.inspect("Unknown/track.flac").unwrap_err().code,
            LocalTrackTagsErrorCode::OutsideConfiguredRoots
        );
        assert_eq!(
            fixture
                .inspect("Music/Escape/escape.flac")
                .unwrap_err()
                .code,
            LocalTrackTagsErrorCode::OutsideConfiguredRoots
        );

        let added = fixture.base.join("Added");
        fs::create_dir(&added).unwrap();
        let mismatched = vec![
            fixture.root.to_string_lossy().into_owned(),
            added.to_string_lossy().into_owned(),
        ];
        assert_eq!(
            inspect_local_track_tags(&fixture.library, "Music/track.flac", &mismatched)
                .unwrap_err()
                .code,
            LocalTrackTagsErrorCode::ReadFailed
        );
    }

    #[test]
    fn reports_unsupported_format_and_dsf_explicitly() {
        let fixture = Fixture::new("Music");
        fs::write(fixture.root.join("notes.txt"), b"not an audio file").unwrap();
        fs::write(fixture.root.join("track.dsf"), b"DSD not parsed here").unwrap();

        let unknown = fixture.inspect("Music/notes.txt").unwrap_err();
        assert_eq!(unknown.code, LocalTrackTagsErrorCode::UnsupportedFormat);
        assert_eq!(unknown.format.as_deref(), Some("txt"));
        let dsf = fixture.inspect("Music/track.dsf").unwrap_err();
        assert_eq!(dsf.code, LocalTrackTagsErrorCode::UnsupportedFormat);
        assert_eq!(dsf.format.as_deref(), Some("dsf"));
    }

    #[test]
    fn malformed_supported_file_is_a_read_failure() {
        let fixture = Fixture::new("Music");
        fs::write(fixture.root.join("broken.flac"), b"fLaC").unwrap();
        assert_eq!(
            fixture.inspect("Music/broken.flac").unwrap_err().code,
            LocalTrackTagsErrorCode::ReadFailed
        );
    }

    #[test]
    fn inspection_does_not_modify_a_read_only_audio_file() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("readonly.flac");
        write_minimal_flac(&file, &["TITLE=Read only"]);
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(&file, permissions).unwrap();
        let before = fs::metadata(&file).unwrap();

        assert_eq!(
            fixture
                .inspect("Music/readonly.flac")
                .unwrap()
                .title
                .as_deref(),
            Some("Read only")
        );

        let after = fs::metadata(&file).unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        assert_eq!(after.permissions().mode(), before.permissions().mode());
    }

    #[test]
    fn album_field_summaries_cover_common_absent_and_mixed_values() {
        let fixture = Fixture::new("Músicas locais");
        let album = fixture.root.join("Artista/Álbum com espaços");
        fs::create_dir_all(&album).unwrap();
        let first = album.join("Faixa 01.flac");
        let second = album.join("Faixa 02.flac");
        write_minimal_flac(
            &first,
            &[
                "ALBUM=Álbum exato",
                "ALBUMARTIST=Artista",
                "DATE=2020",
                "GENRE=MPB",
            ],
        );
        write_minimal_flac(
            &second,
            &["ALBUM=Álbum exato", "ALBUMARTIST=Artista", "DATE=2021"],
        );
        let before = fs::read(&first).unwrap();

        let inspection = inspect_local_album_tags(
            &fixture.library,
            "album-id",
            vec![
                "Músicas locais/Artista/Álbum com espaços/Faixa 01.flac".to_string(),
                "Músicas locais/Artista/Álbum com espaços/Faixa 02.flac".to_string(),
            ],
            &fixture.folders(),
        );

        assert!(inspection.inspection_complete);
        assert_eq!(inspection.physical_track_count, 2);
        assert_eq!(inspection.cue_virtual_count, 0);
        assert_eq!(inspection.fields.album.state, LocalAlbumFieldState::Common);
        assert_eq!(
            inspection.fields.album.value.as_deref(),
            Some("Álbum exato")
        );
        assert_eq!(
            inspection.fields.album_artist.state,
            LocalAlbumFieldState::Common
        );
        assert_eq!(inspection.fields.date.state, LocalAlbumFieldState::Mixed);
        assert_eq!(inspection.fields.genre.state, LocalAlbumFieldState::Mixed);
        assert_eq!(fs::read(&first).unwrap(), before);

        let untagged_a = album.join("Sem tags 1.flac");
        let untagged_b = album.join("Sem tags 2.flac");
        write_minimal_flac(&untagged_a, &[]);
        write_minimal_flac(&untagged_b, &[]);
        let absent = inspect_local_album_tags(
            &fixture.library,
            "untagged-album",
            vec![
                "Músicas locais/Artista/Álbum com espaços/Sem tags 1.flac".to_string(),
                "Músicas locais/Artista/Álbum com espaços/Sem tags 2.flac".to_string(),
            ],
            &fixture.folders(),
        );
        assert_eq!(absent.fields.album.state, LocalAlbumFieldState::Absent);
        assert_eq!(
            absent.fields.album_artist.state,
            LocalAlbumFieldState::Absent
        );
        assert_eq!(absent.fields.date.state, LocalAlbumFieldState::Absent);
        assert_eq!(absent.fields.genre.state, LocalAlbumFieldState::Absent);
    }

    #[test]
    fn album_field_summary_is_indeterminate_when_a_physical_track_fails() {
        let fixture = Fixture::new("Music");
        write_minimal_flac(&fixture.root.join("present.flac"), &["ALBUM=Album"]);

        let inspection = inspect_local_album_tags(
            &fixture.library,
            "album-id",
            vec![
                "Music/present.flac".to_string(),
                "Music/missing.flac".to_string(),
            ],
            &fixture.folders(),
        );

        assert!(!inspection.inspection_complete);
        assert_eq!(inspection.physical_track_count, 2);
        assert_eq!(
            inspection.fields.album.state,
            LocalAlbumFieldState::Indeterminate
        );
        assert_eq!(
            inspection.tracks[1].issue.as_ref().map(|issue| &issue.code),
            Some(&LocalTrackTagsErrorCode::FileNotFound)
        );
    }

    #[test]
    fn cue_only_album_is_logical_read_only_and_not_applicable() {
        let fixture = Fixture::new("Music");
        let inspection = inspect_local_album_tags(
            &fixture.library,
            "cue-album",
            vec![
                "Music/Artist/Album/image.cue/track0001".to_string(),
                "Music/Artist/Album/image.cue/track0002".to_string(),
            ],
            &fixture.folders(),
        );

        assert!(inspection.inspection_complete);
        assert_eq!(inspection.physical_track_count, 0);
        assert_eq!(inspection.cue_virtual_count, 2);
        assert_eq!(
            inspection.fields.album.state,
            LocalAlbumFieldState::NotApplicable
        );
        assert!(inspection.tracks.iter().all(|track| {
            track.kind == LocalAlbumTrackKind::CueVirtual
                && track.tags.is_none()
                && track.issue.as_ref().is_some_and(|issue| {
                    issue.code == LocalTrackTagsErrorCode::CueVirtual
                        && issue.format.as_deref() == Some("cue_virtual")
                })
        }));
    }

    #[test]
    fn batch_preflight_accepts_identical_snapshot_without_modifying_files() {
        let fixture = Fixture::new("Músicas locais");
        let file = fixture.root.join("Álbum com espaços/Faixa única.flac");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        write_minimal_flac(&file, &["ALBUM=Álbum", "GENRE=MPB"]);
        let uri = "Músicas locais/Álbum com espaços/Faixa única.flac";
        let before_bytes = fs::read(&file).unwrap();
        let before_modified = fs::metadata(&file).unwrap().modified().unwrap();

        let plan = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_physical(&fixture, uri)],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![uri.to_string()],
            &fixture.folders(),
        )
        .unwrap();

        assert!(plan.applicable);
        assert_eq!(plan.blocked_reason, None);
        assert_eq!(plan.change_count, 0);
        assert_eq!(plan.unchanged_count, 1);
        assert!(!plan.tracks[0].needs_write);
        assert_eq!(fs::read(&file).unwrap(), before_bytes);
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before_modified);
    }

    #[test]
    fn batch_preflight_rejects_membership_changes_kinds_and_duplicates() {
        let fixture = Fixture::new("Music");
        for name in ["one.flac", "two.flac"] {
            write_minimal_flac(&fixture.root.join(name), &[]);
        }
        let one = "Music/one.flac";
        let two = "Music/two.flac";
        let expected_one = expected_physical(&fixture, one);
        let expected_two = expected_physical(&fixture, two);

        let added = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_one.clone()],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![one.to_string(), two.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            added.code,
            LocalAlbumTagBatchPreflightErrorCode::MembershipChanged
        );

        let removed = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_one.clone(), expected_two],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![one.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            removed.code,
            LocalAlbumTagBatchPreflightErrorCode::MembershipChanged
        );

        let wrong_kind = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![LocalAlbumTagBatchExpectedTrack {
                    uri: one.to_string(),
                    kind: LocalAlbumTrackKind::CueVirtual,
                    revision: None,
                }],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![one.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            wrong_kind.code,
            LocalAlbumTagBatchPreflightErrorCode::MembershipChanged
        );

        let duplicate = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_one.clone(), expected_one],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![one.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            duplicate.code,
            LocalAlbumTagBatchPreflightErrorCode::DuplicateExpectedUri
        );
    }

    #[test]
    fn batch_preflight_rejects_revision_conflict_and_blocks_inspection_failure() {
        let fixture = Fixture::new("Music");
        write_minimal_flac(&fixture.root.join("track.flac"), &["ALBUM=Before"]);
        let uri = "Music/track.flac";
        let mut stale = expected_physical(&fixture, uri);
        stale.revision = Some("stale-revision".to_string());

        let conflict = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(vec![stale], LocalAlbumTagBatchUpdate::default()),
            vec![uri.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            conflict.code,
            LocalAlbumTagBatchPreflightErrorCode::RevisionConflict
        );

        let missing = "Music/missing.flac";
        let blocked = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![LocalAlbumTagBatchExpectedTrack {
                    uri: missing.to_string(),
                    kind: LocalAlbumTrackKind::Physical,
                    revision: Some("expected-revision".to_string()),
                }],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![missing.to_string()],
            &fixture.folders(),
        )
        .unwrap();
        assert!(!blocked.applicable);
        assert_eq!(
            blocked.blocked_reason,
            Some(LocalAlbumTagBatchBlockedReason::TrackInspectionFailed)
        );
        assert_eq!(blocked.change_count, 0);
        assert_eq!(blocked.unchanged_count, 0);
        assert_eq!(
            blocked.tracks[0].issue.as_ref().map(|issue| &issue.code),
            Some(&LocalTrackTagsErrorCode::FileNotFound)
        );
    }

    #[test]
    fn batch_preflight_blocks_cue_changes_but_accepts_unchanged_noop() {
        let fixture = Fixture::new("Music");
        let cue = "Music/Artist/Album/image.cue/track0001";

        let blocked = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_cue(cue)],
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("New album".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            vec![cue.to_string()],
            &fixture.folders(),
        )
        .unwrap();
        assert!(!blocked.applicable);
        assert_eq!(
            blocked.blocked_reason,
            Some(LocalAlbumTagBatchBlockedReason::CueVirtual)
        );
        assert!(blocked.tracks.is_empty());

        let noop = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_cue(cue)],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![cue.to_string()],
            &fixture.folders(),
        )
        .unwrap();
        assert!(noop.applicable);
        assert_eq!(noop.change_count, 0);
        assert_eq!(noop.unchanged_count, 0);
    }

    #[test]
    fn batch_preflight_minimizes_equal_sets_and_absent_removals() {
        let fixture = Fixture::new("Music");
        write_minimal_flac(
            &fixture.root.join("track.flac"),
            &["TITLE=Untouched", "ARTIST=Untouched", "ALBUM=Exact"],
        );
        let uri = "Music/track.flac";
        let plan = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_physical(&fixture, uri)],
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("Exact".to_string()),
                    album_artist: TagFieldUpdate::Remove,
                    date: TagFieldUpdate::Remove,
                    genre: TagFieldUpdate::Remove,
                },
            ),
            vec![uri.to_string()],
            &fixture.folders(),
        )
        .unwrap();

        assert!(plan.applicable);
        assert_eq!(plan.change_count, 0);
        assert_eq!(plan.unchanged_count, 1);
        let update = &plan.tracks[0].update;
        assert!(matches!(update.album, TagFieldUpdate::Unchanged));
        assert!(matches!(update.album_artist, TagFieldUpdate::Unchanged));
        assert!(matches!(update.date, TagFieldUpdate::Unchanged));
        assert!(matches!(update.genre, TagFieldUpdate::Unchanged));
        assert!(matches!(update.title, TagFieldUpdate::Unchanged));
        assert!(matches!(update.artist, TagFieldUpdate::Unchanged));
        assert!(matches!(update.track_number, TagFieldUpdate::Unchanged));
        assert!(matches!(update.track_total, TagFieldUpdate::Unchanged));
        assert!(matches!(update.disc_number, TagFieldUpdate::Unchanged));
        assert!(matches!(update.disc_total, TagFieldUpdate::Unchanged));
    }

    #[test]
    fn batch_preflight_builds_exact_per_track_patches_for_mixed_metadata() {
        let fixture = Fixture::new("Músicas");
        let first = fixture.root.join("Álbum/Faixa 1.flac");
        let second = fixture.root.join("Álbum/Faixa 2.flac");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        write_minimal_flac(
            &first,
            &["ALBUM=  Álbum Único  ", "ALBUMARTIST=Artista", "DATE=2024"],
        );
        write_minimal_flac(&second, &["ALBUM=Outro", "GENRE=Jazz"]);
        let first_uri = "Músicas/Álbum/Faixa 1.flac";
        let second_uri = "Músicas/Álbum/Faixa 2.flac";
        let exact_album = "  Álbum Único  ".to_string();

        let plan = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![
                    expected_physical(&fixture, first_uri),
                    expected_physical(&fixture, second_uri),
                ],
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set(exact_album.clone()),
                    album_artist: TagFieldUpdate::Set("Artista".to_string()),
                    date: TagFieldUpdate::Remove,
                    genre: TagFieldUpdate::Set("Gênero novo".to_string()),
                },
            ),
            vec![first_uri.to_string(), second_uri.to_string()],
            &fixture.folders(),
        )
        .unwrap();

        assert!(plan.applicable);
        assert_eq!(plan.change_count, 2);
        assert_eq!(plan.unchanged_count, 0);
        let first_plan = plan
            .tracks
            .iter()
            .find(|track| track.uri == first_uri)
            .unwrap();
        assert!(matches!(first_plan.update.album, TagFieldUpdate::Unchanged));
        assert!(matches!(
            first_plan.update.album_artist,
            TagFieldUpdate::Unchanged
        ));
        assert!(matches!(first_plan.update.date, TagFieldUpdate::Remove));
        assert_eq!(
            first_plan.update.genre,
            TagFieldUpdate::Set("Gênero novo".to_string())
        );

        let second_plan = plan
            .tracks
            .iter()
            .find(|track| track.uri == second_uri)
            .unwrap();
        assert_eq!(
            second_plan.update.album,
            TagFieldUpdate::Set(exact_album)
        );
        assert_eq!(
            second_plan.update.album_artist,
            TagFieldUpdate::Set("Artista".to_string())
        );
        assert!(matches!(second_plan.update.date, TagFieldUpdate::Unchanged));
        assert_eq!(
            second_plan.update.genre,
            TagFieldUpdate::Set("Gênero novo".to_string())
        );
    }

    #[test]
    fn batch_apply_fresh_preflight_rejects_stale_and_changed_membership_without_writes() {
        let fixture = Fixture::new("Music");
        write_minimal_flac(&fixture.root.join("one.flac"), &["ALBUM=Before"]);
        write_minimal_flac(&fixture.root.join("two.flac"), &["ALBUM=Before"]);
        let one = "Music/one.flac";
        let two = "Music/two.flac";
        let before_one = fs::read(fixture.root.join("one.flac")).unwrap();
        let before_two = fs::read(fixture.root.join("two.flac")).unwrap();
        let update = LocalAlbumTagBatchUpdate {
            album: TagFieldUpdate::Set("After".to_string()),
            ..LocalAlbumTagBatchUpdate::default()
        };

        let mut stale = expected_physical(&fixture, one);
        stale.revision = Some("stale-revision".to_string());
        let stale_error = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(vec![stale], update.clone()),
            vec![one.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            stale_error.code,
            LocalAlbumTagBatchPreflightErrorCode::RevisionConflict
        );

        let membership_error = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(vec![expected_physical(&fixture, one)], update),
            vec![one.to_string(), two.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            membership_error.code,
            LocalAlbumTagBatchPreflightErrorCode::MembershipChanged
        );
        assert_eq!(fs::read(fixture.root.join("one.flac")).unwrap(), before_one);
        assert_eq!(fs::read(fixture.root.join("two.flac")).unwrap(), before_two);
    }

    #[test]
    fn batch_apply_detects_revision_change_before_fresh_preflight() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("track.flac");
        write_minimal_flac(&file, &["ALBUM=Before"]);
        let uri = "Music/track.flac";
        let expected = expected_physical(&fixture, uri);
        let mut changed = OpenOptions::new().append(true).open(&file).unwrap();
        changed.write_all(b"external-change").unwrap();
        changed.sync_all().unwrap();
        drop(changed);
        let before_apply = fs::read(&file).unwrap();

        let error = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected],
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("After".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            vec![uri.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();

        assert_eq!(
            error.code,
            LocalAlbumTagBatchPreflightErrorCode::RevisionConflict
        );
        assert_eq!(fs::read(&file).unwrap(), before_apply);
    }

    #[test]
    fn batch_apply_cue_block_and_untrusted_uri_never_reach_filesystem_writes() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("safe.flac");
        write_minimal_flac(&file, &["ALBUM=Before"]);
        let safe = "Music/safe.flac";
        let cue = "Music/Album/image.cue/track0001";
        let before = fs::read(&file).unwrap();
        let update = LocalAlbumTagBatchUpdate {
            album: TagFieldUpdate::Set("After".to_string()),
            ..LocalAlbumTagBatchUpdate::default()
        };

        let blocked = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![expected_physical(&fixture, safe), expected_cue(cue)],
                update.clone(),
            ),
            vec![safe.to_string(), cue.to_string()],
            &fixture.folders(),
        )
        .unwrap();
        assert!(!blocked.applicable);
        assert_eq!(
            blocked.blocked_reason,
            Some(LocalAlbumTagBatchBlockedReason::CueVirtual)
        );
        assert_eq!(blocked.not_attempted_count, 1);

        let arbitrary = LocalAlbumTagBatchExpectedTrack {
            uri: "../../outside.flac".to_string(),
            kind: LocalAlbumTrackKind::Physical,
            revision: Some("untrusted".to_string()),
        };
        let error = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(vec![arbitrary], update),
            vec![safe.to_string()],
            &fixture.folders(),
        )
        .unwrap_err();
        assert_eq!(
            error.code,
            LocalAlbumTagBatchPreflightErrorCode::MembershipChanged
        );
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[test]
    fn batch_apply_noop_and_single_write_return_truthful_post_write_state() {
        let fixture = Fixture::new("Músicas");
        let file = fixture.root.join("Álbum/Faixa.flac");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        write_minimal_flac(
            &file,
            &["TITLE=Preservado", "ARTIST=Artista", "ALBUM=Antes"],
        );
        let uri = "Músicas/Álbum/Faixa.flac";
        let initial = expected_physical(&fixture, uri);
        let original_revision = initial.revision.clone().unwrap();

        let noop = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![initial.clone()],
                LocalAlbumTagBatchUpdate::default(),
            ),
            vec![uri.to_string()],
            &fixture.folders(),
        )
        .unwrap();
        assert!(noop.applicable);
        assert_eq!(noop.committed_count, 0);
        assert_eq!(noop.unchanged_count, 1);
        assert_eq!(
            noop.tracks[0].state,
            LocalAlbumTagBatchTrackOutcomeState::Unchanged
        );

        let applied = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(
                vec![initial],
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("Depois".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            vec![uri.to_string()],
            &fixture.folders(),
        )
        .unwrap();
        assert_eq!(applied.committed_count, 1);
        assert!(!applied.partial_commit);
        assert_eq!(
            applied.tracks[0].state,
            LocalAlbumTagBatchTrackOutcomeState::Committed
        );
        let tags = applied.tracks[0].tags.as_ref().unwrap();
        assert_eq!(tags.album.as_deref(), Some("Depois"));
        assert_eq!(tags.title.as_deref(), Some("Preservado"));
        assert_eq!(tags.artist.as_deref(), Some("Artista"));
        assert_ne!(tags.revision, original_revision);
    }

    #[test]
    fn batch_executor_orders_writes_by_logical_uri() {
        let fixture = Fixture::new("Music");
        for name in ["z.flac", "a.flac", "m.flac"] {
            write_minimal_flac(&fixture.root.join(name), &["ALBUM=Before"]);
        }
        let uris = ["Music/z.flac", "Music/a.flac", "Music/m.flac"];
        let plan = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                uris.iter()
                    .map(|uri| expected_physical(&fixture, uri))
                    .collect(),
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("After".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            uris.iter().map(|uri| uri.to_string()).collect(),
            &fixture.folders(),
        )
        .unwrap();
        let mut calls = Vec::new();

        let result = execute_local_album_tag_batch_plan_with(plan, |track| {
            calls.push(track.uri.clone());
            Ok(LocalTrackTagWriteResult {
                tags: fixture.inspect(&track.uri).unwrap(),
                committed: true,
            })
        });

        assert_eq!(
            calls,
            vec!["Music/a.flac", "Music/m.flac", "Music/z.flac"]
        );
        assert_eq!(
            result
                .tracks
                .iter()
                .map(|track| track.uri.as_str())
                .collect::<Vec<_>>(),
            calls.iter().map(String::as_str).collect::<Vec<_>>()
        );
    }

    #[test]
    fn batch_apply_first_failure_stops_all_later_writes() {
        let fixture = Fixture::new("Music");
        for name in ["a.flac", "b.flac", "c.flac"] {
            write_minimal_flac(&fixture.root.join(name), &["ALBUM=Before"]);
        }
        let uris = ["Music/a.flac", "Music/b.flac", "Music/c.flac"];
        let mut permissions = fs::metadata(fixture.root.join("a.flac"))
            .unwrap()
            .permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(fixture.root.join("a.flac"), permissions).unwrap();
        let expected = uris
            .iter()
            .map(|uri| expected_physical(&fixture, uri))
            .collect();
        let before_b = fs::read(fixture.root.join("b.flac")).unwrap();
        let before_c = fs::read(fixture.root.join("c.flac")).unwrap();

        let result = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(
                expected,
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("After".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            uris.iter().map(|uri| uri.to_string()).collect(),
            &fixture.folders(),
        )
        .unwrap();

        assert_eq!(result.committed_count, 0);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.not_attempted_count, 2);
        assert!(!result.partial_commit);
        assert_eq!(
            result.tracks[0].error.as_ref().map(|error| &error.code),
            Some(&LocalTrackTagsErrorCode::PermissionDenied)
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains(fixture.base.to_string_lossy().as_ref()));
        assert_eq!(fs::read(fixture.root.join("b.flac")).unwrap(), before_b);
        assert_eq!(fs::read(fixture.root.join("c.flac")).unwrap(), before_c);
    }

    #[test]
    fn batch_apply_reports_partial_commit_and_not_attempted_tracks() {
        let fixture = Fixture::new("Music");
        for name in ["a.flac", "b.flac", "c.flac"] {
            write_minimal_flac(&fixture.root.join(name), &["ALBUM=Before"]);
        }
        let uris = ["Music/a.flac", "Music/b.flac", "Music/c.flac"];
        let mut permissions = fs::metadata(fixture.root.join("b.flac"))
            .unwrap()
            .permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(fixture.root.join("b.flac"), permissions).unwrap();
        let expected = uris
            .iter()
            .map(|uri| expected_physical(&fixture, uri))
            .collect();
        let before_c = fs::read(fixture.root.join("c.flac")).unwrap();

        let result = apply_local_album_tag_batch(
            &fixture.library,
            batch_request(
                expected,
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("After".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            uris.iter().map(|uri| uri.to_string()).collect(),
            &fixture.folders(),
        )
        .unwrap();

        assert_eq!(result.committed_count, 1);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.not_attempted_count, 1);
        assert!(result.partial_commit);
        assert_eq!(
            result.tracks[0].state,
            LocalAlbumTagBatchTrackOutcomeState::Committed
        );
        assert_eq!(
            result.tracks[1].state,
            LocalAlbumTagBatchTrackOutcomeState::Failed
        );
        assert_eq!(
            result.tracks[2].state,
            LocalAlbumTagBatchTrackOutcomeState::NotAttempted
        );
        assert_eq!(fixture.inspect(uris[0]).unwrap().album.as_deref(), Some("After"));
        assert_eq!(fs::read(fixture.root.join("c.flac")).unwrap(), before_c);
    }

    #[test]
    fn batch_writer_detects_conflict_after_prior_track_committed() {
        let fixture = Fixture::new("Music");
        for name in ["a.flac", "b.flac", "c.flac"] {
            write_minimal_flac(&fixture.root.join(name), &["ALBUM=Before"]);
        }
        let uris = ["Music/a.flac", "Music/b.flac", "Music/c.flac"];
        let plan = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                uris.iter()
                    .map(|uri| expected_physical(&fixture, uri))
                    .collect(),
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("After".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            uris.iter().map(|uri| uri.to_string()).collect(),
            &fixture.folders(),
        )
        .unwrap();
        let mut changed = OpenOptions::new()
            .append(true)
            .open(fixture.root.join("b.flac"))
            .unwrap();
        changed.write_all(b"external-change").unwrap();
        changed.sync_all().unwrap();
        drop(changed);
        let before_c = fs::read(fixture.root.join("c.flac")).unwrap();

        let result = execute_local_album_tag_batch_plan(
            &fixture.library,
            plan,
            &fixture.folders(),
        );

        assert_eq!(result.committed_count, 1);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.not_attempted_count, 1);
        assert!(result.partial_commit);
        assert_eq!(
            result.tracks[1].error.as_ref().map(|error| &error.code),
            Some(&LocalTrackTagsErrorCode::RevisionConflict)
        );
        assert_eq!(fs::read(fixture.root.join("c.flac")).unwrap(), before_c);
    }

    #[test]
    fn committed_writer_error_remains_committed_and_stops_the_batch() {
        let fixture = Fixture::new("Music");
        for name in ["a.flac", "b.flac"] {
            write_minimal_flac(&fixture.root.join(name), &["ALBUM=Before"]);
        }
        let uris = ["Music/a.flac", "Music/b.flac"];
        let plan = preflight_local_album_tag_batch(
            &fixture.library,
            batch_request(
                uris.iter()
                    .map(|uri| expected_physical(&fixture, uri))
                    .collect(),
                LocalAlbumTagBatchUpdate {
                    album: TagFieldUpdate::Set("After".to_string()),
                    ..LocalAlbumTagBatchUpdate::default()
                },
            ),
            uris.iter().map(|uri| uri.to_string()).collect(),
            &fixture.folders(),
        )
        .unwrap();
        let mut calls = 0;

        let result = execute_local_album_tag_batch_plan_with(plan, |_| {
            calls += 1;
            Err(LocalTrackTagsError::committed(
                LocalTrackTagsErrorCode::DurabilityFailed,
                "As tags foram substituídas, mas a durabilidade não foi confirmada.",
            ))
        });

        assert_eq!(calls, 1);
        assert_eq!(result.committed_count, 1);
        assert_eq!(result.failed_count, 0);
        assert_eq!(result.not_attempted_count, 1);
        assert!(result.partial_commit);
        assert_eq!(
            result.tracks[0].state,
            LocalAlbumTagBatchTrackOutcomeState::Committed
        );
        assert!(result.tracks[0].tags.is_none());
        assert!(result.tracks[0]
            .error
            .as_ref()
            .is_some_and(|error| error.committed));
    }

    #[test]
    fn transactional_flac_update_sets_all_supported_fields_and_preserves_audio_payload() {
        let fixture = Fixture::new("Músicas locais");
        let album = fixture.root.join("Artista/Álbum com espaços");
        fs::create_dir_all(&album).unwrap();
        let file = album.join("Faixa 01.flac");
        write_minimal_flac(
            &file,
            &[
                "TITLE=Antes",
                "ARTIST=Artista anterior",
                "ALBUM=Álbum anterior",
                "TRACKNUMBER=1",
                "TRACKTOTAL=2",
                "DISCNUMBER=1",
                "DISCTOTAL=1",
            ],
        );
        let before_payload = flac_audio_payload(&file);
        let before = fixture
            .inspect("Músicas locais/Artista/Álbum com espaços/Faixa 01.flac")
            .unwrap();
        assert!(before.writable);

        let updated = fixture
            .update(
                "Músicas locais/Artista/Álbum com espaços/Faixa 01.flac",
                &before.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("Depois — Unicode".into()),
                    artist: TagFieldUpdate::Set("Nova artista".into()),
                    album: TagFieldUpdate::Set("Novo álbum".into()),
                    album_artist: TagFieldUpdate::Set("Artista do álbum".into()),
                    genre: TagFieldUpdate::Set("MPB".into()),
                    date: TagFieldUpdate::Set("2026-10-06".into()),
                    track_number: TagFieldUpdate::Set(2),
                    track_total: TagFieldUpdate::Set(12),
                    disc_number: TagFieldUpdate::Set(1),
                    disc_total: TagFieldUpdate::Set(2),
                },
            )
            .unwrap();

        assert_eq!(updated.title.as_deref(), Some("Depois — Unicode"));
        assert_eq!(updated.artist.as_deref(), Some("Nova artista"));
        assert_eq!(updated.album.as_deref(), Some("Novo álbum"));
        assert_eq!(updated.album_artist.as_deref(), Some("Artista do álbum"));
        assert_eq!(updated.genre.as_deref(), Some("MPB"));
        assert_eq!(updated.date.as_deref(), Some("2026-10-06"));
        assert_eq!(updated.track_number, Some(2));
        assert_eq!(updated.track_total, Some(12));
        assert_eq!(updated.disc_number, Some(1));
        assert_eq!(updated.disc_total, Some(2));
        assert_ne!(updated.revision, before.revision);
        assert_eq!(flac_audio_payload(&file), before_payload);

        let mut reopened = File::open(&file).unwrap();
        let parsed = read_tagged_file(&mut reopened, true).unwrap();
        assert_eq!(parsed.file_type(), FileType::Flac);
        assert_eq!(parsed.properties().sample_rate(), Some(44_100));
        assert_eq!(parsed.properties().bit_depth(), Some(16));
        assert_eq!(parsed.properties().channels(), Some(2));
        assert_eq!(
            parsed.properties().duration(),
            std::time::Duration::from_secs(1)
        );
        assert!(temp_files_below(&fixture.root).is_empty());
    }

    #[test]
    fn remove_and_unchanged_have_distinct_patch_semantics() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("patch.flac");
        write_minimal_flac(
            &file,
            &["TITLE=Keep me", "ARTIST=Remove me", "ALBUM=Keep album"],
        );
        let before = fixture.inspect("Music/patch.flac").unwrap();
        let updated = fixture
            .update(
                "Music/patch.flac",
                &before.revision,
                &LocalTrackTagUpdate {
                    artist: TagFieldUpdate::Remove,
                    genre: TagFieldUpdate::Set("Jazz".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(updated.title.as_deref(), Some("Keep me"));
        assert_eq!(updated.album.as_deref(), Some("Keep album"));
        assert_eq!(updated.artist, None);
        assert_eq!(updated.genre.as_deref(), Some("Jazz"));
    }

    #[test]
    fn custom_tags_comments_replaygain_and_artwork_survive_text_update() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("preserve.flac");
        write_minimal_flac(&file, &["TITLE=Before"]);
        let (before_items, before_pictures) = add_preserved_metadata(&file);
        let before_payload = flac_audio_payload(&file);
        let before = fixture.inspect("Music/preserve.flac").unwrap();

        fixture
            .update(
                "Music/preserve.flac",
                &before.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("After".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let mut reopened = File::open(&file).unwrap();
        let tagged = read_tagged_file(&mut reopened, true).unwrap();
        let tag = tagged.primary_tag().unwrap();
        let after_items = tag.items().cloned().collect::<Vec<_>>();
        for key in [
            ItemKey::Composer,
            ItemKey::ReplayGainTrackGain,
            ItemKey::MusicBrainzRecordingId,
            ItemKey::Comment,
        ] {
            assert_eq!(
                after_items.iter().find(|item| item.key() == key),
                before_items.iter().find(|item| item.key() == key)
            );
        }
        assert_eq!(tag.pictures(), before_pictures);
        assert_eq!(flac_audio_payload(&file), before_payload);
    }

    #[test]
    fn stale_revision_and_mid_transaction_change_are_conflicts() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("conflict.flac");
        write_minimal_flac(&file, &["TITLE=Original"]);
        let read = fixture.inspect("Music/conflict.flac").unwrap();
        let first = fixture
            .update(
                "Music/conflict.flac",
                &read.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("First".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let after_first = fs::read(&file).unwrap();

        let stale = fixture
            .update(
                "Music/conflict.flac",
                &read.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("Stale".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(stale.code, LocalTrackTagsErrorCode::RevisionConflict);
        assert_eq!(fs::read(&file).unwrap(), after_first);

        let concurrent = update_local_track_tags_with_failure(
            &fixture.library,
            "Music/conflict.flac",
            &fixture.folders(),
            &first.revision,
            &LocalTrackTagUpdate {
                title: TagFieldUpdate::Set("Transaction".into()),
                ..Default::default()
            },
            Some(TransactionFailPoint::ConcurrentModification),
        )
        .unwrap_err();
        assert_eq!(concurrent.code, LocalTrackTagsErrorCode::RevisionConflict);
        let bytes = fs::read(&file).unwrap();
        assert!(bytes.ends_with(b"external-change"));
        assert!(!bytes
            .windows(b"Transaction".len())
            .any(|part| part == b"Transaction"));
        assert!(temp_files_below(&fixture.root).is_empty());
    }

    #[test]
    fn all_precommit_failure_points_keep_original_and_clean_temp() {
        for fail_at in [
            TransactionFailPoint::CreateTemp,
            TransactionFailPoint::Copy,
            TransactionFailPoint::Save,
            TransactionFailPoint::Validate,
        ] {
            let fixture = Fixture::new("Music");
            let file = fixture.root.join(format!("failure-{fail_at:?}.flac"));
            write_minimal_flac(&file, &["TITLE=Original"]);
            let before_bytes = fs::read(&file).unwrap();
            let path = format!("Music/failure-{fail_at:?}.flac");
            let read = fixture.inspect(&path).unwrap();

            let error = update_local_track_tags_with_failure(
                &fixture.library,
                &path,
                &fixture.folders(),
                &read.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("Changed".into()),
                    ..Default::default()
                },
                Some(fail_at),
            )
            .unwrap_err();
            assert_eq!(error.code, LocalTrackTagsErrorCode::TransactionFailed);
            assert_eq!(fs::read(&file).unwrap(), before_bytes);
            assert!(temp_files_below(&fixture.root).is_empty());
        }
    }

    #[test]
    fn permissions_and_xattrs_are_preserved() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("metadata.flac");
        write_minimal_flac(&file, &["TITLE=Before"]);
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o640);
        fs::set_permissions(&file, permissions).unwrap();
        let xattr_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file)
            .unwrap();
        set_user_xattr(&xattr_file, "user.sonante-test", b"preserve-me");
        let before_metadata = fs::metadata(&file).unwrap();
        let before_xattrs = read_xattrs(xattr_file.as_raw_fd()).unwrap();
        drop(xattr_file);
        let read = fixture.inspect("Music/metadata.flac").unwrap();

        fixture
            .update(
                "Music/metadata.flac",
                &read.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("After".into()),
                    ..Default::default()
                },
            )
            .unwrap();

        let after_file = File::open(&file).unwrap();
        let after_metadata = after_file.metadata().unwrap();
        assert_eq!(
            after_metadata.mode() & 0o7777,
            before_metadata.mode() & 0o7777
        );
        assert_eq!(after_metadata.uid(), before_metadata.uid());
        assert_eq!(after_metadata.gid(), before_metadata.gid());
        assert_eq!(read_xattrs(after_file.as_raw_fd()).unwrap(), before_xattrs);
    }

    #[test]
    fn multiple_hard_links_are_rejected_without_splitting_file_identity() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("linked.flac");
        let second_link = fixture.root.join("linked-copy.flac");
        write_minimal_flac(&file, &["TITLE=Before"]);
        fs::hard_link(&file, &second_link).unwrap();
        let before = fs::read(&file).unwrap();
        let read = fixture.inspect("Music/linked.flac").unwrap();

        let error = fixture
            .update(
                "Music/linked.flac",
                &read.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("After".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();

        assert_eq!(
            error.code,
            LocalTrackTagsErrorCode::MetadataPreservationFailed
        );
        assert_eq!(fs::read(&file).unwrap(), before);
        assert_eq!(fs::read(&second_link).unwrap(), before);
        assert!(temp_files_below(&fixture.root).is_empty());
    }

    #[test]
    fn read_only_cue_remote_and_unsupported_inputs_never_change_files() {
        let fixture = Fixture::new("Music");
        let readonly = fixture.root.join("readonly-write.flac");
        write_minimal_flac(&readonly, &["TITLE=Before"]);
        let mut permissions = fs::metadata(&readonly).unwrap().permissions();
        permissions.set_mode(0o444);
        fs::set_permissions(&readonly, permissions).unwrap();
        let before = fixture.inspect("Music/readonly-write.flac").unwrap();
        let original_bytes = fs::read(&readonly).unwrap();
        let error = fixture
            .update(
                "Music/readonly-write.flac",
                &before.revision,
                &LocalTrackTagUpdate {
                    title: TagFieldUpdate::Set("After".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert_eq!(error.code, LocalTrackTagsErrorCode::PermissionDenied);
        assert_eq!(fs::read(&readonly).unwrap(), original_bytes);

        for path in [
            "Music/Album.cue/track0001",
            "https://example.invalid/audio.flac?X-Plex-Token=secret",
        ] {
            let error = fixture
                .update(path, "opaque", &LocalTrackTagUpdate::default())
                .unwrap_err();
            assert!(matches!(
                error.code,
                LocalTrackTagsErrorCode::CueVirtual | LocalTrackTagsErrorCode::InvalidLocalPath
            ));
        }

        let unsupported = fixture.root.join("track.dsf");
        fs::write(&unsupported, b"DSD unsupported").unwrap();
        let read_error = fixture.inspect("Music/track.dsf").unwrap_err();
        let resolved =
            resolve_local_track_file(&fixture.library, "Music/track.dsf", &fixture.folders())
                .unwrap();
        let unsupported_revision = revision_for_file(&resolved.file).unwrap();
        let write_error = fixture
            .update(
                "Music/track.dsf",
                &unsupported_revision,
                &LocalTrackTagUpdate::default(),
            )
            .unwrap_err();
        assert_eq!(read_error.code, LocalTrackTagsErrorCode::UnsupportedFormat);
        assert_eq!(write_error.code, LocalTrackTagsErrorCode::UnsupportedFormat);
        assert_eq!(fs::read(&unsupported).unwrap(), b"DSD unsupported");
        assert!(temp_files_below(&fixture.root).is_empty());
    }

    #[test]
    fn invalid_values_are_rejected_before_any_temp_or_write() {
        let fixture = Fixture::new("Music");
        let file = fixture.root.join("validation.flac");
        write_minimal_flac(&file, &["TITLE=Before", "TRACKNUMBER=2", "TRACKTOTAL=3"]);
        let read = fixture.inspect("Music/validation.flac").unwrap();
        let original = fs::read(&file).unwrap();

        for patch in [
            LocalTrackTagUpdate {
                title: TagFieldUpdate::Set("bad\nvalue".into()),
                ..Default::default()
            },
            LocalTrackTagUpdate {
                track_number: TagFieldUpdate::Set(0),
                ..Default::default()
            },
            LocalTrackTagUpdate {
                track_number: TagFieldUpdate::Set(4),
                ..Default::default()
            },
        ] {
            let error = fixture
                .update("Music/validation.flac", &read.revision, &patch)
                .unwrap_err();
            assert_eq!(error.code, LocalTrackTagsErrorCode::InvalidTagUpdate);
            assert_eq!(fs::read(&file).unwrap(), original);
            assert!(temp_files_below(&fixture.root).is_empty());
        }
    }
}
