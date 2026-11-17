//! Durable logical queue only. Reading this file never starts an engine or opens media.
use super::{
    library::{Selection, Track},
    protocol::Result,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const MAX_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
struct StoredTrack {
    metadata: Track,
    path: PathBuf,
    #[serde(default)]
    root: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedQueue {
    root: PathBuf,
    tracks: Vec<StoredTrack>,
    pub index: usize,
    pub elapsed: f64,
    pub duration: f64,
    pub volume: u8,
    pub album_id: Option<String>,
    #[serde(default)]
    pub options: Option<super::options::PlaybackOptions>,
}
impl SavedQueue {
    pub fn capture(
        root: &Path,
        tracks: &[Track],
        index: usize,
        elapsed: f64,
        duration: f64,
        volume: u8,
        album_id: Option<String>,
    ) -> Self {
        Self {
            root: root.into(),
            tracks: tracks
                .iter()
                .map(|t| StoredTrack {
                    metadata: { let mut logical=t.clone(); logical.remote_lease=None; logical.remote_connection_id=None; logical },
                    path: t.path.clone(),
                    root: if t.remote_source.is_some() { None } else { Some(t.media_root.clone().unwrap_or_else(|| root.to_owned())) },
                })
                .collect(),
            index,
            elapsed,
            duration,
            volume,
            album_id,
            options: None,
        }
    }
    pub fn inserted(
        &self,
        selected: Selection,
        placement: super::queue_edit::Placement,
    ) -> Result<Self> {
        let mut tracks = self.tracks();
        if tracks.len() + selected.tracks.len() > 1000 {
            return Err("queue_limit");
        }
        let count = selected.tracks.len();
        let at = super::queue_edit::position(tracks.len(), Some(self.index), placement)?;
        tracks.splice(at..at, selected.tracks);
        let index = if self.index >= at {
            self.index + count
        } else {
            self.index
        };
        let mut result = Self::capture(
            &self.root,
            &tracks,
            index,
            self.elapsed,
            self.duration,
            self.volume,
            None,
        );
        result.options = self.options;
        result.validate()?;
        Ok(result)
    }
    pub fn edited(&self, action: super::queue_edit::Edit) -> Result<Option<Self>> {
        let mut tracks = self.tracks();
        let plan = super::queue_edit::plan(tracks.len(), Some(self.index), false, action)?;
        let index = super::queue_edit::remap(tracks.len(), self.index, plan);
        let removed_current = plan.to.is_none() && plan.from == self.index;
        super::queue_edit::apply(&mut tracks, plan);
        let Some(index) = index else {
            return Ok(None);
        };
        let mut result = Self::capture(
            &self.root,
            &tracks,
            index,
            if removed_current { 0.0 } else { self.elapsed },
            if removed_current { 0.0 } else { self.duration },
            self.volume,
            None,
        );
        result.options = self.options;
        result.validate()?;
        Ok(Some(result))
    }
    fn logical(&self) -> Result<super::queue_v3::Logical> {
        if self.tracks.iter().any(|t|t.metadata.upnp.is_some()){return Err("queue_restore_failed");}
        if self.tracks.iter().all(|row| row.metadata.remote_source.is_none()) && !valid_path(&self.root) { return Err("queue_restore_failed"); }
        let tracks = self.tracks().iter().map(|track| super::queue_v3::Entry::local(track, &self.root)).collect::<Result<Vec<_>>>()?;
        let value = super::queue_v3::Logical { tracks, index: self.index, elapsed: self.elapsed, duration: self.duration, volume: self.volume, album_id: self.album_id.clone(), options: self.options };
        value.validate()?;
        Ok(value)
    }
    fn from_logical(value: super::queue_v3::Logical) -> Self {
        let root = value.tracks.iter().find_map(|row| match &row.source {
            super::queue_v3::Source::Local { root, .. } => Some(root.clone()), _ => None,
        }).unwrap_or_default();
        let tracks = value.tracks.into_iter().map(|row| {
            let (path, root, remote_source) = match row.source {
                super::queue_v3::Source::Local { root, path } => (path, Some(root), None),
                source => (PathBuf::new(), None, Some(source)),
            };
            let d = row.metadata;
            let metadata = Track { id:d.id,title:d.title,artist:d.artist,album:d.album,disc:d.disc,number:d.number,file_name:d.file_name,catalog_album_id:d.catalog_album_id,path:PathBuf::new(),media_root:None,radio:None,upnp:None,remote_source,remote_connection_id:None,plex_image:None,remote_lease:None };
            StoredTrack { metadata, path, root }
        }).collect();
        Self { root, tracks, index:value.index,elapsed:value.elapsed,duration:value.duration,volume:value.volume,album_id:value.album_id,options:value.options }
    }
    fn validate(&self) -> Result<()> { self.logical().map(|_| ()) }
    pub fn selection(&self) -> Selection {
        Selection {
            root: self.tracks.first().and_then(|t| t.root.clone()).unwrap_or_else(|| self.root.clone()),
            tracks: self.tracks(),
            start: self.index,
            album_id: self.album_id.clone(),
        }
    }
    pub fn tracks(&self) -> Vec<Track> {
        self.tracks
            .iter()
            .map(|row| {
                let mut t = row.metadata.clone();
                t.path = row.path.clone();
                t.media_root = if t.remote_source.is_some() { None } else { Some(row.root.clone().unwrap_or_else(|| self.root.clone())) };
                t
            })
            .collect()
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
// Admission is bounded and refuses reparse/cloud/nonregular objects. Absence
// is separate from a dangling entry or I/O failure, for migration authority.
fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options.custom_flags(FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT).share_mode(FILE_SHARE_READ);
    }
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK); }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return match fs::symlink_metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None), _ => Err("queue_restore_failed"),
        },
        Err(_) => return Err("queue_restore_failed"),
    };
    let meta = file.metadata().map_err(|_| "queue_restore_failed")?;
    if !meta.is_file() || crate::persistence::linked(&meta) || meta.len() > MAX_BYTES { return Err("queue_restore_failed"); }
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN};
        if meta.file_attributes() & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN) != 0 { return Err("queue_restore_failed"); }
    }
    let mut bytes = Vec::new(); file.take(MAX_BYTES+1).read_to_end(&mut bytes).map_err(|_| "queue_restore_failed")?;
    if bytes.len() as u64 > MAX_BYTES { return Err("queue_restore_failed"); }
    Ok(Some(bytes))
}
fn decode(bytes: &[u8], legacy: bool) -> Result<Option<SavedQueue>> {
    if !legacy {
        return super::queue_v4::decode(bytes).map(|queue|queue.map(|(logical,images)|{
            let mut saved=SavedQueue::from_logical(logical);
            for (row,image) in saved.tracks.iter_mut().zip(images){row.metadata.plex_image=image;}
            saved
        }));
    }
    let raw:serde_json::Value=serde_json::from_slice(bytes).map_err(|_|"queue_restore_failed")?;
    let value=if raw["version"]==3 {super::queue_v3::decode(bytes)}else{super::queue_v3::decode_legacy(bytes)}?;
    Ok(value.map(SavedQueue::from_logical))
}
// Compatibility entry point for unversioned fixture paths. Production uses
// open_chain: its current v3 file never accepts a legacy envelope.
#[cfg(test)]
fn load(path: &Path) -> Result<Option<SavedQueue>> {
    let Some(bytes) = read_bytes(path)? else { return Ok(None) };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "queue_restore_failed")?;
    decode(&bytes, value["version"] != 4)
}
fn save(path: &Path, queue: Option<&SavedQueue>) -> Result<()> {
    let logical = queue.map(SavedQueue::logical).transpose().map_err(|_| "queue_save_failed")?;
    let images=queue.map(|q|q.tracks.iter().map(|row|row.metadata.plex_image.clone()).collect::<Vec<_>>()).unwrap_or_default();
    let bytes = super::queue_v4::encode(logical.as_ref(),&images).map_err(|_| "queue_save_failed")?;
    let parent = path.parent().ok_or("queue_save_failed")?;
    fs::create_dir_all(parent).map_err(|_| "queue_save_failed")?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "queue_save_failed")?;
    let temp = parent.join(format!("queue-{:x?}.tmp", random));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| "queue_save_failed")?;
        file.write_all(&bytes).map_err(|_| "queue_save_failed")?;
        file.sync_all().map_err(|_| "queue_save_failed")?;
        drop(file);
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let from: Vec<_> = temp.as_os_str().encode_wide().chain([0]).collect();
            let to: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err("queue_save_failed");
            }
        }
        #[cfg(not(windows))]
        fs::rename(&temp, path).map_err(|_| "queue_save_failed")?;
        Ok(())
    })();
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("queue_save_failed"),
    }
    result
}
pub struct Journal {
    path: PathBuf,
    pub saved: Option<SavedQueue>,
    pub warning: Option<&'static str>,
    last_write: Instant,
    dirty: bool,
}
impl Journal {
    #[cfg(test)]
    pub fn open(path: PathBuf) -> Self { Self::open_migrating(path, None) }
    #[cfg(test)]
    pub fn open_migrating(path: PathBuf, legacy: Option<PathBuf>) -> Self {
        let source = if fs::symlink_metadata(&path).is_err_and(|e|e.kind()==std::io::ErrorKind::NotFound) { legacy.as_deref().unwrap_or(&path) } else { &path };
        Self::loaded(path.clone(), load(source))
    }
    pub fn open_chain(path: PathBuf, legacy: Vec<PathBuf>) -> Self {
        let value = (|| {
            if let Some(bytes) = read_bytes(&path)? { return decode(&bytes, false); }
            for old in legacy {
                if let Some(bytes) = read_bytes(&old)? { return decode(&bytes, true); }
            }
            Ok(None)
        })();
        Self::loaded(path, value)
    }
    fn loaded(path: PathBuf, value: Result<Option<SavedQueue>>) -> Self {
        let (saved, warning) = match value { Ok(q)=>(q,None),Err(e)=>(None,Some(e)) };
        Self { path, saved, warning, last_write:Instant::now(), dirty:false }
    }
    // Confirmed live radio has no durable media queue. A failed write retains
    // the old on-disk checkpoint and exposes dirty/warning as for confirmed edits.
    pub fn observe_checkpoint(&mut self, value: Option<SavedQueue>, force: bool) {
        match value {
            Some(value) => self.observe(value, force),
            // Never automatically replace an unreadable/corrupt checkpoint.
            // Radio admission refuses this state before preparing a candidate.
            None if self.warning == Some("queue_restore_failed") => {},
            None => {
                if self.saved.is_some() {
                    self.observe_empty();
                } else if force || self.last_write.elapsed() >= Duration::from_secs(5) {
                    self.flush();
                }
            }
        }
    }
    // Call only with state observed from the owned MPD after accepted operations.
    pub fn observe(&mut self, value: SavedQueue, force: bool) {
        let cursor_changed = self
            .saved
            .as_ref()
            .is_none_or(|old| old.index != value.index);
        self.saved = Some(value);
        self.dirty = true;
        if force || cursor_changed || self.last_write.elapsed() >= Duration::from_secs(5) {
            self.flush()
        }
    }
    // Only after owned MPD proof confirms an empty queue or a transient source. Unlike a
    // logical edit, this effect already happened and cannot be rolled back by
    // pretending the old queue still exists. Preserve dirty state on failure.
    pub(super) fn observe_empty(&mut self) {
        self.saved = None;
        self.dirty = true;
        self.flush();
    }
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.last_write = Instant::now();
        match save(&self.path, self.saved.as_ref()) {
            Ok(()) => {
                self.dirty = false;
                self.warning = None
            }
            Err(e) => self.warning = Some(e),
        }
    }
    pub fn replace_logical(&mut self, value: Option<SavedQueue>) -> Result<()> {
        save(&self.path, value.as_ref())?;
        self.saved = value;
        self.warning = None;
        self.dirty = false;
        self.last_write = Instant::now();
        Ok(())
    }
    pub fn clear(&mut self) -> Result<()> {
        self.replace_logical(None)
    }
}
#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
