//! Smart playlists using the original Linux rules/store/selection policies.
//! Linux orchestration remains untouched. No MPD/media I/O or remote providers.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const INVALID: &str = "SMART_PLAYLIST_INVALID";
const STORAGE: &str = "SMART_PLAYLIST_STORAGE";
const READ: &str = "SMART_PLAYLIST_READ";
const MAX_BYTES: u64 = 1_048_576;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Local {},
    Plex { server_id: String, library_id: String },
    Navidrome { account_id: String },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    Album,
    Artist,
    AddedNewest,
    AddedOldest,
    YearNewest,
    YearOldest,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrackMode {
    #[default]
    Album,
    Varied,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    pub source: Source,
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre_family: Option<String>,
    #[serde(default)]
    pub track_mode: TrackMode,
    pub artist: Option<String>,
    pub year_from: Option<u32>,
    pub year_to: Option<u32>,
    pub added_days: Option<u32>,
    pub favorites_only: bool,
    pub sort: Sort,
    pub limit: usize,
}
fn safe_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
impl Rules {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let source_ok = match &self.source {
            Source::Local {} => true,
            Source::Navidrome { account_id } => crate::navidrome::valid_account(account_id),
            Source::Plex {
                server_id,
                library_id,
            } => {
                safe_text(server_id, 256)
                    && server_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    && !library_id.is_empty()
                    && library_id.len() <= 32
                    && library_id.bytes().all(|b| b.is_ascii_digit())
            }
        };
        if !source_ok
            || !(1..=500).contains(&self.limit)
            || self.genre_family.as_deref().is_some_and(|family| {
                !matches!(self.source, Source::Local {})
                    || self.genre.is_some()
                    || !matches!(
                        family,
                        "brazilian"
                            | "classical"
                            | "metal"
                            | "electronic"
                            | "jazz"
                            | "blues"
                            | "reggae"
                            | "hipHop"
                            | "funkSoul"
                            | "folkCountry"
                            | "rock"
                            | "pop"
                            | "latin"
                            | "world"
                            | "soundtrack"
                            | "other"
                    )
            })
            || self.genre.as_ref().is_some_and(|v| !safe_text(v, 256))
            || self.artist.as_ref().is_some_and(|v| !safe_text(v, 256))
            || self.year_from.is_some_and(|v| !(1000..=9999).contains(&v))
            || self.year_to.is_some_and(|v| !(1000..=9999).contains(&v))
            || self.year_from.zip(self.year_to).is_some_and(|(a, b)| a > b)
            || self.added_days.is_some_and(|v| !(1..=3650).contains(&v))
        {
            return Err(INVALID.into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub rules: Rules,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    version: u32,
    playlists: Vec<Definition>,
}
impl Store {
    fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        if self.version != 1 || self.playlists.len() > 200 {
            return Err(STORAGE.into());
        }
        for p in &self.playlists {
            if !p.id.starts_with("smart-")
                || !safe_text(&p.id, 100)
                || !safe_text(&p.name, 100)
                || !ids.insert(&p.id)
                || !names.insert(p.name.trim().to_lowercase())
                || p.created_at == 0
                || p.updated_at < p.created_at
                || p.updated_at > 8_640_000_000_000_000
                || p.rules.validate().is_err()
            {
                return Err(STORAGE.into());
            }
        }
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    List,
    Save {
        id: Option<String>,
        name: String,
        rules: Rules,
    },
    Delete {
        id: String,
    },
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}
fn mutate(store: &mut Store, action: Action) -> Result<bool, String> {
    match action {
        Action::List => Ok(false),
        Action::Delete { id } => {
            let len = store.playlists.len();
            store.playlists.retain(|p| p.id != id);
            if len == store.playlists.len() {
                return Err(INVALID.into());
            }
            Ok(true)
        }
        Action::Save { id, name, rules } => {
            rules.validate()?;
            let name = name.trim().to_string();
            if !safe_text(&name, 100) {
                return Err(INVALID.into());
            }
            if store.playlists.iter().any(|p| {
                Some(p.id.as_str()) != id.as_deref()
                    && p.name.trim().to_lowercase() == name.to_lowercase()
            }) {
                return Err("SMART_PLAYLIST_DUPLICATE".into());
            }
            let time = now().saturating_mul(1000).max(1);
            if let Some(id) = id {
                let p = store
                    .playlists
                    .iter_mut()
                    .find(|p| p.id == id)
                    .ok_or(INVALID)?;
                p.name = name;
                p.rules = rules;
                p.updated_at = time.max(p.updated_at.saturating_add(1));
            } else {
                if store.playlists.len() >= 200 {
                    return Err(INVALID.into());
                }
                store.playlists.push(Definition {
                    id: format!("smart-{}-{}", time, NEXT.fetch_add(1, Ordering::Relaxed)),
                    name,
                    created_at: time,
                    updated_at: time,
                    rules,
                });
            }
            Ok(true)
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub genres: Vec<String>,
    pub year: Option<u32>,
    pub added_at: Option<u64>,
    #[serde(skip)]
    pub(crate) favorite_key: String,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub(crate) image: Option<crate::plex::PlexImageRef>,
}
fn normalize(v: &str) -> String {
    v.trim().to_lowercase()
}
fn matches(a: &Album, r: &Rules, favorites: &HashSet<String>, time: u64) -> bool {
    r.genre
        .as_ref()
        .is_none_or(|g| a.genres.iter().any(|v| normalize(v) == normalize(g)))
        && r.artist
            .as_ref()
            .is_none_or(|v| normalize(v) == normalize(&a.artist))
        && r.year_from.is_none_or(|v| a.year.is_some_and(|y| y >= v))
        && r.year_to.is_none_or(|v| a.year.is_some_and(|y| y <= v))
        && r.added_days.is_none_or(|days| {
            a.added_at
                .is_some_and(|t| t <= time && t >= time.saturating_sub(u64::from(days) * 86400))
        })
        && (!r.favorites_only || favorites.contains(&a.id))
}
fn select(mut albums: Vec<Album>, r: &Rules, favorites: &HashSet<String>, time: u64) -> Vec<Album> {
    albums.retain(|a| matches(a, r, favorites, time));
    albums.sort_by(|a, b| {
        let primary = match r.sort {
            Sort::Album => normalize(&a.title).cmp(&normalize(&b.title)),
            Sort::Artist => normalize(&a.artist).cmp(&normalize(&b.artist)),
            Sort::AddedNewest => b.added_at.cmp(&a.added_at),
            Sort::AddedOldest => a
                .added_at
                .is_none()
                .cmp(&b.added_at.is_none())
                .then(a.added_at.cmp(&b.added_at)),
            Sort::YearNewest => b.year.cmp(&a.year),
            Sort::YearOldest => a
                .year
                .is_none()
                .cmp(&b.year.is_none())
                .then(a.year.cmp(&b.year)),
        };
        primary
            .then(normalize(&a.artist).cmp(&normalize(&b.artist)))
            .then(normalize(&a.title).cmp(&normalize(&b.title)))
            .then(a.id.cmp(&b.id))
    });
    albums
}
fn load(path: &Path) -> Result<Store, String> {
    crate::persistence::prepare_private_file_for_load(path, "smart-playlists")
        .map_err(|_| STORAGE)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x00200000);
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Store {
                version: 1,
                playlists: vec![],
            })
        }
        Err(_) => return Err(STORAGE.into()),
    };
    let meta = file.metadata().map_err(|_| STORAGE)?;
    if !meta.is_file() || crate::persistence::linked(&meta) || meta.len() > MAX_BYTES {
        return Err(STORAGE.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| STORAGE)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(STORAGE.into());
    }
    let store: Store = serde_json::from_slice(&bytes).map_err(|_| STORAGE)?;
    store.validate()?;
    Ok(store)
}
fn dispatch(path: &Path, action: Action) -> Result<Vec<Definition>, String> {
    dispatch_with_legacy(path,None,action)
}
fn dispatch_with_legacy(path:&Path,legacy:Option<&Path>,action:Action)->Result<Vec<Definition>,String>{
    let _lock =
        crate::persistence::operation_lock(&path.with_extension("lock")).map_err(|_| STORAGE)?;
    // Current empty/corrupt/linked records remain authoritative. Only an
    // absent current file may read the previous Local-only rules read-only.
    crate::persistence::prepare_private_file_for_load(path,"smart-playlists").map_err(|_|STORAGE)?;
    let source=match (std::fs::symlink_metadata(path),legacy){
        (Err(e),Some(old)) if e.kind()==std::io::ErrorKind::NotFound=>old,
        _=>path,
    };
    let mut store=load(source)?;
    if mutate(&mut store, action)? {
        store.validate()?;
        let bytes = serde_json::to_vec_pretty(&store).map_err(|_| STORAGE)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(STORAGE.into());
        }
        crate::persistence::atomic_write_private(path, &bytes, "smart-playlists")
            .map_err(|_| STORAGE)?;
    }
    Ok(store.playlists)
}
struct Message {
    action: Option<Action>,
    reply: mpsc::Sender<Result<Vec<Definition>, String>>,
}
#[derive(Clone)]
pub struct Controller {
    sender: mpsc::SyncSender<Message>,
    stopping: Arc<AtomicBool>,
}
impl Controller {
    pub fn new(path: PathBuf) -> Result<Self, String> { Self::new_inner(path,None) }
    pub(crate) fn with_legacy(path:PathBuf,legacy:PathBuf)->Result<Self,String>{Self::new_inner(path,Some(legacy))}
    fn new_inner(path:PathBuf,legacy:Option<PathBuf>)->Result<Self,String>{
        let (sender, receiver) = mpsc::sync_channel::<Message>(16);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        std::thread::Builder::new().name("sonante-windows-smart".into()).spawn(move||{
            while let Ok(message)=receiver.recv(){ let Some(action)=message.action else {let _=message.reply.send(Ok(vec![]));break;};
                let result=if stop.load(Ordering::Acquire){Err(STORAGE.into())}else{dispatch_with_legacy(&path,legacy.as_deref(),action)};
                if message.reply.send(result).is_err(){eprintln!("Windows smart playlists: operation_receiver_closed; reread before retry");}
            }
        }).map_err(|_|STORAGE)?;
        Ok(Self { sender, stopping })
    }
    pub fn call(&self, action: Action) -> Result<Vec<Definition>, String> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(STORAGE.into());
        }
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message {
                action: Some(action),
                reply,
            })
            .map_err(|_| STORAGE)?;
        // A timed-out mutation may finish. The original UI offers reread/retry;
        // never retry a mutation automatically or claim it was not written.
        receiver
            .recv_timeout(Duration::from_secs(4))
            .map_err(|_| STORAGE.to_string())?
    }
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) -> Result<(), String> {
        self.begin_shutdown();
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(Message {
                action: None,
                reply,
            })
            .map_err(|_| STORAGE)?;
        receiver
            .recv_timeout(Duration::from_secs(3))
            .map_err(|_| STORAGE.to_string())??;
        Ok(())
    }
}
#[derive(Serialize)]
pub struct Selection {
    pub albums: Vec<Album>,
    pub matched_albums: usize,
}
pub fn catalog(
    library: &crate::windows_mpd::library::Library,
) -> Result<(String, Vec<Album>), String> {
    let (token, rows) = library.complete_album_snapshot().map_err(|_| READ)?;
    let albums = rows
        .into_iter()
        .map(|a| Album {
            favorite_key: format!("windows/{}", a.id),
            id: a.id,
            title: a.title,
            artist: a.artist,
            genres: a.genre.into_iter().collect(),
            year: a.year.filter(|y| (1000..=9999).contains(y)),
            added_at: a.added_at,
            image: None,
        })
        .collect();
    Ok((token, albums))
}
pub fn preview(
    library: &crate::windows_mpd::library::Library,
    favorites: &crate::windows_favorites::Store,
    rules: Rules,
) -> Result<Selection, String> {
    rules.validate()?;
    if !matches!(rules.source, Source::Local {}) { return Err("SMART_PLAYLIST_SOURCE".into()); }
    let (token, albums) = catalog(library)?;
    let all = if rules.favorites_only {
        favorites.list().map_err(|_| READ)?.unwrap_or_default()
    } else {
        vec![]
    };
    let mut ids = HashSet::new();
    for a in &albums {
        if all.iter().any(|f| {
            f.source == "local"
                && (f.id == a.id
                    || f.path_or_key == a.favorite_key
                        && normalize(&f.title) == normalize(&a.title))
        }) {
            ids.insert(a.id.clone());
        }
    }
    let albums = select(albums, &rules, &ids, now());
    let matched_albums = albums.len();
    library.check_album_snapshot(&token).map_err(|_| READ)?;
    Ok(Selection {
        albums,
        matched_albums,
    })
}
/// The catalog is presentation data. Explicit Play still resolves opaque URIs
/// and validates the physical files using the existing admitted transaction.
pub fn epoch(
    library: &crate::windows_mpd::library::Library,
    expected: Option<&str>,
) -> Result<String, String> {
    let token = library.album_snapshot_token().map_err(|_| READ)?;
    if expected.is_some_and(|old| old != token) {
        return Err(READ.into());
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut r = [0; 12];
            getrandom::fill(&mut r).unwrap();
            let p = std::env::temp_dir().join(format!("sonante-smart-{r:x?}"));
            std::fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> PathBuf {
            self.0.join("smart.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn rules() -> Rules {
        serde_json::from_value(json!({"source":{"kind":"local"},"genre":null,"artist":null,"year_from":null,"year_to":null,"added_days":null,"favorites_only":false,"sort":"album","limit":100})).unwrap()
    }
    fn save(name: &str) -> Action {
        Action::Save {
            id: None,
            name: name.into(),
            rules: rules(),
        }
    }
    #[test]
    fn original_rule_validation_and_legacy_defaults() {
        let r = rules();
        assert_eq!(r.track_mode, TrackMode::Album);
        assert!(r.genre_family.is_none());
        r.validate().unwrap();
        for v in [
            json!({"kind":"plex","server_id":"server","library_id":"../1"}),
            json!({"kind":"navidrome","account_id":"invalid"}),
            json!({"kind":"local","server_id":"bad"}),
        ] {
            let invalid = serde_json::from_value::<Source>(v).map(|source| Rules { source, ..rules() }.validate().is_err()).unwrap_or(true);
            assert!(invalid);
        }
        for mut v in [serde_json::to_value(&r).unwrap(); 1] {
            v["limit"] = json!(501);
            assert!(serde_json::from_value::<Rules>(v)
                .unwrap()
                .validate()
                .is_err());
        }
        for (key, value) in [
            ("genre_family", json!("invented")),
            ("artist", json!("bad\nvalue")),
            ("added_days", json!(0)),
            ("year_from", json!(999)),
        ] {
            let mut v = serde_json::to_value(&r).unwrap();
            v[key] = value;
            assert!(serde_json::from_value::<Rules>(v)
                .unwrap()
                .validate()
                .is_err());
        }
        for field in ["match_any", "path", "token"] {
            let mut v = serde_json::to_value(&r).unwrap();
            v[field] = json!(true);
            assert!(serde_json::from_value::<Rules>(v).is_err());
        }
    }
    #[test]
    fn native_rules_crud_cross_instance_reload_no_tracks_or_manual_changes() {
        let f = Fixture::new();
        let manual = f.0.join("playlists.json");
        std::fs::write(&manual, b"manual fixture").unwrap();
        let a = Controller::new(f.path()).unwrap();
        let b = Controller::new(f.path()).unwrap();
        let rows = a.call(save(" Jazz ")).unwrap();
        assert_eq!(rows[0].name, "Jazz");
        let id = rows[0].id.clone();
        assert_eq!(b.call(Action::List).unwrap()[0].id, id);
        b.call(save("Rock")).unwrap();
        assert_eq!(a.call(Action::List).unwrap().len(), 2);
        a.call(Action::Save {
            id: Some(id.clone()),
            name: "Renamed".into(),
            rules: Rules {
                track_mode: TrackMode::Varied,
                genre_family: Some("jazz".into()),
                ..rules()
            },
        })
        .unwrap();
        assert_eq!(
            b.call(Action::List).unwrap()[0].rules.track_mode,
            TrackMode::Varied
        );
        b.call(Action::Delete { id }).unwrap();
        assert_eq!(a.call(Action::List).unwrap().len(), 1);
        let bytes = std::fs::read(f.path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("media_locator"));
        assert_eq!(std::fs::read(manual).unwrap(), b"manual fixture");
        a.shutdown().unwrap();
        b.shutdown().unwrap();
    }
    #[test]
    fn invalid_or_duplicate_mutations_leave_previous_bytes_untouched() {
        let f = Fixture::new();
        dispatch(&f.path(), save("Jazz")).unwrap();
        let before = std::fs::read(f.path()).unwrap();
        for action in [
            save(" jazz "),
            Action::Delete {
                id: "unknown".into(),
            },
            Action::Save {
                id: Some("unknown".into()),
                name: "No".into(),
                rules: rules(),
            },
            Action::Save {
                id: None,
                name: "No".into(),
                rules: Rules {
                    limit: 0,
                    ..rules()
                },
            },
        ] {
            assert!(dispatch(&f.path(), action).is_err());
            assert_eq!(std::fs::read(f.path()).unwrap(), before);
        }
    }
    #[test]
    fn corrupt_future_oversize_duplicate_and_unknown_fields_are_preserved() {
        let f = Fixture::new();
        let mut valid = serde_json::to_value(Store {
            version: 1,
            playlists: vec![],
        })
        .unwrap();
        for bytes in [
            b"corrupt".to_vec(),
            serde_json::to_vec(&json!({"version":2,"playlists":[]})).unwrap(),
            vec![b' '; MAX_BYTES as usize + 1],
            {
                valid["foreign"] = json!(true);
                serde_json::to_vec(&valid).unwrap()
            },
        ] {
            std::fs::write(f.path(), &bytes).unwrap();
            assert!(dispatch(&f.path(), Action::List).is_err());
            assert!(dispatch(&f.path(), save("No")).is_err());
            assert_eq!(std::fs::read(f.path()).unwrap(), bytes);
        }
        std::fs::remove_file(f.path()).unwrap();
        dispatch(&f.path(), save("Jazz")).unwrap();
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(f.path()).unwrap()).unwrap();
        let row = doc["playlists"][0].clone();
        doc["playlists"].as_array_mut().unwrap().push(row);
        let bytes = serde_json::to_vec(&doc).unwrap();
        std::fs::write(f.path(), &bytes).unwrap();
        assert!(dispatch(&f.path(), Action::List).is_err());
        assert_eq!(std::fs::read(f.path()).unwrap(), bytes);
    }
    #[test]
    fn bound_two_hundred_definitions_and_serialized_size() {
        let mut store = Store {
            version: 1,
            playlists: vec![],
        };
        for i in 0..200 {
            mutate(&mut store, save(&format!("P{i}"))).unwrap();
        }
        store.validate().unwrap();
        let bytes = serde_json::to_vec(&store).unwrap();
        assert!(mutate(&mut store, save("Overflow")).is_err());
        assert_eq!(serde_json::to_vec(&store).unwrap(), bytes);
    }
    #[test]
    fn lock_conflict_is_explicit_and_does_not_replace_saved_rules() {
        let f = Fixture::new();
        dispatch(&f.path(), save("Saved")).unwrap();
        let before = std::fs::read(f.path()).unwrap();
        let guard = crate::persistence::operation_lock(&f.path().with_extension("lock")).unwrap();
        assert!(dispatch(&f.path(), save("Other instance")).is_err());
        assert_eq!(std::fs::read(f.path()).unwrap(), before);
        drop(guard);
        assert_eq!(dispatch(&f.path(), Action::List).unwrap().len(), 1);
    }
    #[test]
    fn shutdown_rejects_later_admission_and_reopen_restores_rules() {
        let f = Fixture::new();
        let c = Controller::new(f.path()).unwrap();
        c.call(save("Saved")).unwrap();
        c.shutdown().unwrap();
        assert!(c.call(save("No")).is_err());
        let c = Controller::new(f.path()).unwrap();
        assert_eq!(c.call(Action::List).unwrap()[0].name, "Saved");
        c.shutdown().unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_target_is_refused_without_touching_other_file() {
        let f = Fixture::new();
        let target = f.0.join("elsewhere");
        std::fs::write(&target, b"other fixture").unwrap();
        std::os::unix::fs::symlink(&target, f.path()).unwrap();
        assert!(dispatch(&f.path(), save("No")).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"other fixture");
    }
    #[cfg(windows)]
    #[test]
    fn native_sharing_denial_preserves_target() {
        use std::os::windows::fs::OpenOptionsExt;
        let f = Fixture::new();
        dispatch(&f.path(), save("Saved")).unwrap();
        let _guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(f.path())
            .unwrap();
        assert!(dispatch(&f.path(), Action::List).is_err());
        assert!(dispatch(&f.path(), save("No")).is_err());
    }
    fn library(f: &Fixture) -> crate::windows_mpd::library::Library {
        let root = f.0.join("catalog");
        std::fs::create_dir(&root).unwrap();
        for n in ["A", "B"] {
            std::fs::write(
                root.join(format!("{n}.flac")),
                b"isolated invalid header fixture; no audio",
            )
            .unwrap();
        }
        let l = crate::windows_mpd::library::Library::new(f.0.join("folder.json"));
        l.select(root).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while l.status().unwrap().phase == "scanning" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        l
    }
    #[test]
    fn complete_catalog_is_coherent_dates_unknown_and_rescan_invalidates_token() {
        let f = Fixture::new();
        let l = library(&f);
        let (token, albums) = catalog(&l).unwrap();
        assert!(!albums.is_empty());
        assert!(albums.iter().all(|a| a.added_at.is_none()));
        epoch(&l, Some(&token)).unwrap();
        l.refresh().unwrap();
        assert!(epoch(&l, Some(&token)).is_err());
        l.stop();
        assert!(catalog(&l).is_err());
    }
    #[test]
    fn empty_unconfigured_catalog_is_not_successful_empty_selection() {
        let f = Fixture::new();
        let l = crate::windows_mpd::library::Library::new(f.0.join("folder.json"));
        assert!(catalog(&l).is_err());
        assert!(epoch(&l, None).is_err());
        l.stop();
    }
    #[test]
    fn favorite_selection_uses_native_path_and_original_title_policy_without_cover_reads() {
        let f = Fixture::new();
        let l = library(&f);
        let (_, rows) = catalog(&l).unwrap();
        let a = &rows[0];
        let favorites = crate::windows_favorites::Store::new(f.0.join("favorites.json"));
        favorites.import(&json!([{"id":a.favorite_key,"source":"local","path_or_key":a.favorite_key,"title":a.title,"artist":a.artist}]).to_string()).unwrap();
        let selected = preview(
            &l,
            &favorites,
            Rules {
                favorites_only: true,
                ..rules()
            },
        )
        .unwrap();
        assert_eq!(selected.albums.len(), 1);
        assert_eq!(selected.albums[0].id, a.id);
        assert_eq!(
            preview(
                &l,
                &favorites,
                Rules {
                    added_days: Some(30),
                    ..rules()
                }
            )
            .unwrap()
            .matched_albums,
            0
        );
        l.stop();
        favorites.shutdown().unwrap();
    }
    #[test]
    fn disc_zero_smart_selection_can_be_saved_but_noncanonical_discs_still_fail() {
        use crate::windows_playlists::dispatch;
        let f = Fixture::new();
        let store = crate::playlists::PlaylistStore::new(f.0.join("playlists.json"));
        let item = |disc: &str| json!({"media_locator":{"kind":"local","uri":format!("windows/{}/{disc}/{}","a".repeat(64),"b".repeat(64))},"metadata":{"title":"Zero","artist":"A","album":"B"}});
        assert!(dispatch(
            &store,
            "create_playlist_with_items",
            &json!({"name":"Zero","items":[item("0")]})
        )
        .is_ok());
        let before = std::fs::read(f.0.join("playlists.json")).unwrap();
        for d in ["00", "01", "-1", "4294967296", "1/2"] {
            assert!(dispatch(
                &store,
                "create_playlist_with_items",
                &json!({"name":"No","items":[item(d)]})
            )
            .is_err());
            assert_eq!(std::fs::read(f.0.join("playlists.json")).unwrap(), before);
        }
    }
    #[cfg(not(windows))]
    #[test]
    fn all_sorts_and_combined_filters_match_extracted_original_policy() {
        let mut rows = vec![];
        for i in 0..72 {
            rows.push(Album {
                id: format!("{i:064x}"),
                title: format!("Álbum {}", i % 7),
                artist: format!("Artista {}", i % 3),
                genres: vec![if i % 2 == 0 { "Jazz" } else { "Rock" }.into()],
                year: if i % 4 == 0 {
                    None
                } else {
                    Some(1960 + i % 50)
                },
                added_at: if i % 5 == 0 {
                    None
                } else {
                    Some(900_000 + u64::from(i) * 1000)
                },
                favorite_key: String::new(),
                image: None,
            });
        }
        let favorites: HashSet<String> = rows.iter().step_by(3).map(|a| a.id.clone()).collect();
        for sort in [
            Sort::Album,
            Sort::Artist,
            Sort::AddedNewest,
            Sort::AddedOldest,
            Sort::YearNewest,
            Sort::YearOldest,
        ] {
            for mode in 0..32 {
                let r = Rules {
                    sort,
                    genre: if mode & 1 != 0 {
                        Some(" jazz ".into())
                    } else {
                        None
                    },
                    artist: if mode & 2 != 0 {
                        Some("Artista 1".into())
                    } else {
                        None
                    },
                    year_from: if mode & 4 != 0 { Some(1970) } else { None },
                    added_days: if mode & 8 != 0 { Some(1) } else { None },
                    favorites_only: mode & 16 != 0,
                    ..rules()
                };
                let expected = crate::original_smart_policy::project(
                    serde_json::to_value(&r).unwrap(),
                    serde_json::to_value(&rows).unwrap(),
                    favorites.clone(),
                    970_000,
                );
                let actual =
                    serde_json::to_value(select(rows.clone(), &r, &favorites, 970_000)).unwrap();
                assert_eq!(actual, expected);
            }
        }
    }
}

// Date validation/conversion copied from the original Linux catalog.
pub(crate) fn parse_local_album_added_at(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() != 20 || !bytes.iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        10 => *b == b'T',
        13 | 16 => *b == b':',
        19 => *b == b'Z',
        _ => b.is_ascii_digit(),
    }) { return None; }
    let year = value[..4].parse::<u32>().ok()?;
    let month = value[5..7].parse::<usize>().ok()?;
    let day = value[8..10].parse::<u32>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if year < 1970 || !(1..=12).contains(&month) || day == 0 || day > days[month - 1]
        || value[11..13].parse::<u32>().ok()? > 23
        || value[14..16].parse::<u32>().ok()? > 59
        || value[17..19].parse::<u32>().ok()? > 59 { return None; }
    Some(value.to_owned())
}

