//! Native Windows local favorites. Original UI/service contract; no media/MPD I/O.
//! The Linux store is coupled to POSIX resolution/Plex and stays unchanged.
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs::File, io::Read, path::PathBuf};
type Result<T> = std::result::Result<T, &'static str>;
const FAILED: &str = "windows_favorites_failed";
const INVALID: &str = "windows_favorites_invalid";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ALBUMS: usize = 10000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub source: String,
    pub title: String,
    pub artist: String,
    #[serde(default, deserialize_with = "year")]
    pub year: Option<String>,
    pub path_or_key: String,
    #[serde(default = "exists")]
    pub exists: bool,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub plex_server_id:Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub plex_thumb:Option<crate::plex::PlexImageRef>,
    // Local artwork is deliberately not part of the durable model. The bridge still
    // hydrates original covers with at most two concurrent native reads.
}
fn exists() -> bool {
    true
}
fn year<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<String>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Some(s),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}
impl Album {
    fn validate(&self) -> Result<()> {
        if self.source=="plex" {
            crate::windows_plex::validate_favorite(self)?;
            return Ok(());
        }
        if self.plex_server_id.is_some()||self.plex_thumb.is_some(){return Err(INVALID);}
        let hash = self.path_or_key.strip_prefix("windows/").unwrap_or("");
        if self.source != "local"
            || hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || (self.id != self.path_or_key && self.id != hash)
            || [&self.title, &self.artist]
                .iter()
                .any(|s| s.len() > 16384 || s.contains('\0'))
            || self
                .year
                .as_ref()
                .is_some_and(|s| s.len() > 64 || s.contains('\0'))
        {
            return Err(INVALID);
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u8,
    albums: Vec<Album>,
}
fn validate(albums: &[Album]) -> Result<()> {
    if albums.len() > MAX_ALBUMS {
        return Err(INVALID);
    }
    let mut identities = HashSet::new();
    for album in albums {
        album.validate()?;
        if !identities.insert((&album.id, &album.source,&album.plex_server_id)) {
            return Err(INVALID);
        }
    }
    Ok(())
}
#[derive(Clone)]
pub struct Store {
    path: PathBuf,
    legacy:Option<PathBuf>,
    gate: std::sync::Arc<Gate>,
}
#[derive(Default)]
struct Gate {
    state: std::sync::Mutex<(bool, usize)>,
    changed: std::sync::Condvar,
}
struct Activity(std::sync::Arc<Gate>);
impl Drop for Activity {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.state.lock() {
            state.1 -= 1;
            self.0.changed.notify_all();
        }
    }
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            legacy:None,
            gate: std::sync::Arc::new(Gate::default()),
        }
    }
    pub fn with_legacy(path:PathBuf,legacy:PathBuf)->Self{let mut s=Self::new(path);s.legacy=Some(legacy);s}
    fn admit(&self) -> Result<Activity> {
        let mut state = self.gate.state.lock().map_err(|_| FAILED)?;
        if state.0 {
            return Err("shutting_down");
        }
        if state.1 >= 16 {
            return Err("windows_favorites_busy");
        }
        state.1 += 1;
        Ok(Activity(self.gate.clone()))
    }
    pub fn begin_shutdown(&self) {
        if let Ok(mut state) = self.gate.state.lock() {
            state.0 = true;
        }
    }
    fn drain(&self, timeout: std::time::Duration) -> Result<()> {
        self.begin_shutdown();
        let state = self.gate.state.lock().map_err(|_| FAILED)?;
        let (state, _) = self
            .gate
            .changed
            .wait_timeout_while(state, timeout, |s| s.1 != 0)
            .map_err(|_| FAILED)?;
        if state.1 != 0 {
            return Err(FAILED);
        }
        Ok(())
    }
    pub fn shutdown(&self) -> Result<()> {
        self.drain(std::time::Duration::from_secs(3))
    }
    fn lock(&self) -> Result<File> {
        crate::persistence::operation_lock(&self.path.with_extension("lock")).map_err(|e| {
            if e == "windows_playlist_busy" {
                "windows_favorites_busy"
            } else {
                FAILED
            }
        })
    }
    fn read(&self) -> Result<Option<Vec<Album>>> {
        crate::persistence::prepare_private_file_for_load(&self.path, "favorites.json")
            .map_err(|_| FAILED)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let file = match options.open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return match &self.legacy{Some(path)=>Self::new(path.clone()).read(),None=>Ok(None)},
            Err(_) => return Err(FAILED),
        };
        let metadata = file.metadata().map_err(|_| FAILED)?;
        if !metadata.is_file()
            || crate::persistence::linked(&metadata)
            || metadata.len() > MAX_BYTES as u64
        {
            return Err(INVALID);
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| FAILED)?;
        if bytes.len() > MAX_BYTES {
            return Err(INVALID);
        }
        let doc: Document = serde_json::from_slice(&bytes).map_err(|_| INVALID)?;
        if !matches!(doc.version,1|2) {
            return Err(INVALID);
        }
        validate(&doc.albums)?;
        Ok(Some(doc.albums))
    }
    fn write(&self, albums: Vec<Album>) -> Result<()> {
        validate(&albums)?;
        let bytes =
            serde_json::to_vec_pretty(&Document { version: 2, albums }).map_err(|_| FAILED)?;
        if bytes.len() > MAX_BYTES {
            return Err(INVALID);
        }
        crate::persistence::atomic_write_private(&self.path, &bytes, "favorites.json")
            .map_err(|_| FAILED)
    }
    /// None means migration has not happened. A corrupt file is an error, never
    /// absence: the bridge must not turn it into an empty list or reimport.
    pub fn list(&self) -> Result<Option<Vec<Album>>> {
        let _activity = self.admit()?;
        let _lock = self.lock()?;
        self.read()
    }
    /// Serialized with toggle and other app instances. First valid import wins;
    /// later or stale WebViews cannot resurrect a removed favorite.
    pub fn import(&self, legacy: &str) -> Result<Vec<Album>> {
        let _activity = self.admit()?;
        let _lock = self.lock()?;
        if let Some(albums) = self.read()? {
            return Ok(albums);
        }
        if legacy.len() > MAX_BYTES {
            return Err(INVALID);
        }
        let albums: Vec<Album> = serde_json::from_str(legacy).map_err(|_| INVALID)?;
        validate(&albums)?;
        self.write(albums.clone())?;
        Ok(albums)
    }
    pub fn toggle(&self, album: Album) -> Result<bool> {
        let _activity = self.admit()?;
        album.validate()?;
        let _lock = self.lock()?;
        let mut albums = self.read()?.ok_or("windows_favorites_migration_required")?;
        let found = albums
            .iter()
            .any(|a| a.id == album.id && a.source == album.source && a.plex_server_id==album.plex_server_id);
        if found {
            albums.retain(|a| a.id != album.id || a.source != album.source || a.plex_server_id!=album.plex_server_id);
        } else {
            albums.push(album);
        }
        self.write(albums)?;
        Ok(!found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut random = [0; 12];
            getrandom::fill(&mut random).unwrap();
            let dir = std::env::temp_dir().join(format!("sonante-favorites-{random:x?}"));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> Store {
            Store::new(self.0.join("favorites.json"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn album(n: usize) -> Album {
        Album {
            id: format!("windows/{n:064x}"),
            source: "local".into(),
            title: format!("Álbum {n}"),
            artist: "Artista".into(),
            year: Some("2026".into()),
            path_or_key: format!("windows/{n:064x}"),
            exists: true,
            plex_server_id:None,plex_thumb:None,
        }
    }
    fn ids(albums: Vec<Album>) -> Vec<String> {
        albums.into_iter().map(|a| a.id).collect()
    }
    #[test]
    fn migration_is_one_time_and_preserves_original_fields_order_flexible_year_not_artwork() {
        let f = Fixture::new();
        let s = f.store();
        assert!(s.list().unwrap().is_none());
        let mut rows = serde_json::to_value(vec![album(2), album(1)]).unwrap();
        rows[0]["year"] = serde_json::json!(2025);
        rows[0]["thumb"] = serde_json::json!("data:image/png;base64,old-art");
        rows[0]["plex_image"] = serde_json::json!({"unsupported":"ignored"});
        rows[1]["year"] = serde_json::json!("");
        rows[1]["exists"] = serde_json::json!(false);
        let legacy = serde_json::to_string(&rows).unwrap();
        let output = s.import(&legacy).unwrap();
        assert_eq!(output[0].year.as_deref(), Some("2025"));
        assert!(output[1].year.is_none());
        assert!(!output[1].exists);
        assert_eq!(ids(output), vec![album(2).id, album(1).id]);
        let bytes = fs::read_to_string(&s.path).unwrap();
        assert!(!bytes.contains("thumb"));
        assert!(!bytes.contains("plex_image"));
        assert!(!bytes.contains("old-art"));
        assert_eq!(
            ids(f.store().list().unwrap().unwrap()),
            vec![album(2).id, album(1).id]
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&legacy).unwrap(),
            rows
        );
    }
    #[test]
    fn toggle_retains_id_source_identity_order_confirmation_and_survives_store_reopen() {
        let f = Fixture::new();
        let s = f.store();
        assert_eq!(
            s.toggle(album(1)),
            Err("windows_favorites_migration_required")
        );
        s.import("[]").unwrap();
        assert!(s.toggle(album(2)).unwrap());
        assert!(s.toggle(album(1)).unwrap());
        assert!(!s.toggle(album(2)).unwrap());
        assert!(s.toggle(album(3)).unwrap());
        assert_eq!(
            ids(f.store().list().unwrap().unwrap()),
            vec![album(1).id, album(3).id]
        );
    }
    #[test]
    fn stale_second_import_never_overwrites_native_toggles_or_resurrects_removals() {
        let f = Fixture::new();
        let s = f.store();
        let old = serde_json::to_string(&vec![album(1)]).unwrap();
        s.import(&old).unwrap();
        assert!(!s.toggle(album(1)).unwrap());
        s.toggle(album(2)).unwrap();
        assert_eq!(ids(f.store().import(&old).unwrap()), vec![album(2).id]);
        assert_eq!(
            ids(s.import("not valid legacy").unwrap()),
            vec![album(2).id]
        );
    }
    #[test]
    fn corrupt_future_or_duplicate_destination_is_preserved_never_absence_or_reimport() {
        let f = Fixture::new();
        let s = f.store();
        for bytes in [
            b"not json".to_vec(),
            serde_json::to_vec(&serde_json::json!({"version":3,"albums":[]})).unwrap(),
            serde_json::to_vec(&Document {
                version: 1,
                albums: vec![album(1), album(1)],
            })
            .unwrap(),
        ] {
            fs::write(&s.path, &bytes).unwrap();
            assert!(s.list().is_err());
            assert!(s.import("[]").is_err());
            assert!(s.toggle(album(2)).is_err());
            assert_eq!(fs::read(&s.path).unwrap(), bytes);
        }
    }
    #[test]
    fn invalid_legacy_identity_remote_paths_duplicates_or_size_do_not_create_a_store() {
        let f = Fixture::new();
        let s = f.store();
        let mut bad = album(1);
        bad.path_or_key = "C:/private/audio".into();
        let mut remote = album(2);
        remote.source = "plex".into();
        for legacy in [
            "{bad".into(),
            serde_json::to_string(&vec![bad]).unwrap(),
            serde_json::to_string(&vec![remote]).unwrap(),
            serde_json::to_string(&vec![album(1), album(1)]).unwrap(),
            " ".repeat(MAX_BYTES + 1),
        ] {
            assert!(s.import(&legacy).is_err());
            assert!(!s.path.exists());
            assert!(s.list().unwrap().is_none());
        }
    }
    #[test]
    fn busy_store_is_explicit_and_cannot_modify_the_confirmed_file() {
        let f = Fixture::new();
        let s = f.store();
        s.import("[]").unwrap();
        let before = fs::read(&s.path).unwrap();
        let _held = s.lock().unwrap();
        assert_eq!(f.store().toggle(album(1)), Err("windows_favorites_busy"));
        assert!(s.list().is_err());
        assert_eq!(fs::read(&s.path).unwrap(), before);
    }
    #[test]
    fn separate_instances_serialize_read_modify_replace_without_lost_distinct_additions() {
        let f = Fixture::new();
        let s = f.store();
        s.import("[]").unwrap();
        let jobs = (1..=8)
            .map(|n| {
                let s = s.clone();
                std::thread::spawn(move || {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    loop {
                        match s.toggle(album(n)) {
                            Ok(true) => break,
                            Err("windows_favorites_busy")
                                if std::time::Instant::now() < deadline =>
                            {
                                std::thread::yield_now()
                            }
                            other => panic!("unexpected result {other:?}"),
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        for job in jobs {
            job.join().unwrap();
        }
        let rows = s.list().unwrap().unwrap();
        assert_eq!(rows.len(), 8);
        for n in 1..=8 {
            assert!(rows.iter().any(|a| a.id == album(n).id));
        }
    }
    #[test]
    fn original_card_path_identity_and_legacy_hash_identity_survive_without_rewriting() {
        let f = Fixture::new();
        let s = f.store();
        let current = album(1);
        assert_eq!(current.id, current.path_or_key);
        let mut legacy = album(2);
        legacy.id = format!("{:064x}", 2);
        let rows = vec![current, legacy];
        assert_eq!(
            ids(s.import(&serde_json::to_string(&rows).unwrap()).unwrap()),
            ids(rows)
        );
        let mut foreign = album(3);
        foreign.id = album(4).id;
        assert!(s.toggle(foreign).is_err());
        assert!(validate(&vec![album(0); MAX_ALBUMS + 1]).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn linked_destination_is_rejected_without_reading_or_replacing_the_target() {
        let f = Fixture::new();
        let s = f.store();
        let target = f.0.join("keep");
        fs::write(&target, b"backup").unwrap();
        std::os::unix::fs::symlink(&target, &s.path).unwrap();
        assert!(s.list().is_err());
        assert!(s.import("[]").is_err());
        assert_eq!(fs::read(target).unwrap(), b"backup");
    }
    #[test]
    fn shutdown_fences_new_operations_and_drains_admitted_work_without_holding_io_locks() {
        use std::time::Duration;
        let f = Fixture::new();
        let s = f.store();
        s.import("[]").unwrap();
        let activities = (0..16).map(|_| s.admit().unwrap()).collect::<Vec<_>>();
        assert!(matches!(s.admit(), Err("windows_favorites_busy")));
        s.begin_shutdown();
        assert!(matches!(s.list(), Err("shutting_down")));
        assert_eq!(s.toggle(album(1)), Err("shutting_down"));
        assert!(s.import("[]").is_err());
        let copy = s.clone();
        let (done, rx) = std::sync::mpsc::channel();
        let join = std::thread::spawn(move || done.send(copy.shutdown()).unwrap());
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(20)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        drop(activities);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), Ok(()));
        join.join().unwrap();
        assert!(fs::read_to_string(&s.path).unwrap().contains("albums"));
    }
    #[test]
    fn stalled_activity_has_bounded_shutdown_error_and_no_new_admission() {
        let f = Fixture::new();
        let s = f.store();
        let _held = s.admit().unwrap();
        assert_eq!(s.drain(std::time::Duration::from_millis(10)), Err(FAILED));
        assert!(matches!(s.admit(), Err("shutting_down")));
    }
    #[test]
    fn refused_nonfile_destination_cannot_claim_success_or_destroy_backup() {
        let f = Fixture::new();
        let s = f.store();
        fs::create_dir(&s.path).unwrap();
        fs::write(s.path.join("keep"), b"backup").unwrap();
        assert!(s.import("[]").is_err());
        assert_eq!(fs::read(s.path.join("keep")).unwrap(), b"backup");
    }
}
#[cfg(test)]mod plex_migration_tests{
 use super::*;
 fn plex(server:&str)->Album{Album{id:"123".into(),source:"plex".into(),title:"Fixture".into(),artist:"Fixture".into(),year:None,path_or_key:"123".into(),exists:true,plex_server_id:Some(server.into()),plex_thumb:Some(crate::plex::PlexImageRef{server_id:server.into(),path:"/library/metadata/123/thumb/1".into()})}}
 #[test]fn migration_preserves_legacy_bytes_and_server_scoped_removals_cannot_resurrect(){
  let mut id=[0;12];getrandom::fill(&mut id).unwrap();let root=std::env::temp_dir().join(format!("sonante-favorites-plex-{id:x?}"));std::fs::create_dir(&root).unwrap();
  let legacy=root.join("v1.json");let current=root.join("v2.json");let old=br#"{"version":1,"albums":[]}"#;std::fs::write(&legacy,old).unwrap();
  let store=Store::with_legacy(current.clone(),legacy.clone());assert!(store.list().unwrap().unwrap().is_empty());assert!(!current.exists());assert!(store.toggle(plex("one")).unwrap());assert!(store.toggle(plex("two")).unwrap());assert_eq!(store.list().unwrap().unwrap().len(),2);assert!(!store.toggle(plex("one")).unwrap());assert_eq!(store.list().unwrap().unwrap()[0].plex_server_id.as_deref(),Some("two"));
  let mut bad=plex("two");bad.plex_thumb.as_mut().unwrap().server_id="one".into();assert!(store.toggle(bad).is_err());let mut bad=plex("two");bad.plex_thumb.as_mut().unwrap().path="/library/metadata/123/thumb?X-Plex-Token=private".into();assert!(store.toggle(bad).is_err());
  assert!(!store.toggle(plex("two")).unwrap());assert!(Store::with_legacy(current.clone(),legacy.clone()).list().unwrap().unwrap().is_empty());assert_eq!(std::fs::read(&legacy).unwrap(),old);
  std::fs::write(&current,b"corrupt").unwrap();assert!(store.list().is_err());assert!(store.import("[]").is_err());assert_eq!(std::fs::read(current).unwrap(),b"corrupt");std::fs::remove_dir_all(root).unwrap();
 }
}
