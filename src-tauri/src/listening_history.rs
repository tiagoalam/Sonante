//! Bounded, source-typed listening history. No artwork, catalog or MPD reads.
use crate::audio::{CurrentMedia, MediaLocator, PlaybackStatus, TrackMetadata};
use serde::{Deserialize, Serialize};
use std::{io::Read, os::unix::fs::{MetadataExt, OpenOptionsExt}, path::PathBuf, sync::Mutex, time::{SystemTime, UNIX_EPOCH}};
use sha2::{Digest, Sha256};
const LIMIT: usize = 100;
const MAX_BYTES: u64 = 1_048_576;
const ERROR: &str = "HISTORY_UNAVAILABLE";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry { pub id: String, pub played_at: u64, pub track: TrackMetadata }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryFile { pub version: u32, pub enabled: bool, pub entries: Vec<HistoryEntry> }
impl Default for HistoryFile { fn default() -> Self { Self { version: 1, enabled: true, entries: vec![] } } }
pub struct HistoryStore { path: PathBuf, data: Option<HistoryFile>, last: Option<String>, failed: bool }
pub struct HistoryState(pub Mutex<HistoryStore>);
impl Default for HistoryStore { fn default() -> Self { Self::new(crate::persistence::sonante_config_dir().join("listening-history.json")) } }
fn clean(text: &str) -> String { text.chars().filter(|c| !c.is_control()).take(256).collect() }
fn sanitized(track: &TrackMetadata) -> Option<TrackMetadata> {
    let locator = match &track.media_locator {
        Some(MediaLocator::Local { uri }) if crate::local_media::is_safe_relative_mpd_uri(uri) && crate::local_media::is_catalog_track_uri(uri) => MediaLocator::Local { uri: uri.clone() },
        Some(MediaLocator::Plex { server_id, part_key, rating_key, .. }) if !server_id.is_empty() && server_id.len() <= 256 && part_key.len() <= 4096
            && part_key.starts_with("/library/parts/") && !part_key.contains(['?', '#', '\0', '\r', '\n']) && !crate::plex::contains_plex_token(part_key) =>
            MediaLocator::Plex { server_id: server_id.clone(), part_key: part_key.clone(), rating_key: rating_key.as_ref().filter(|key| !key.is_empty() && key.len() <= 32 && key.bytes().all(|c| c.is_ascii_digit())).cloned(), file_path: None },
        Some(locator @ MediaLocator::Navidrome { .. }) => crate::navidrome::storage_locator(locator)?,
        None if crate::local_media::is_safe_relative_mpd_uri(&track.uri) && crate::local_media::is_catalog_track_uri(&track.uri) => MediaLocator::Local { uri: track.uri.clone() },
        _ => return None,
    };
    Some(TrackMetadata { title: clean(&track.title), artist: clean(&track.artist), album: clean(&track.album), uri: String::new(), media_locator: Some(locator), thumb: None,
        plex_image: track.plex_image.clone().filter(|image| image.is_valid()), duration: track.duration.filter(|d| d.is_finite() && *d >= 0.) })
}
fn identity(track: &TrackMetadata) -> String {
    let key = match track.media_locator.as_ref().unwrap() {
        MediaLocator::Local { uri } => serde_json::json!(["local", uri]),
        MediaLocator::Plex { server_id, part_key, .. } => serde_json::json!(["plex", server_id, part_key]),
        MediaLocator::Navidrome { account_id, song_id, .. } => serde_json::json!(["navidrome", account_id, song_id]),
        _ => unreachable!(),
    };
    format!("{:x}", Sha256::digest(key.to_string().as_bytes()))
}
impl HistoryStore {
    pub fn new(path: PathBuf) -> Self { Self { path, data: None, last: None, failed: false } }
    fn load(&mut self) -> Result<(), String> {
        if self.failed { return Err(ERROR.into()); }
        if self.data.is_some() { return Ok(()); }
        let result = (|| {
            let file = match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(&self.path) {
                Ok(file) => file, Err(e) if e.kind() == std::io::ErrorKind::NotFound => { self.data = Some(HistoryFile::default()); return Ok(()); }, Err(_) => return Err(ERROR.to_string()),
            };
            let meta = file.metadata().map_err(|_| ERROR)?;
            if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 || meta.len() > MAX_BYTES { return Err(ERROR.into()); }
            let mut bytes = Vec::new(); file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| ERROR)?;
            if bytes.len() as u64 > MAX_BYTES { return Err(ERROR.into()); }
            let mut data: HistoryFile = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
            if data.version != 1 || data.entries.len() > LIMIT { return Err(ERROR.into()); }
            let mut ids = std::collections::HashSet::new();
            for entry in &mut data.entries {
                entry.track = sanitized(&entry.track).ok_or(ERROR)?;
                if entry.played_at > 8_640_000_000_000_000 || entry.id != identity(&entry.track) || !ids.insert(entry.id.clone()) { return Err(ERROR.into()); }
            }
            self.data = Some(data); Ok(())
        })();
        if result.is_err() { self.failed = true; }
        result
    }
    fn save(&self, data: &HistoryFile) -> Result<(), String> {
        let bytes = serde_json::to_vec(data).map_err(|_| ERROR)?;
        if bytes.len() as u64 > MAX_BYTES { return Err(ERROR.into()); }
        crate::persistence::atomic_write_private(&self.path, &bytes, "listening-history.json").map_err(|_| ERROR.into())
    }
    pub fn list(&mut self) -> Result<HistoryFile, String> { self.load()?; Ok(self.data.as_ref().unwrap().clone()) }
    pub fn configure(&mut self, enabled: Option<bool>, clear: bool) -> Result<HistoryFile, String> {
        self.load()?;
        let mut data = self.data.as_ref().unwrap().clone();
        if let Some(enabled) = enabled { data.enabled = enabled; if !enabled { self.data.as_mut().unwrap().enabled = false; } }
        if clear { data.entries.clear(); }
        self.save(&data)?; self.data = Some(data.clone());
        // Keep last observed identity after clear, so an ongoing track does not reappear immediately.
        Ok(data)
    }
    pub fn observe(&mut self, state: &str, track: Option<TrackMetadata>, now: u64) -> bool {
        if state == "stop" { self.last = None; return false; }
        if state != "play" { return false; }
        let Some(track) = track.as_ref().and_then(sanitized) else { return false; };
        let id = identity(&track);
        if self.last.as_ref() == Some(&id) { return false; }
        self.last = Some(id.clone());
        if self.load().is_err() || !self.data.as_ref().unwrap().enabled { return false; }
        let mut data = self.data.as_ref().unwrap().clone();
        data.entries.retain(|entry| entry.id != id);
        data.entries.insert(0, HistoryEntry { id, played_at: now, track }); data.entries.truncate(LIMIT);
        // A failed disk write must neither break playback nor trigger retries every status tick.
        if self.save(&data).is_ok() { self.data = Some(data); true }
        else { self.failed = true; eprintln!("[History] persistence=unavailable"); false }
    }
}
pub(crate) fn observed_track(status: &PlaybackStatus, queue: &[TrackMetadata]) -> Option<TrackMetadata> {
    if !status.snapshot_consistent || status.playback_error.is_some() { return None; }
    match status.current_media.as_ref()? {
        CurrentMedia::Local { uri, queue_index } => {
            let track = queue.get(*queue_index)?;
            let matches = match &track.media_locator { Some(MediaLocator::Local { uri: expected }) => uri == expected, None => uri == &track.uri, _ => false };
            matches.then(|| track.clone())
        },
        CurrentMedia::Plex { server_id, part_key, queue_index } => {
            let track = queue.get(*queue_index)?;
            matches!(&track.media_locator, Some(MediaLocator::Plex { server_id: s, part_key: p, .. }) if s == server_id && p == part_key).then(|| track.clone())
        },
        CurrentMedia::Navidrome {connection_id,song_id,queue_index} => {
            let track=queue.get(*queue_index)?;
            matches!(&track.media_locator,Some(MediaLocator::Navidrome {connection_id:c,song_id:s,..}) if c==connection_id && s==song_id).then(||track.clone())
        }, _ => None,
    }
}
pub(crate) fn observe(app: &tauri::AppHandle, status: &PlaybackStatus) {
    use tauri::{Manager, Emitter};
    // Clone only the matching occurrence, release Audio before any file access.
    let track = { let state = app.state::<crate::AudioState>(); let Ok(audio) = state.0.lock() else { return; }; audio.history_track(status) };
    if crate::scrobbling::is_enabled(app) { crate::scrobbling::observe(app, status, track.clone()); }
    if let Some(state) = app.try_state::<HistoryState>() { if let Ok(mut store) = state.0.lock() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |t| t.as_millis() as u64);
        let changed = store.observe(&status.state, track, now);
        drop(store);
        if changed { let _ = app.emit("listening-history-changed", ()); }
    } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn store() -> HistoryStore {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("sonante-058-history-{}-{id}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&path).unwrap(); HistoryStore::new(path.join("listening-history.json"))
    }
    fn local(n: usize) -> TrackMetadata { TrackMetadata { title: format!("Track {n}"), artist: "Artist".into(), album: "Album".into(),
        uri: format!("Source/{n}.flac"), media_locator: None, thumb: Some("private artwork bytes".into()), plex_image: None, duration: Some(23.) } }
    #[test] fn history_records_only_playing_and_deduplicates_ticks_pause_seek() {
        let mut store = store(); store.observe("pause",Some(local(1)),1); assert!(store.list().unwrap().entries.is_empty());
        store.observe("play",Some(local(1)),2); store.observe("play",Some(local(1)),3); store.observe("pause",Some(local(1)),4); store.observe("play",Some(local(1)),5);
        assert_eq!(store.list().unwrap().entries[0].played_at,2);
        store.observe("stop",None,6); store.observe("play",Some(local(1)),7); assert_eq!(store.list().unwrap().entries[0].played_at,7);
        let reopened = HistoryStore::new(store.path.clone()).list().unwrap(); assert_eq!(reopened.entries.len(),1);
        assert!(reopened.entries[0].track.thumb.is_none()); assert!(reopened.entries[0].track.uri.is_empty());
    }
    #[test] fn history_is_bounded_and_disable_clear_do_not_repopulate_current_track() {
        let mut store = store(); for n in 0..105 { store.observe("play",Some(local(n)),n as u64); }
        let data=store.list().unwrap(); assert_eq!(data.entries.len(),100); assert_eq!(data.entries[0].track.title,"Track 104");
        store.configure(Some(false),false).unwrap(); store.observe("play",Some(local(200)),200);
        assert_eq!(store.list().unwrap().entries.len(),100);
        store.configure(Some(true),true).unwrap(); store.observe("play",Some(local(200)),201); assert!(store.list().unwrap().entries.is_empty());
        store.observe("play",Some(local(201)),202); assert_eq!(store.list().unwrap().entries.len(),1);
    }
    #[test] fn history_never_persists_plex_credentials_upnp_or_physical_paths() {
        let mut store=store(); let mut track=local(1);
        track.uri="https://fixture.invalid/?X-Plex-Token=SECRET".into();
        track.media_locator=Some(MediaLocator::Plex{server_id:"server".into(),part_key:"/library/parts/12/file.flac".into(),rating_key:Some("45".into()),file_path:Some("/private/path".into())});
        store.observe("play",Some(track.clone()),1);
        let raw=std::fs::read_to_string(&store.path).unwrap(); assert!(!raw.contains("SECRET")); assert!(!raw.contains("/private/path")); assert!(!raw.contains("artwork bytes"));
        track.media_locator=Some(MediaLocator::Upnp{session_id:"remote".into()}); store.observe("play",Some(track.clone()),2);
        track.media_locator=Some(MediaLocator::Local{uri:"../escape.flac".into()}); store.observe("play",Some(track),3);
        assert_eq!(store.list().unwrap().entries.len(),1);
    }
    #[test] fn history_refuses_symlinks_oversized_or_corrupt_file_without_overwrite() {
        for mode in 0..3 {
            let mut store=store();
            if mode==0 { let target=store.path.with_extension("target"); std::fs::write(&target,"preserved").unwrap(); std::os::unix::fs::symlink(target,&store.path).unwrap(); }
            if mode==1 { let file=std::fs::File::create(&store.path).unwrap();file.set_len(MAX_BYTES+1).unwrap(); }
            if mode==2 { std::fs::write(&store.path,"invalid").unwrap(); }
            let before=std::fs::symlink_metadata(&store.path).unwrap();store.observe("play",Some(local(1)),1);
            assert!(store.list().is_err()); assert_eq!(std::fs::symlink_metadata(&store.path).unwrap().len(),before.len());
        }
    }
    #[test] fn history_failed_write_does_not_retry_or_enable_recording() {
        let mut store=store(); store.load().unwrap();
        std::fs::create_dir(&store.path).unwrap(); store.observe("play",Some(local(1)),1);
        assert!(store.failed); store.observe("play",Some(local(2)),2); assert!(store.path.is_dir());
        let mut another=super::tests::store();another.configure(Some(false),false).unwrap();
        std::fs::rename(&another.path,another.path.with_extension("preserved")).unwrap();std::fs::create_dir(&another.path).unwrap();
        assert!(another.configure(Some(true),false).is_err());assert!(!another.data.as_ref().unwrap().enabled);
    }
    #[test] fn navidrome_history_is_durable_and_deduplicates_reconnected_sessions() {
        let mut history=store();let account="a".repeat(64);let mut track=local(1);
        track.media_locator=Some(MediaLocator::Navidrome{account_id:Some(account.clone()),connection_id:"OLD_SESSION".into(),song_id:"song-a".into()});
        assert!(history.observe("play",Some(track.clone()),1));
        track.media_locator=Some(MediaLocator::Navidrome{account_id:Some(account.clone()),connection_id:"NEW_SESSION".into(),song_id:"song-a".into()});
        assert!(!history.observe("play",Some(track.clone()),2));
        let raw=std::fs::read_to_string(&history.path).unwrap();assert!(!raw.contains("SESSION"));assert!(!raw.contains("private artwork"));
        let saved=HistoryStore::new(history.path.clone()).list().unwrap();assert_eq!(saved.entries.len(),1);assert!(matches!(&saved.entries[0].track.media_locator,Some(MediaLocator::Navidrome{connection_id,..}) if connection_id.is_empty()));
    }

}