pub(crate) fn local_added(value: Option<&str>) -> Option<u64> {
    let value = parse_local_album_added_at(value?)?;
    let y = value[0..4].parse::<i64>().ok()?;
    let m = value[5..7].parse::<i64>().ok()?;
    let d = value[8..10].parse::<i64>().ok()?;
    let y = y - i64::from(m <= 2);
    let era = y / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let h = value[11..13].parse::<i64>().ok()?;
    let min = value[14..16].parse::<i64>().ok()?;
    let sec = value[17..19].parse::<i64>().ok()?;
    u64::try_from(days * 86400 + h * 3600 + min * 60 + sec).ok()
}

/// Shared selection policy; remote catalogs/favorites are supplied only after
/// source-bound, complete reads. This function has no I/O or playback effects.
pub(crate) fn select_remote(albums: Vec<Album>, rules: &Rules, favorites: &HashSet<String>) -> Result<Selection,String> {
    rules.validate()?;
    if matches!(rules.source, Source::Local {}) { return Err("SMART_PLAYLIST_SOURCE".into()); }
    let albums=select(albums,rules,favorites,now());
    let matched_albums=albums.len();Ok(Selection{albums,matched_albums})
}

#[cfg(test)]
mod remote_policy_tests {
 use super::*;
 #[test]
 fn original_remote_rules_validation_and_selection_match_linux_policy() {
  for source in [Source::Plex{server_id:"server-one".into(),library_id:"1".into()},Source::Navidrome{account_id:"a".repeat(64)}] {
   let mut r=Rules{source,genre:Some("Jazz".into()),genre_family:None,track_mode:TrackMode::Varied,artist:None,year_from:Some(1990),year_to:Some(2020),added_days:None,favorites_only:true,sort:Sort::YearNewest,limit:100};
   r.validate().unwrap();
   let albums:Vec<_>=(0..20).map(|i|Album{id:i.to_string(),title:format!("Album{i}"),artist:"Artist".into(),genres:vec![if i%2==0{"Jazz"}else{"Rock"}.into()],year:Some(1980+i*2),added_at:None,favorite_key:String::new(),image:None}).collect();
   let favorites:HashSet<_>=(0..10).map(|i|i.to_string()).collect();
   #[cfg(not(windows))] {
    let expected=crate::original_smart_policy::project(serde_json::to_value(&r).unwrap(),serde_json::to_value(&albums).unwrap(),favorites.clone(),0);
    assert_eq!(serde_json::to_value(select_remote(albums,&r,&favorites).unwrap().albums).unwrap(),expected);
   }
   #[cfg(windows)] {assert_eq!(select_remote(albums,&r,&favorites).unwrap().matched_albums,2);}
   r.genre_family=Some("jazz".into());assert!(r.validate().is_err());
  }
 }
}

