//! Original tag read/inspection DTOs and value projection; no file writer.
use serde::{Serialize,Deserialize};
use lofty::{file::{TaggedFile,TaggedFileExt,FileType},tag::{Accessor,ItemKey}};
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
    pub(super) fn new(code: LocalTrackTagsErrorCode, message: &str) -> Self {
        Self {
            code,
            message: message.to_string(),
            format: None,
            editable: false,
            unsupported_reason: None,
            committed: false,
        }
    }

    pub(super) fn unsupported(code: LocalTrackTagsErrorCode, format: &str, reason: &str) -> Self {
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


pub(super) fn local_track_tags_from_file(tagged_file: &TaggedFile, revision: String) -> LocalTrackTags {
    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());
    let file_type = tagged_file.file_type();
    let writable = false; // Windows transactional writer is not qualified yet.

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
pub(super) fn summarize_album_field(
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
