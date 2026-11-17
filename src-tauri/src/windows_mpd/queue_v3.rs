//! Wire format/projection for the mixed-source Windows journal. This codec has
//! no disk access, engine, listener, migration write or automatic repair.
use super::{
    library::Track,
    options::PlaybackOptions,
    queue_edit::{self, Edit, Placement},
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
const ERROR: &str = "queue_restore_failed";
const MAX_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const FILE_NAME: &str = "windows-queue-v3.json";
#[path = "../windows_navidrome/unique_json.rs"]
mod unique_json;
type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Source {
    Local {
        root: PathBuf,
        path: PathBuf,
    },
    Navidrome {
        account_id: String,
        song_id: String,
    },
    Plex {
        server_id: String,
        part_key: String,
        rating_key: String,
    },
}
impl Source {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Local { root, path }
                if valid_path(root) && valid_path(path) && path.starts_with(root) =>
            {
                Ok(())
            }
            Self::Navidrome {
                account_id,
                song_id,
            } if crate::navidrome::valid_account(account_id) && crate::navidrome::id(song_id) => {
                Ok(())
            }
            Self::Plex {
                server_id,
                part_key,
                rating_key,
            } if !server_id.is_empty()
                && server_id.len() <= 256
                && server_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                && !rating_key.is_empty()
                && rating_key.len() <= 128
                && rating_key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                && part_key.starts_with('/')
                && !part_key.starts_with("//")
                && part_key.len() <= 4096
                && !part_key.contains(['\\', '\0', '\r', '\n', '?', '#'])
                && !part_key.contains("://")
                && !crate::plex::contains_plex_token(part_key) =>
            {
                Ok(())
            }
            _ => Err(ERROR),
        }
    }
    /// Only stable locator fields survive. Legacy references lacking account or
    /// exact PMS metadata identity remain unavailable until explicitly rebound.
    pub(crate) fn from_locator(locator: &crate::audio::MediaLocator) -> Result<Self> {
        use crate::audio::MediaLocator;
        let source = match locator {
            MediaLocator::Navidrome {
                account_id: Some(account_id),
                song_id,
                ..
            } => Self::Navidrome {
                account_id: account_id.clone(),
                song_id: song_id.clone(),
            },
            MediaLocator::Plex {
                server_id,
                part_key,
                rating_key: Some(rating_key),
                ..
            } => Self::Plex {
                server_id: server_id.clone(),
                part_key: part_key.clone(),
                rating_key: rating_key.clone(),
            },
            _ => return Err(ERROR),
        };
        source.validate()?;
        Ok(source)
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Display {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) disc: u32,
    pub(crate) number: u32,
    pub(crate) file_name: String,
    #[serde(default)]
    pub(crate) catalog_album_id: Option<String>,
}
impl Display {
    fn validate(&self) -> Result<()> {
        // Same display limits as the existing local SavedQueue; do not reject
        // an existing multiline tag or invent missing metadata during migration.
        if [
            &self.id,
            &self.title,
            &self.artist,
            &self.album,
            &self.file_name,
        ]
        .iter()
        .any(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err(ERROR);
        }
        Ok(())
    }
    fn local(track: &Track) -> Self {
        Self {
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            disc: track.disc,
            number: track.number,
            file_name: track.file_name.clone(),
            catalog_album_id: track.catalog_album_id.clone(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) metadata: Display,
    pub(crate) source: Source,
}
impl Entry {
    pub(crate) fn local(track: &Track, root: &Path) -> Result<Self> {
        if track.radio.is_some() || track.upnp.is_some() {
            return Err(ERROR);
        }
        let row = Self {
            metadata: Display::local(track),
            source: match &track.remote_source {
                Some(Source::Local { .. }) => return Err(ERROR),
                Some(source) if track.path.as_os_str().is_empty() && track.media_root.is_none() => source.clone(),
                Some(_) => return Err(ERROR),
                None => Source::Local {
                    root: track.media_root.clone().unwrap_or_else(|| root.to_path_buf()),
                    path: track.path.clone(),
                },
            },
        };
        row.validate()?;
        Ok(row)
    }
    fn validate(&self) -> Result<()> {
        self.metadata.validate()?;
        self.source.validate()
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Logical {
    pub(crate) tracks: Vec<Entry>,
    pub(crate) index: usize,
    pub(crate) elapsed: f64,
    pub(crate) duration: f64,
    pub(crate) volume: u8,
    pub(crate) album_id: Option<String>,
    pub(crate) options: Option<PlaybackOptions>,
}
impl Logical {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.tracks.is_empty()
            || self.tracks.len() > 1000
            || self.index >= self.tracks.len()
            || self.volume > 100
            || !self.elapsed.is_finite()
            || !self.duration.is_finite()
            || self.elapsed < 0.
            || self.duration < 0.
            || self.elapsed > 604800.
            || self.duration > 604800.
            || (self.duration > 0. && self.elapsed > self.duration)
            || self.album_id.as_ref().is_some_and(|s| s.len() > 128)
        {
            return Err(ERROR);
        }
        for row in &self.tracks {
            row.validate()?;
        }
        Ok(())
    }
    pub(crate) fn inserted(&self, tracks: Vec<Entry>, placement: Placement) -> Result<Self> {
        self.validate()?;
        if tracks.is_empty() || self.tracks.len() + tracks.len() > 1000 {
            return Err(ERROR);
        }
        for row in &tracks {
            row.validate()?;
        }
        let mut next = self.clone();
        let count = tracks.len();
        let at = queue_edit::position(self.tracks.len(), Some(self.index), placement)?;
        next.tracks.splice(at..at, tracks);
        if self.index >= at {
            next.index += count;
        }
        next.album_id = None;
        next.validate()?;
        Ok(next)
    }
    pub(crate) fn edited(&self, action: Edit) -> Result<Option<Self>> {
        self.validate()?;
        let plan = queue_edit::plan(self.tracks.len(), Some(self.index), false, action)?;
        let index = queue_edit::remap(self.tracks.len(), self.index, plan);
        let removed = plan.to.is_none() && plan.from == self.index;
        let mut next = self.clone();
        queue_edit::apply(&mut next.tracks, plan);
        let Some(index) = index else { return Ok(None) };
        next.index = index;
        next.album_id = None;
        if removed {
            next.elapsed = 0.;
            next.duration = 0.;
        }
        next.validate()?;
        Ok(Some(next))
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    content: Content,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Content {
    Empty {},
    Queue { queue: Logical },
}
// Enforce the wire budget while serializing, not after allocating a larger
// temporary JSON buffer. This is metadata only; no credentials are accepted.
struct Writer(Vec<u8>);
impl std::io::Write for Writer {
    fn write(&mut self,bytes:&[u8])->std::io::Result<usize>{
        if self.0.len().checked_add(bytes.len()).is_none_or(|n|n>MAX_BYTES){return Err(std::io::Error::other("queue_size"));}
        self.0.extend_from_slice(bytes);Ok(bytes.len())
    }
    fn flush(&mut self)->std::io::Result<()>{Ok(())}
}
pub(crate) fn encode(queue: Option<&Logical>) -> Result<Vec<u8>> {
    if let Some(queue) = queue {
        queue.validate()?;
    }
    let record = Record {
        version: 3,
        content: match queue {
            Some(q) => Content::Queue { queue: q.clone() },
            None => Content::Empty {},
        },
    };
    let mut writer=Writer(Vec::new());
    serde_json::to_writer(&mut writer,&record).map_err(|_|ERROR)?;
    Ok(writer.0)
}
fn value(bytes: &[u8]) -> Result<serde_json::Value> {
    if bytes.len() > MAX_BYTES {
        return Err(ERROR);
    }
    unique_json::readonly_unique_json(bytes).map_err(|_| ERROR)
}
fn options(value: Option<&serde_json::Value>) -> Result<()> {
    if let Some(value) = value.filter(|v| !v.is_null()) {
        let object = value.as_object().ok_or(ERROR)?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "repeat" | "single" | "random"))
        {
            return Err(ERROR);
        }
    }
    Ok(())
}
pub(crate) fn decode(bytes: &[u8]) -> Result<Option<Logical>> {
    let value = value(bytes)?;
    options(
        value
            .get("content")
            .and_then(|v| v.get("queue"))
            .and_then(|v| v.get("options")),
    )?;
    let record: Record = serde_json::from_value(value).map_err(|_| ERROR)?;
    if record.version != 3 {
        return Err(ERROR);
    }
    match record.content {
        Content::Empty {} => Ok(None),
        Content::Queue { queue } => {
            queue.validate()?;
            Ok(Some(queue))
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyEnvelope {
    version: u8,
    queue: Option<Legacy>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRow {
    metadata: serde_json::Value,
    path: PathBuf,
    #[serde(default)]
    root: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    root: PathBuf,
    tracks: Vec<LegacyRow>,
    index: usize,
    elapsed: f64,
    duration: f64,
    volume: u8,
    album_id: Option<String>,
    #[serde(default)]
    options: Option<PlaybackOptions>,
}
/// Read-only compatibility projection. Calling this function cannot rewrite,
/// delete, repair, reopen an audio path or start an engine/listener.
pub(crate) fn decode_legacy(bytes: &[u8]) -> Result<Option<Logical>> {
    let raw = value(bytes)?;
    if !raw.as_object().is_some_and(|v| v.contains_key("queue")) {
        return Err(ERROR);
    }
    options(raw.get("queue").and_then(|v| v.get("options")))?;
    let envelope: LegacyEnvelope = serde_json::from_value(raw).map_err(|_| ERROR)?;
    if ![1, 2].contains(&envelope.version) {
        return Err(ERROR);
    }
    let Some(old) = envelope.queue else {
        return Ok(None);
    };
    if !valid_path(&old.root) {
        return Err(ERROR);
    }
    let mut tracks = Vec::new();
    for mut row in old.tracks {
        let metadata = row.metadata.as_object_mut().ok_or(ERROR)?;
        if metadata.remove("radio").is_some_and(|v| !v.is_null()) || metadata.remove("upnp").is_some_and(|v| !v.is_null()) {
            return Err(ERROR);
        }
        let display: Display = serde_json::from_value(row.metadata).map_err(|_| ERROR)?;
        tracks.push(Entry {
            metadata: display,
            source: Source::Local {
                root: row.root.unwrap_or_else(|| old.root.clone()),
                path: row.path,
            },
        });
    }
    let queue = Logical {
        tracks,
        index: old.index,
        elapsed: old.elapsed,
        duration: old.duration,
        volume: old.volume,
        album_id: old.album_id,
        options: old.options,
    };
    queue.validate()?;
    Ok(Some(queue))
}
pub(crate) struct Restored {
    pub(crate) queue: Option<Logical>,
    pub(crate) needs_migration: bool,
}
/// The caller supplies independently admitted bounded file reads. Only absence
/// permits the legacy provider; I/O errors or invalid v3 bytes never fall back.
pub(crate) fn restore(
    current: std::result::Result<Option<Vec<u8>>, ()>,
    legacy: impl FnOnce() -> std::result::Result<Option<Vec<u8>>, ()>,
) -> Result<Restored> {
    match current.map_err(|_| ERROR)? {
        Some(bytes) => Ok(Restored {
            queue: decode(&bytes)?,
            needs_migration: false,
        }),
        None => match legacy().map_err(|_| ERROR)? {
            Some(bytes) => Ok(Restored {
                queue: decode_legacy(&bytes)?,
                needs_migration: true,
            }),
            None => Ok(Restored {
                queue: None,
                needs_migration: false,
            }),
        },
    }
}
fn valid_path(p: &Path) -> bool {
    let Some(s)=p.to_str()else{return false;};
    #[cfg(windows)]
    let prefix_allowed=match p.components().next(){
        Some(std::path::Component::Prefix(prefix))=>match prefix.kind(){
            std::path::Prefix::Disk(_)|std::path::Prefix::VerbatimDisk(_)=>true,
            std::path::Prefix::UNC(server,share)|std::path::Prefix::VerbatimUNC(server,share)=>!server.is_empty()&&share.to_str().is_some_and(|s|!s.is_empty()&&!s.eq_ignore_ascii_case("pipe")&&!s.eq_ignore_ascii_case("IPC$")),
            _=>false,
        },_=>false,
    };
    #[cfg(not(windows))]
    let prefix_allowed=true; // Portable fixture paths, not Windows authority.
    p.is_absolute()&&prefix_allowed&&s.len()<=32768&&!s.contains(['\0','\r','\n'])
        &&!p.components().any(|c|matches!(c,std::path::Component::ParentDir))
}
#[cfg(test)]
#[path = "queue_v3_tests.rs"]
mod tests;