#[cfg(test)]
mod migration_tests {
 use super::*;
 fn rule(source:Source)->Rules{Rules{source,genre:None,genre_family:None,track_mode:TrackMode::Album,artist:None,year_from:None,year_to:None,added_days:None,favorites_only:false,sort:Sort::Album,limit:10}}
 #[test]
 fn remote_rules_use_separate_current_file_and_never_overwrite_legacy_or_hide_current_corruption(){
  let mut id=[0;12];getrandom::fill(&mut id).unwrap();let dir=std::env::temp_dir().join(format!("sonante-smart-migration-{id:x?}"));std::fs::create_dir(&dir).unwrap();let old=dir.join("v1.json");let current=dir.join("v2.json");
  dispatch(&old,Action::Save{id:None,name:"Local".into(),rules:rule(Source::Local{})}).unwrap();let old_bytes=std::fs::read(&old).unwrap();
  let host=Controller::with_legacy(current.clone(),old.clone()).unwrap();assert_eq!(host.call(Action::List).unwrap().len(),1);assert!(!current.exists());
  assert_eq!(host.call(Action::Save{id:None,name:"Remote".into(),rules:rule(Source::Navidrome{account_id:"a".repeat(64)})}).unwrap().len(),2);assert_eq!(std::fs::read(&old).unwrap(),old_bytes);host.shutdown().unwrap();
  let current_bytes=std::fs::read(&current).unwrap();std::fs::write(&current,b"{broken").unwrap();let h=Controller::with_legacy(current.clone(),old.clone()).unwrap();assert!(h.call(Action::List).is_err());assert!(h.call(Action::Save{id:None,name:"No".into(),rules:rule(Source::Local{})}).is_err());assert_eq!(std::fs::read(&current).unwrap(),b"{broken");assert_eq!(std::fs::read(&old).unwrap(),old_bytes);h.shutdown().unwrap();
  std::fs::write(&current,b"{\"version\":1,\"playlists\":[]}").unwrap();let h=Controller::with_legacy(current.clone(),old.clone()).unwrap();assert!(h.call(Action::List).unwrap().is_empty());h.shutdown().unwrap();
  std::fs::write(&current,current_bytes).unwrap();assert_eq!(dispatch(&old,Action::List).unwrap().len(),1);assert_eq!(std::fs::read(&old).unwrap(),old_bytes);std::fs::remove_dir_all(dir).unwrap();
 }
}
