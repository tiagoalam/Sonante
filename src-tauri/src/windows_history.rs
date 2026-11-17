//! Windows adapter for the original source-typed listening-history UI/service. No
//! media/artwork/provider reads and no file I/O on the MPD or UI threads.
use crate::audio::MediaLocator;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const ERROR: &str = "HISTORY_UNAVAILABLE";
const LIMIT: usize = 100;
const MAX_BYTES: u64 = 1_048_576;
const MAX_TIME: u64 = 8_640_000_000_000_000;
type Result<T> = std::result::Result<T, &'static str>;
pub const EVENT: &str = "listening-history-changed";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_locator: Option<MediaLocator>,
    #[serde(default)]
    pub thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plex_image: Option<serde_json::Value>,
    #[serde(default)]
    pub duration: Option<f64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub played_at: u64,
    pub track: TrackMetadata,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryFile {
    pub version: u32,
    pub enabled: bool,
    pub entries: Vec<HistoryEntry>,
}
impl Default for HistoryFile {
    fn default() -> Self {
        Self {
            version: 2,
            enabled: true,
            entries: vec![],
        }
    }
}
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(256).collect()
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn catalog_uri(uri: &str) -> bool {
    let parts: Vec<_> = uri.split('/').collect();
    parts.len() == 4
        && parts[0] == "windows"
        && hex(parts[1])
        && hex(parts[3])
        && parts[2]
            .parse::<u32>()
            .is_ok_and(|n| n.to_string() == parts[2])
}
fn sanitized(track:&TrackMetadata)->Option<TrackMetadata>{
 let locator=match track.media_locator.as_ref()?{
  MediaLocator::Local{uri} if catalog_uri(uri)=>MediaLocator::Local{uri:uri.clone()},
  locator@MediaLocator::Navidrome{..}=>crate::navidrome::storage_locator(locator)?,
  locator@MediaLocator::Plex{server_id,part_key,rating_key:Some(rating_key),..}=>{
   let source=crate::windows_mpd::queue_v3::Source::from_locator(locator).ok()?;
   if !part_key.starts_with("/library/parts/")||part_key.contains('%')||part_key.split('/').any(|s|s=="."||s==".."){return None;}
   source.validate().ok()?;MediaLocator::Plex{server_id:server_id.clone(),part_key:part_key.clone(),rating_key:Some(rating_key.clone()),file_path:None}
  },
  _=>return None,
 };
 let plex_image=match (&locator,&track.plex_image){(locator@MediaLocator::Plex{..},Some(raw))=>{
  serde_json::from_value::<crate::plex::PlexImageRef>(raw.clone()).ok().filter(|image|crate::windows_mpd::queue_v3::Source::from_locator(locator).is_ok_and(|source|crate::windows_mpd::queue_v4::valid_image(&source,image))).and_then(|image|serde_json::to_value(image).ok())
 },_=>None};
 Some(TrackMetadata{title:clean(&track.title),artist:clean(&track.artist),album:clean(&track.album),uri:String::new(),media_locator:Some(locator),thumb:None,plex_image,duration:track.duration.filter(|v|v.is_finite()&&*v>=0.)})
}
fn identity(track:&TrackMetadata)->String{
 let key=match track.media_locator.as_ref().unwrap(){
  MediaLocator::Local{uri}=>serde_json::json!(["local",uri]),
  MediaLocator::Navidrome{account_id,song_id,..}=>serde_json::json!(["navidrome",account_id,song_id]),
  MediaLocator::Plex{server_id,part_key,..}=>serde_json::json!(["plex",server_id,part_key]),
  _=>unreachable!(),
 };
 format!("{:x}",Sha256::digest(key.to_string().as_bytes()))
}
fn read_current(path:&Path,legacy:Option<&Path>)->Result<HistoryFile>{
 let source=if std::fs::symlink_metadata(path).is_err_and(|e|e.kind()==std::io::ErrorKind::NotFound){legacy.unwrap_or(path)}else{path};
 read(source,if source==path{2}else{1})
}
fn read(path: &Path, version:u32) -> Result<HistoryFile> {
    crate::persistence::prepare_private_file_for_load(path, "history").map_err(|_| ERROR)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .share_mode(1)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = match options.open(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HistoryFile::default()),
        Err(_) => return Err(ERROR),
    };
    let metadata = file.metadata().map_err(|_| ERROR)?;
    if !metadata.is_file() || crate::persistence::linked(&metadata) || metadata.len() > MAX_BYTES {
        return Err(ERROR);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ERROR);
    }
    let mut data: HistoryFile = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    if data.version != version || data.entries.len() > LIMIT {
        return Err(ERROR);
    }
    let mut ids = HashSet::new();
    for entry in &mut data.entries {
        if version==1&&!matches!(entry.track.media_locator,Some(MediaLocator::Local{..})){return Err(ERROR);}
        entry.track = sanitized(&entry.track).ok_or(ERROR)?;
        if entry.played_at > MAX_TIME
            || entry.id != identity(&entry.track)
            || !ids.insert(entry.id.clone())
        {
            return Err(ERROR);
        }
    }
    data.version=2;
    Ok(data)
}
fn write(path: &Path, data: &HistoryFile) -> Result<()> {
    let bytes = serde_json::to_vec(data).map_err(|_| ERROR)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ERROR);
    }
    crate::persistence::atomic_write_private(path, &bytes, "history").map_err(|_| ERROR)
}
pub struct Store {
    path: PathBuf,
    legacy:Option<PathBuf>,
    last: Option<String>,
    failed: bool,
}
impl Store {
    pub fn new(path: PathBuf) -> Self {Self::with_legacy(path,None)}
    pub fn with_legacy(path:PathBuf,legacy:Option<PathBuf>)->Self{
        Self {
            path,legacy,
            last: None,
            failed: false,
        }
    }
    fn locked(&self) -> Result<std::fs::File> {
        if self.failed {
            return Err(ERROR);
        }
        crate::persistence::operation_lock(&self.path.with_extension("lock")).map_err(|_| ERROR)
    }
    pub fn list(&mut self) -> Result<HistoryFile> {
        let result = (|| {
            let _lock = self.locked()?;
            read_current(&self.path,self.legacy.as_deref())
        })();
        if result.is_err() {
            self.failed = true
        }
        result
    }
    pub fn configure(&mut self, enabled: Option<bool>, clear: bool) -> Result<HistoryFile> {
        let result = (|| {
            let _lock = self.locked()?;
            let mut data = read_current(&self.path,self.legacy.as_deref())?;
            if let Some(v) = enabled {
                data.enabled = v
            }
            if clear {
                data.entries.clear()
            }
            write(&self.path, &data)?;
            Ok(data)
        })();
        if result.is_err() {
            self.failed = true
        }
        result
    }
    pub fn observe(&mut self, state: &str, track: Option<TrackMetadata>, now: u64) -> Result<bool> {
        if state == "stop" {
            self.last = None;
            return Ok(false);
        }
        if state != "play" {
            return Ok(false);
        }
        let Some(track) = track.as_ref().and_then(sanitized) else {
            return Ok(false);
        };
        let id = identity(&track);
        if self.last.as_ref() == Some(&id) {
            return Ok(false);
        }
        self.last = Some(id.clone());
        let result = (|| {
            let _lock = self.locked()?;
            let mut data = read_current(&self.path,self.legacy.as_deref())?;
            if !data.enabled {
                return Ok(false);
            }
            if now > MAX_TIME {
                return Err(ERROR);
            }
            data.entries.retain(|entry| entry.id != id);
            data.entries.insert(
                0,
                HistoryEntry {
                    id,
                    played_at: now,
                    track,
                },
            );
            data.entries.truncate(LIMIT);
            write(&self.path, &data)?;
            Ok(true)
        })();
        if result.is_err() {
            self.failed = true
        }
        result
    }
}
enum Operation {
    Observe {
        state: &'static str,
        track: Option<TrackMetadata>,
        now: u64,
    },
    Query {
        enabled: Option<bool>,
        clear: bool,
        reply: mpsc::Sender<Result<HistoryFile>>,
    },
    Shutdown(mpsc::Sender<()>),
}
#[derive(Clone)]
pub struct Controller {
    sender: mpsc::SyncSender<Operation>,
    stopping: Arc<AtomicBool>,
    fault: Arc<AtomicBool>,
}
impl Controller {
    pub fn new(path: PathBuf, changed: impl Fn() + Send + 'static) -> Result<Self> {Self::with_legacy(path,None,changed)}
    pub fn with_legacy(path:PathBuf,legacy:Option<PathBuf>,changed:impl Fn()+Send+'static)->Result<Self>{
        let (sender, receiver) = mpsc::sync_channel::<Operation>(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let fault = Arc::new(AtomicBool::new(false));
        let worker_fault = fault.clone();
        std::thread::Builder::new()
            .name("sonante-windows-history".into())
            .spawn(move || {
                let mut store = Store::with_legacy(path,legacy);
                let mut notified_fault = false;
                while let Ok(operation) = receiver.recv() {
                    if worker_fault.load(Ordering::Acquire) && !notified_fault {
                        notified_fault = true;
                        changed()
                    }
                    match operation {
                        Operation::Shutdown(reply) => {
                            let _ = reply.send(());
                            break;
                        }
                        Operation::Query {
                            enabled,
                            clear,
                            reply,
                        } => {
                            let result = if worker_fault.load(Ordering::Acquire) {
                                Err(ERROR)
                            } else if enabled.is_some() || clear {
                                store.configure(enabled, clear)
                            } else {
                                store.list()
                            };
                            let changed_ok = result.is_ok() && (enabled.is_some() || clear);
                            if result.is_err() {
                                worker_fault.store(true, Ordering::Release);
                                if !notified_fault {
                                    notified_fault = true;
                                    changed();
                                }
                            }
                            let _ = reply.send(result);
                            if changed_ok {
                                changed()
                            }
                        }
                        Operation::Observe { state, track, now } => {
                            if worker_fault.load(Ordering::Acquire) {
                                continue;
                            }
                            match store.observe(state, track, now) {
                                Ok(true) => changed(),
                                Ok(false) => {}
                                Err(_) => {
                                    worker_fault.store(true, Ordering::Release);
                                    eprintln!("Windows history: persistence_unavailable");
                                    if !notified_fault {
                                        notified_fault = true;
                                        changed()
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .map_err(|_| ERROR)?;
        Ok(Self {
            sender,
            stopping,
            fault,
        })
    }
    pub fn call(&self, enabled: Option<bool>, clear: bool) -> Result<HistoryFile> {
        if self.stopping.load(Ordering::Acquire) || self.fault.load(Ordering::Acquire) {
            return Err(ERROR);
        }
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Operation::Query {
                enabled,
                clear,
                reply,
            })
            .map_err(|_| ERROR)?;
        receiver
            .recv_timeout(Duration::from_secs(4))
            .map_err(|_| ERROR)?
    }
    pub fn publisher(&self) -> Publisher {
        Publisher {
            controller: self.clone(),
            last: None,
        }
    }
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release)
    }
    pub fn shutdown(&self) -> Result<()> {
        self.begin_shutdown();
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Operation::Shutdown(reply))
            .map_err(|_| ERROR)?;
        receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| ERROR)
    }
}
/// Owned solely by the MPD worker. Only bounded, in-memory metadata publication;
/// never waits for a store/file/event or sends anything back to the audio worker.
pub struct Publisher {
    controller: Controller,
    last: Option<String>,
}
impl Publisher {
    pub fn publish(
        &mut self,
        running: bool,
        state: &str,
        warning: bool,
        track: Option<&crate::windows_mpd::library::Track>,
    ) {
        if self.controller.stopping.load(Ordering::Acquire)
            || self.controller.fault.load(Ordering::Acquire)
        {
            return;
        }
        let (state, track, key) = if !running || state == "stop" {
            ("stop", None, "stop".to_owned())
        } else {
            if state != "play" || warning {
                return;
            }
            let Some(track) = track else { return };
            if track.upnp.is_some(){return;}
            let metadata=if let Some(source)=&track.remote_source{
                let Some(lease)=track.remote_lease.as_ref().filter(|lease|lease.healthy()&&lease.source()==source&&track.remote_connection_id.as_deref()==Some(lease.connection_id()))else{return;};let _lease=lease;
                if track.radio.is_some()||!track.path.as_os_str().is_empty()||track.media_root.is_some(){return;}
                let locator=match source{
                    crate::windows_mpd::queue_v3::Source::Navidrome{account_id,song_id}=>MediaLocator::Navidrome{account_id:Some(account_id.clone()),song_id:song_id.clone(),connection_id:String::new()},
                    crate::windows_mpd::queue_v3::Source::Plex{server_id,part_key,rating_key}=>MediaLocator::Plex{server_id:server_id.clone(),part_key:part_key.clone(),rating_key:Some(rating_key.clone()),file_path:None},
                    _=>return,
                };
                TrackMetadata{title:track.title.clone(),artist:track.artist.clone(),album:track.album.clone(),uri:String::new(),media_locator:Some(locator),thumb:None,plex_image:track.plex_image.as_ref().and_then(|image|serde_json::to_value(image).ok()),duration:None}
            }else{
                if track.catalog_album_id.is_none()||track.radio.is_some(){return;}
                let uri=crate::windows_mpd::library::track_uri(track);if !catalog_uri(&uri){return;}
                TrackMetadata{title:track.title.clone(),artist:track.artist.clone(),album:track.album.clone(),uri:String::new(),media_locator:Some(MediaLocator::Local{uri}),thumb:None,plex_image:None,duration:None}
            };
            let Some(metadata)=sanitized(&metadata)else{return;};let key=identity(&metadata);
            ("play",Some(metadata),key)
        };
        if self.last.as_ref() == Some(&key) {
            return;
        }
        let now = match SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|v| u64::try_from(v.as_millis()).ok())
        {
            Some(v) if v <= MAX_TIME => v,
            _ => {
                self.controller.fault.store(true, Ordering::Release);
                eprintln!("Windows history: clock_unavailable");
                return;
            }
        };
        if self
            .controller
            .sender
            .try_send(Operation::Observe { state, track, now })
            .is_ok()
        {
            self.last = Some(key)
        } else {
            // Never silently omit a transition and claim a complete history. Playback
            // remains independent, and reads now report unavailable until restart.
            if !self.controller.fault.swap(true, Ordering::AcqRel) {
                eprintln!("Windows history: observer_unavailable")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let mut b = [0; 12];
            getrandom::fill(&mut b).unwrap();
            let p = std::env::temp_dir().join(format!("sonante-win-history-{b:x?}"));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn file(&self) -> PathBuf {
            self.0.join("history.json")
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn uri(n: u32) -> String {
        format!("windows/{:064x}/1/{n:064x}", 1)
    }
    fn track(n: u32) -> TrackMetadata {
        TrackMetadata {
            title: format!("Faixa {n}"),
            artist: "Artista".into(),
            album: "Álbum".into(),
            uri: "private physical transport MUST NOT persist".into(),
            media_locator: Some(MediaLocator::Local { uri: uri(n) }),
            thumb: Some("private artwork bytes".into()),
            plex_image: Some(serde_json::json!({"path":"PRIVATE"})),
            duration: Some(23.),
        }
    }
    fn native(n: u32) -> crate::windows_mpd::library::Track {
        crate::windows_mpd::library::Track {
            id: format!("{n:064x}"),
            title: format!("Faixa {n}"),
            artist: "Artista".into(),
            album: "Álbum".into(),
            disc: 1,
            number: n,
            file_name: "private.flac".into(),
            catalog_album_id: Some(format!("{:064x}", 1)),
            media_root: None,
            radio: None,upnp:None,
            remote_source: None,
            remote_connection_id: None,
            plex_image:None,remote_lease: None,
            path: PathBuf::from("C:/private/never-opened.flac"),
        }
    }
    #[test]
    fn records_only_playing_deduplicates_pause_seek_ticks_stop_rearms_and_reopens() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        assert!(!s.observe("pause", Some(track(1)), 1).unwrap());
        assert!(!t.file().exists());
        assert!(s.observe("play", Some(track(1)), 2).unwrap());
        for state in ["play", "pause", "play"] {
            assert!(!s.observe(state, Some(track(1)), 3).unwrap())
        }
        assert_eq!(s.list().unwrap().entries[0].played_at, 2);
        s.observe("stop", None, 4).unwrap();
        assert!(s.observe("play", Some(track(1)), 5).unwrap());
        let data = Store::new(t.file()).list().unwrap();
        assert_eq!(data.entries.len(), 1);
        assert_eq!(data.entries[0].played_at, 5)
    }
    #[test]
    fn limit_order_and_revisited_identity_follow_original_policy() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        for n in 0..105 {
            assert!(s.observe("play", Some(track(n)), n as u64).unwrap())
        }
        let data = s.list().unwrap();
        assert_eq!(data.entries.len(), 100);
        assert_eq!(data.entries[0].track.title, "Faixa 104");
        s.observe("play", Some(track(50)), 200).unwrap();
        let data = s.list().unwrap();
        assert_eq!(data.entries.len(), 100);
        assert_eq!(data.entries[0].track.title, "Faixa 50");
        assert_eq!(
            data.entries
                .iter()
                .filter(|e| e.track.title == "Faixa 50")
                .count(),
            1
        )
    }
    #[test]
    fn disable_clear_and_enable_do_not_repopulate_current_track() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        s.observe("play", Some(track(1)), 1).unwrap();
        s.configure(Some(false), false).unwrap();
        assert!(!s.observe("play", Some(track(2)), 2).unwrap());
        s.configure(Some(true), true).unwrap();
        assert!(!s.observe("play", Some(track(2)), 3).unwrap());
        assert!(s.list().unwrap().entries.is_empty());
        s.observe("play", Some(track(3)), 4).unwrap();
        s.configure(None, true).unwrap();
        assert!(!s.observe("play", Some(track(3)), 5).unwrap());
        assert!(s.list().unwrap().entries.is_empty());
        s.observe("stop", None, 6).unwrap();
        assert!(s.observe("play", Some(track(3)), 7).unwrap())
    }
    #[test]
    fn metadata_cleaned_no_artwork_physical_path_credentials_or_session_authority() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        let mut a = track(1);
        a.title = format!("a\n\r\0{}", "♫".repeat(400));
        a.duration = Some(-1.);
        s.observe("play", Some(a), 1).unwrap();
        let bytes = fs::read_to_string(t.file()).unwrap();
        for secret in [
            "PRIVATE",
            "physical transport",
            "artwork bytes",
            "C:",
            "file_name",
            "plex_image",
        ] {
            assert!(!bytes.contains(secret), "{secret}")
        }
        let a = &s.list().unwrap().entries[0].track;
        assert_eq!(a.title.chars().count(), 256);
        assert!(!a.title.chars().any(char::is_control));
        assert_eq!(a.duration, None);
        assert!(a.uri.is_empty());
        assert!(a.thumb.is_none())
    }
    #[test]
    fn opaque_catalog_namespace_rejects_transport_paths_and_invalid_ids() {
        for value in [
            "C:/Music/a.flac",
            "../a.flac",
            "file:///a.flac",
            "https://fixture.invalid/?token=secret",
            "windows-file/abc",
            "windows/abc/1/def",
            &format!("windows/{:064x}/01/{:064x}", 1, 2),
            &format!("windows/{:064x}/1/{:064x}/extra", 1, 2),
        ] {
            let t = Temp::new();
            let mut s = Store::new(t.file());
            let mut a = track(1);
            a.media_locator = Some(MediaLocator::Local { uri: value.into() });
            assert!(!s.observe("play", Some(a), 1).unwrap());
            assert!(!t.file().exists())
        }
        assert!(catalog_uri(&uri(1)));
        // The real Windows catalog admits disk tag 0 as a u32, too.
        assert!(catalog_uri(&format!("windows/{:064x}/0/{:064x}", 1, 2)))
    }
    #[test]
    fn no_remote_or_uncatalogued_track_can_enter_the_local_store() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        let mut a = track(1);
        for locator in [
            None,
            Some(MediaLocator::Radio {
                station_id: "private".into(),
            }),
            Some(MediaLocator::Plex {
                server_id: "server".into(),
                part_key: "/library/parts/1/file".into(),
                rating_key: None,
                file_path: None,
            }),
            Some(MediaLocator::Navidrome {
                connection_id: "SESSION".into(),
                song_id: "song".into(),
                account_id: None,
            }),
        ] {
            a.media_locator = locator;
            assert!(!s.observe("play", Some(a.clone()), 1).unwrap())
        }
        assert!(!t.file().exists())
    }
    #[test]
    fn corrupt_future_oversized_and_adulterated_files_are_preserved_and_error_is_sticky() {
        for mode in 0..7 {
            let t = Temp::new();
            let mut s = Store::new(t.file());
            s.observe("play", Some(track(1)), 1).unwrap();
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(t.file()).unwrap()).unwrap();
            match mode {
                0 => fs::write(t.file(), "invalid").unwrap(),
                1 => {
                    value["version"] = 3.into();
                    fs::write(t.file(), value.to_string()).unwrap()
                }
                2 => {
                    value["entries"][0]["id"] = "forged".into();
                    fs::write(t.file(), value.to_string()).unwrap()
                }
                3 => {
                    value["entries"][0]["played_at"] = (MAX_TIME + 1).into();
                    fs::write(t.file(), value.to_string()).unwrap()
                }
                4 => {
                    let entry = value["entries"][0].clone();
                    value["entries"].as_array_mut().unwrap().push(entry);
                    fs::write(t.file(), value.to_string()).unwrap()
                }
                5 => fs::write(t.file(), vec![b' '; MAX_BYTES as usize + 1]).unwrap(),
                _ => {
                    value["entries"][0]["track"]["media_locator"]["uri"] = "../escape".into();
                    fs::write(t.file(), value.to_string()).unwrap()
                }
            }
            let before = fs::read(t.file()).unwrap();
            assert!(s.list().is_err());
            assert!(s.observe("play", Some(track(2)), 2).is_err());
            assert!(s.configure(Some(false), true).is_err());
            assert_eq!(fs::read(t.file()).unwrap(), before);
            fs::remove_file(t.file()).unwrap();
            assert!(s.list().is_err());
            assert!(Store::new(t.file()).list().unwrap().entries.is_empty())
        }
    }
    #[test]
    fn invalid_directory_target_is_preserved_and_cannot_enable_or_record() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        s.configure(Some(false), false).unwrap();
        fs::rename(t.file(), t.0.join("preserved")).unwrap();
        fs::create_dir(t.file()).unwrap();
        assert!(s.configure(Some(true), false).is_err());
        assert!(s.observe("play", Some(track(1)), 1).is_err());
        assert!(t.file().is_dir());
        let old: HistoryFile =
            serde_json::from_slice(&fs::read(t.0.join("preserved")).unwrap()).unwrap();
        assert!(!old.enabled)
    }
    #[test]
    fn read_revalidates_cross_instance_changes_without_stale_overwrite() {
        let t = Temp::new();
        let mut a = Store::new(t.file());
        let mut b = Store::new(t.file());
        a.list().unwrap();
        b.observe("play", Some(track(2)), 2).unwrap();
        a.observe("play", Some(track(1)), 3).unwrap();
        assert_eq!(b.list().unwrap().entries.len(), 2);
        b.configure(Some(false), false).unwrap();
        assert!(!a.observe("play", Some(track(3)), 4).unwrap());
        assert_eq!(a.list().unwrap().entries.len(), 2)
    }
    #[test]
    fn worker_configuration_is_ordered_with_observations_and_restart_persists() {
        let t = Temp::new();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        let controller = Controller::new(t.file(), move || {
            c.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        let mut p = controller.publisher();
        p.publish(true, "play", false, Some(&native(1)));
        assert_eq!(controller.call(None, false).unwrap().entries.len(), 1);
        controller.call(Some(false), false).unwrap();
        p.publish(true, "play", false, Some(&native(2)));
        assert_eq!(controller.call(None, false).unwrap().entries.len(), 1);
        controller.call(Some(true), true).unwrap();
        p.publish(true, "play", false, Some(&native(2)));
        assert!(controller.call(None, false).unwrap().entries.is_empty());
        p.publish(true, "play", false, Some(&native(3)));
        assert_eq!(controller.call(None, false).unwrap().entries.len(), 1);
        controller.shutdown().unwrap();
        assert!(count.load(Ordering::Relaxed) >= 4);
        assert_eq!(
            Store::new(t.file()).list().unwrap().entries[0].track.title,
            "Faixa 3"
        )
    }
    #[test]
    fn publisher_has_no_startup_media_io_and_only_confirms_clean_live_catalog_play() {
        let t = Temp::new();
        let c = Controller::new(t.file(), || {}).unwrap();
        let mut p = c.publisher();
        p.publish(false, "pause", false, Some(&native(1)));
        p.publish(true, "pause", false, Some(&native(1)));
        p.publish(true, "play", true, Some(&native(1)));
        let mut direct = native(1);
        direct.catalog_album_id = None;
        p.publish(true, "play", false, Some(&direct));
        assert!(c.call(None, false).unwrap().entries.is_empty());
        assert!(!t.file().exists());
        p.publish(true, "play", false, Some(&native(1)));
        for _ in 0..100 {
            p.publish(true, "play", false, Some(&native(1)))
        }
        assert_eq!(c.call(None, false).unwrap().entries.len(), 1);
        c.shutdown().unwrap()
    }
    #[test]
    fn bounded_nonblocking_publisher_reports_saturation_instead_of_silent_success() {
        let t = Temp::new();
        let (sender, receiver) = mpsc::sync_channel(16);
        let c = Controller {
            sender,
            stopping: Arc::new(AtomicBool::new(false)),
            fault: Arc::new(AtomicBool::new(false)),
        };
        let mut p = c.publisher();
        for n in 0..17 {
            p.publish(true, "play", false, Some(&native(n)))
        }
        assert!(c.fault.load(Ordering::Acquire));
        // A saturated store cannot hold up the independent MPD Job shutdown.
        let started = std::time::Instant::now();
        assert!(c.shutdown().is_err());
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(receiver.try_iter().count(), 16);
        assert!(c.call(None, false).is_err());
        assert!(!t.file().exists())
    }
    #[test]
    fn shutdown_fences_publication_and_queries_without_starting_an_engine() {
        let t = Temp::new();
        let c = Controller::new(t.file(), || {}).unwrap();
        let mut p = c.publisher();
        c.begin_shutdown();
        p.publish(true, "play", false, Some(&native(1)));
        assert!(c.call(None, false).is_err());
        c.shutdown().unwrap();
        assert!(!t.file().exists())
    }
    #[test]
    fn concurrent_instance_lock_refuses_without_overwriting_or_claiming_success() {
        let t = Temp::new();
        let mut s = Store::new(t.file());
        s.observe("play", Some(track(1)), 1).unwrap();
        let before = fs::read(t.file()).unwrap();
        let lock = crate::persistence::operation_lock(&t.file().with_extension("lock")).unwrap();
        assert!(s.observe("play", Some(track(2)), 2).is_err());
        assert_eq!(fs::read(t.file()).unwrap(), before);
        drop(lock);
        assert!(s.list().is_err());
        assert_eq!(Store::new(t.file()).list().unwrap().entries.len(), 1);
    }
    #[cfg(windows)]
    #[test]
    fn native_windows_denied_sharing_preserves_existing_history() {
        use std::os::windows::fs::OpenOptionsExt;
        let t = Temp::new();
        let mut s = Store::new(t.file());
        s.observe("play", Some(track(1)), 1).unwrap();
        let before = fs::read(t.file()).unwrap();
        let handle = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(t.file())
            .unwrap();
        assert!(s.observe("play", Some(track(2)), 2).is_err());
        drop(handle);
        assert_eq!(fs::read(t.file()).unwrap(), before);
    }
    #[cfg(unix)]
    #[test]
    fn symlink_target_is_never_read_or_overwritten() {
        let t = Temp::new();
        let target = t.0.join("keep");
        fs::write(&target, "preserved").unwrap();
        std::os::unix::fs::symlink(&target, t.file()).unwrap();
        let mut s = Store::new(t.file());
        assert!(s.list().is_err());
        assert!(s.observe("play", Some(track(1)), 1).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "preserved")
    }
    #[test]
    fn remote_history_uses_stable_source_identity_and_discards_credentials_and_sessions(){
        let t=Temp::new();let mut store=Store::new(t.file());let mut nav=track(1);nav.media_locator=Some(MediaLocator::Navidrome{account_id:Some("a".repeat(64)),song_id:"song-one".into(),connection_id:"PRIVATE_SESSION_ONE".into()});
        assert!(store.observe("play",Some(nav.clone()),1).unwrap());if let Some(MediaLocator::Navidrome{connection_id,..})=&mut nav.media_locator{*connection_id="PRIVATE_SESSION_TWO".into();}assert!(!store.observe("play",Some(nav.clone()),2).unwrap());
        if let Some(MediaLocator::Navidrome{account_id,..})=&mut nav.media_locator{*account_id=Some("b".repeat(64));}assert!(store.observe("play",Some(nav),3).unwrap());
        let mut plex=track(2);plex.media_locator=Some(MediaLocator::Plex{server_id:"server-one".into(),part_key:"/library/parts/10/file.flac".into(),rating_key:Some("12".into()),file_path:Some("PRIVATE_SERVER_PATH".into())});plex.plex_image=Some(serde_json::json!({"server_id":"server-one","path":"/library/metadata/20/thumb/1"}));assert!(store.observe("play",Some(plex.clone()),4).unwrap());
        let original=store.list().unwrap();assert_eq!(original.entries.len(),3);let saved=Store::new(t.file()).list().unwrap();assert_eq!(saved.entries[0].track.plex_image,plex.plex_image);assert_eq!(saved.entries[0].id,original.entries[0].id);
        let bytes=fs::read_to_string(t.file()).unwrap();for denied in ["PRIVATE","physical transport","artwork bytes","connection_id\":\"P","http://","file_path\":\"P"]{assert!(!bytes.contains(denied));}
        store.configure(Some(false),false).unwrap();assert!(!store.observe("play",Some(track(4)),5).unwrap());store.configure(Some(true),true).unwrap();assert!(store.list().unwrap().entries.is_empty());
    }
    #[test]
    fn history_v2_migrates_v1_without_rewriting_and_corruption_never_falls_back(){
        let t=Temp::new();let old=t.0.join("v1.json");let current=t.0.join("v2.json");let mut seed=Store::new(old.clone());seed.observe("play",Some(track(1)),1).unwrap();let mut data:HistoryFile=serde_json::from_slice(&fs::read(&old).unwrap()).unwrap();data.version=1;let bytes=serde_json::to_vec(&data).unwrap();fs::write(&old,&bytes).unwrap();
        let mut store=Store::with_legacy(current.clone(),Some(old.clone()));assert_eq!(store.list().unwrap().entries[0].track.title,"Faixa 1");assert!(!current.exists());assert_eq!(fs::read(&old).unwrap(),bytes);store.configure(None,true).unwrap();assert!(Store::with_legacy(current.clone(),Some(old.clone())).list().unwrap().entries.is_empty());assert_eq!(fs::read(&old).unwrap(),bytes);
        fs::write(&current,b"corrupt current history").unwrap();let mut bad=Store::with_legacy(current.clone(),Some(old.clone()));assert!(bad.list().is_err());assert!(bad.configure(Some(true),true).is_err());assert_eq!(fs::read(&current).unwrap(),b"corrupt current history");assert_eq!(fs::read(&old).unwrap(),bytes);
    }
    struct FixtureLease{source:crate::windows_mpd::queue_v3::Source,live:Arc<AtomicBool>}
    impl crate::windows_mpd::library::RemoteLease for FixtureLease{
        fn uri(&self)->&str{"http://127.0.0.1/PRIVATE_CAPABILITY"}
        fn source(&self)->&crate::windows_mpd::queue_v3::Source{&self.source}
        fn connection_id(&self)->&str{"PRIVATE_SESSION"}
        fn healthy(&self)->bool{self.live.load(Ordering::Acquire)}
        fn command(&self,_:&mut dyn FnMut()->crate::windows_mpd::protocol::Result<Vec<String>>)->crate::windows_mpd::protocol::Result<Vec<String>>{panic!("history must never issue a media command")}
    }
    #[test]
    fn publisher_records_only_confirmed_live_remote_leases_and_never_restored_or_stale_rows(){
        let t=Temp::new();let controller=Controller::new(t.file(),||{}).unwrap();let mut publisher=controller.publisher();let live=Arc::new(AtomicBool::new(true));let mut rows=Vec::new();
        for source in [crate::windows_mpd::queue_v3::Source::Plex{server_id:"server-one".into(),rating_key:"12".into(),part_key:"/library/parts/10/file.flac".into()},crate::windows_mpd::queue_v3::Source::Navidrome{account_id:"a".repeat(64),song_id:"song-one".into()}]{let mut row=native(1);row.catalog_album_id=None;row.path=PathBuf::new();row.remote_source=Some(source.clone());row.remote_connection_id=Some("PRIVATE_SESSION".into());row.remote_lease=Some(Arc::new(FixtureLease{source,live:live.clone()}));rows.push(row);}
        for row in &rows{publisher.publish(false,"pause",false,Some(row));publisher.publish(true,"pause",false,Some(row));publisher.publish(true,"play",true,Some(row));let mut logical=row.clone();logical.remote_lease=None;publisher.publish(true,"play",false,Some(&logical));}
        assert!(controller.call(None,false).unwrap().entries.is_empty());assert!(!t.file().exists());
        for row in &rows{publisher.publish(true,"play",false,Some(row));}assert_eq!(controller.call(None,false).unwrap().entries.len(),2);
        controller.call(None,true).unwrap();publisher.publish(true,"stop",false,None);live.store(false,Ordering::Release);for row in &rows{publisher.publish(true,"play",false,Some(row));}assert!(controller.call(None,false).unwrap().entries.is_empty());controller.shutdown().unwrap();
    }
}
