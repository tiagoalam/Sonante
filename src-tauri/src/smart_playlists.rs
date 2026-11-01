//! Saved rules, evaluated on demand. Never scans files or issues playback commands.
use crate::plex::{PlexClient, PlexImageRef};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
const INVALID: &str = "SMART_PLAYLIST_INVALID";
const STORAGE: &str = "SMART_PLAYLIST_STORAGE";
const SOURCE: &str = "SMART_PLAYLIST_SOURCE";
const READ: &str = "SMART_PLAYLIST_READ";
const FILE: &str = "smart-playlists.json";
const MAX_BYTES: u64 = 1_048_576;
static NEXT: AtomicU64 = AtomicU64::new(0);
#[derive(Default)]
pub struct State(pub Mutex<()>);
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    Local {},
    Plex {
        server_id: String,
        library_id: String,
    },
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
    fn validate(&self) -> Result<(), String> {
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
fn load(path: &Path) -> Result<Store, String> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
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
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_BYTES
    {
        return Err(STORAGE.into());
    }
    let mut bytes = vec![];
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
#[tauri::command]
pub async fn smart_playlist_store(
    app: AppHandle,
    action: Action,
) -> Result<Vec<Definition>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let _lock = state.0.lock().map_err(|_| STORAGE)?;
        let lease = app
            .state::<crate::profile_lease::State>()
            .lease()
            .map_err(|_| STORAGE)?;
        lease.validate().map_err(|_| STORAGE)?;
        let path = crate::persistence::sonante_config_dir().join(FILE);
        let mut store = load(&path)?;
        if mutate(&mut store, action)? {
            store.validate()?;
            let bytes = serde_json::to_vec_pretty(&store).map_err(|_| STORAGE)?;
            if bytes.len() as u64 > MAX_BYTES {
                return Err(STORAGE.into());
            }
            lease.validate().map_err(|_| STORAGE)?;
            crate::persistence::atomic_write_private_synced(&path, &bytes, FILE)
                .map_err(|_| STORAGE)?;
        }
        Ok(store.playlists.into_iter().filter(|definition| match definition.rules.source { Source::Local {} => crate::music_sources::local(), Source::Plex {..} => crate::music_sources::plex(), Source::Navidrome {..} => crate::music_sources::navidrome() }).collect())
    })
    .await
    .map_err(|_| STORAGE.to_string())?
}
#[derive(Serialize)]
pub struct Sources {
    local: bool,
    plex_server_id: Option<String>,
    navidrome_account_id: Option<String>,
}
#[tauri::command]
pub async fn smart_playlist_sources(app: AppHandle) -> Result<Sources, String> {
    let (local, plex_server_id, navidrome) = {
        let state = app.state::<crate::ConfigState>();
        let config = state.0.lock().map_err(|_| SOURCE)?;
        let sources = config.sources.unwrap_or_default();
        (sources.local && !config.local_folders.is_empty(), config.plex_server_id.clone().filter(|_| sources.plex), sources.navidrome)
    };
    let navidrome_account_id = if navidrome { crate::navidrome::navidrome_status(app).await.ok().and_then(|s| s.account_id) } else { None };
    Ok(Sources { local, plex_server_id, navidrome_account_id })
}
#[derive(Clone, Serialize)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub genres: Vec<String>,
    pub year: Option<u32>,
    pub added_at: Option<u64>,
    #[serde(skip)]
    pub(crate) favorite_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) image: Option<PlexImageRef>,
}
pub(crate) fn local_added(value: Option<&str>) -> Option<u64> {
    let value = crate::audio::parse_local_album_added_at(value?)?;
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
fn plex_client(app: &AppHandle, server: &str) -> Result<PlexClient, String> {
    let state = app.state::<crate::PlexState>();
    let client = state.0.lock().map_err(|_| SOURCE)?.clone();
    if !client.navigation_server_is_selected(server) {
        return Err(SOURCE.into());
    }
    Ok(client)
}
fn check_source(app: &AppHandle, source: &Source) -> Result<(), String> {
    let enabled = match source { Source::Local {} => crate::music_sources::local(), Source::Plex {..} => crate::music_sources::plex(), Source::Navidrome {..} => crate::music_sources::navidrome() };
    if !enabled { return Err(SOURCE.into()); }
    if let Source::Navidrome { account_id } = source { crate::navidrome::account(account_id).map_err(|_| SOURCE)?; }
    if let Source::Plex { server_id, .. } = source {
        plex_client(app, server_id)?;
    }
    Ok(())
}
async fn catalog(app: &AppHandle, source: &Source) -> Result<Vec<Album>, String> {
    check_source(app, source)?;
    let albums = match source {
        Source::Navidrome { account_id } => crate::navidrome::smart_catalog(account_id).await.map_err(|_| READ)?,
        Source::Local {} => crate::get_local_albums(app.clone(), None)
            .await
            .map_err(|_| READ)?
            .into_iter()
            .map(|a| Album {
                id: a.id,
                title: a.title,
                artist: a.artist,
                genres: a.genres,
                year: a
                    .year
                    .and_then(|v| v.get(..4).and_then(|v| v.parse().ok()))
                    .filter(|y| (1000..=9999).contains(y)),
                added_at: local_added(a.added_at.as_deref()),
                favorite_key: a.folder_path,
                image: None,
            })
            .collect(),
        Source::Plex {
            server_id,
            library_id,
        } => {
            let client = plex_client(app, server_id)?;
            let result = client
                .get_albums(library_id, "titleSort:asc")
                .await
                .map_err(|_| READ)?;
            if result.server_id != *server_id
                || result.library_id != *library_id
                || result.completeness != crate::plex::AlbumCatalogCompleteness::Complete
            {
                return Err(SOURCE.into());
            }
            result
                .albums
                .into_iter()
                .map(|a| Album {
                    id: a.rating_key.clone(),
                    favorite_key: a.rating_key,
                    title: a.title,
                    artist: a.artist,
                    genres: a.genres,
                    year: a.year.filter(|y| (1000..=9999).contains(y)),
                    added_at: a.added_at,
                    image: a.thumb,
                })
                .collect()
        }
    };
    check_source(app, source)?;
    Ok(albums)
}
#[tauri::command]
pub async fn smart_playlist_catalog(app: AppHandle, source: Source) -> Result<Vec<Album>, String> {
    let rules = Rules {
        source: source.clone(),
        genre: None,
        genre_family: None,
        track_mode: TrackMode::Album,
        artist: None,
        year_from: None,
        year_to: None,
        added_days: None,
        favorites_only: false,
        sort: Sort::Album,
        limit: 1,
    };
    rules.validate()?;
    tokio::time::timeout(Duration::from_secs(120), catalog(&app, &source))
        .await
        .map_err(|_| READ.to_string())?
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
#[derive(Serialize)]
pub struct PreviewSelection {
    albums: Vec<Album>,
    matched_albums: usize,
}
async fn preview(app: &AppHandle, r: Rules) -> Result<PreviewSelection, String> {
    r.validate()?;
    let albums = catalog(app, &r.source).await?;
    let mut favorites = HashSet::new();
    if r.favorites_only && matches!(r.source, Source::Navidrome { .. }) {
        if let Source::Navidrome { account_id } = &r.source { favorites.extend(crate::navidrome::smart_favorites(account_id).await.map_err(|_| READ)?); }
    } else if r.favorites_only {
        // Read favorites without rechecking every physical folder or writing migrations.
        let all = tauri::async_runtime::spawn_blocking(|| {
            crate::favorites::FavoriteAlbum::load_all(None)
        })
        .await
        .map_err(|_| READ)?
        .map_err(|_| READ)?;
        for a in &albums {
            if all.iter().any(|f| match &r.source {
                Source::Navidrome { .. } => false,
                Source::Local {} => {
                    f.source == "local"
                        && (f.id == a.id
                            || f.path_or_key == a.favorite_key
                                && normalize(&f.title) == normalize(&a.title))
                }
                Source::Plex { server_id, .. } => {
                    f.source == "plex"
                        && f.path_or_key == a.id
                        && f.plex_image
                            .as_ref()
                            .is_none_or(|image| image.server_id == *server_id)
                }
            }) {
                favorites.insert(a.id.clone());
            }
        }
    }
    let albums = select(albums, &r, &favorites, now());
    let matched_albums = albums.len();
    check_source(app, &r.source)?;
    // Return the ordered selection for native rules. The renderer applies the
    // optional Local genre family with the same classifier used by the editor,
    // before track expansion. It schedules track batches
    // separately, allowing foreground player controls between source reads.
    Ok(PreviewSelection {
        albums,
        matched_albums,
    })
}

#[tauri::command]
pub async fn smart_playlist_preview(
    app: AppHandle,
    rules: Rules,
) -> Result<PreviewSelection, String> {
    tokio::time::timeout(Duration::from_secs(120), preview(&app, rules))
        .await
        .map_err(|_| READ.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rules() -> Rules {
        Rules {
            source: Source::Local {},
            genre: None,
            genre_family: None,
            track_mode: TrackMode::Album,
            artist: None,
            year_from: None,
            year_to: None,
            added_days: None,
            favorites_only: false,
            sort: Sort::Album,
            limit: 100,
        }
    }
    fn album(id: &str, title: &str, year: Option<u32>, added_at: Option<u64>) -> Album {
        Album {
            id: id.into(),
            title: title.into(),
            artist: "Artist".into(),
            genres: vec!["Jazz".into()],
            year,
            added_at,
            favorite_key: id.into(),
            image: None,
        }
    }
    #[test]
    fn local_family_and_track_mode_rules_roundtrip_and_preserve_legacy_exact_meaning() {
        let mut old = serde_json::to_value(rules()).unwrap();
        old.as_object_mut().unwrap().remove("genre_family");
        old.as_object_mut().unwrap().remove("track_mode");
        old["genre"] = serde_json::json!("Jazz / Rock");
        let legacy: Rules = serde_json::from_value(old).unwrap();
        assert_eq!(legacy.genre.as_deref(), Some("Jazz / Rock"));
        assert_eq!(legacy.genre_family, None);
        assert_eq!(legacy.track_mode, TrackMode::Album);
        let mut a = album("1", "A", None, None);
        a.genres = vec!["Jazz".into()];
        assert!(
            !matches(&a, &legacy, &HashSet::new(), 0),
            "old exact tag never becomes a family match"
        );
        a.genres = vec!["Jazz / Rock".into()];
        assert!(matches(&a, &legacy, &HashSet::new(), 0));
        for family in [
            "brazilian",
            "classical",
            "metal",
            "electronic",
            "jazz",
            "blues",
            "reggae",
            "hipHop",
            "funkSoul",
            "folkCountry",
            "rock",
            "pop",
            "latin",
            "world",
            "soundtrack",
            "other",
        ] {
            let mut r = rules();
            r.genre_family = Some(family.into());
            r.track_mode = TrackMode::Varied;
            r.validate().unwrap();
            let restored: Rules = serde_json::from_slice(&serde_json::to_vec(&r).unwrap()).unwrap();
            assert_eq!(restored, r);
            r.genre = Some("Jazz".into());
            assert!(r.validate().is_err());
            r.genre = None;
            r.source = Source::Plex {
                server_id: "server-a".into(),
                library_id: "7".into(),
            };
            assert!(r.validate().is_err());
        }
        for family in ["", "16", "invented", "Jazz"] {
            let mut r = rules();
            r.genre_family = Some(family.into());
            assert!(r.validate().is_err());
        }
        let mut invalid = serde_json::to_value(rules()).unwrap();
        invalid["track_mode"] = serde_json::json!("invented");
        assert!(serde_json::from_value::<Rules>(invalid).is_err());
        let mut store = Store {
            version: 1,
            playlists: vec![],
        };
        let r = Rules {
            genre_family: Some("brazilian".into()),
            track_mode: TrackMode::Varied,
            ..rules()
        };
        mutate(
            &mut store,
            Action::Save {
                id: None,
                name: "Brazil".into(),
                rules: r.clone(),
            },
        )
        .unwrap();
        let restored: Store = serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.playlists[0].rules, r);
    }
    #[test]
    fn rules_are_anded_and_missing_dates_years_do_not_match_filters() {
        let mut r = rules();
        r.genre = Some("jazz".into());
        r.artist = Some(" artist ".into());
        r.year_from = Some(1960);
        r.year_to = Some(1969);
        r.added_days = Some(7);
        r.favorites_only = true;
        let favorites = HashSet::from(["1".into()]);
        let a = album("1", "A", Some(1965), Some(1_000_000));
        assert!(matches(&a, &r, &favorites, 1_100_000));
        assert!(!matches(&a, &r, &HashSet::new(), 1_100_000));
        for a in [
            album("1", "A", None, Some(1_000_000)),
            album("1", "A", Some(1965), None),
            album("1", "A", Some(1970), Some(1_000_000)),
            album("1", "A", Some(1965), Some(2_000_000)),
        ] {
            assert!(!matches(&a, &r, &favorites, 1_100_000));
        }
    }
    #[test]
    fn sorting_is_deterministic_and_missing_dates_stay_last_in_both_directions() {
        let values = vec![
            album("3", "A", Some(1960), None),
            album("2", "A", Some(1960), Some(200)),
            album("1", "A", Some(1960), Some(100)),
        ];
        let mut r = rules();
        r.sort = Sort::AddedNewest;
        assert_eq!(
            select(values.clone(), &r, &HashSet::new(), 300)
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "1", "3"]
        );
        r.sort = Sort::AddedOldest;
        assert_eq!(
            select(values.clone(), &r, &HashSet::new(), 300)
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2", "3"]
        );
        r.sort = Sort::Album;
        assert_eq!(
            select(values, &r, &HashSet::new(), 300)
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2", "3"]
        );
    }
    #[test]
    fn local_added_uses_verified_catalog_utc_not_folder_mtime() {
        assert_eq!(local_added(Some("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(local_added(Some("2000-02-29T12:00:00Z")), Some(951825600));
        assert_eq!(local_added(Some("2026-10-31T00:00:00Z")), Some(1793404800));
        for invalid in [
            "2026-02-29T00:00:00Z",
            "2026-10-31",
            "2026-10-31T00:00:00+00:00",
            "PRIVATE_PATH",
        ] {
            assert_eq!(local_added(Some(invalid)), None);
        }
    }
    #[test]
    fn rule_validation_rejects_mixed_sources_injection_unbounded_and_unknown_fields() {
        let mut r = rules();
        assert!(r.validate().is_ok());
        r.limit = 501;
        assert!(r.validate().is_err());
        r.limit = 100;
        r.year_from = Some(1970);
        r.year_to = Some(1960);
        assert!(r.validate().is_err());
        let r = Rules {
            source: Source::Plex {
                server_id: "server-a".into(),
                library_id: "7/../all".into(),
            },
            ..rules()
        };
        assert!(r.validate().is_err());
        assert!(serde_json::from_value::<Source>(
            serde_json::json!({"kind":"local","server_id":"server-a"})
        )
        .is_err());
        let mut json = serde_json::to_value(rules()).unwrap();
        json["match_any"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Rules>(json).is_err());
    }
    #[test]
    fn rules_persist_without_track_materialization_and_invalid_changes_do_not_replace_them() {
        let mut store = Store {
            version: 1,
            playlists: vec![],
        };
        let r = rules();
        mutate(
            &mut store,
            Action::Save {
                id: None,
                name: " Jazz ".into(),
                rules: r.clone(),
            },
        )
        .unwrap();
        let id = store.playlists[0].id.clone();
        let original = serde_json::to_vec(&store).unwrap();
        assert!(!String::from_utf8_lossy(&original).contains("media_locator"));
        assert!(mutate(
            &mut store,
            Action::Save {
                id: None,
                name: "jazz".into(),
                rules: r.clone()
            }
        )
        .unwrap_err()
        .contains("DUPLICATE"));
        assert_eq!(serde_json::to_vec(&store).unwrap(), original);
        let mut invalid = r.clone();
        invalid.limit = 0;
        assert!(mutate(
            &mut store,
            Action::Save {
                id: Some(id.clone()),
                name: "Invalid".into(),
                rules: invalid
            }
        )
        .is_err());
        assert_eq!(serde_json::to_vec(&store).unwrap(), original);
        mutate(
            &mut store,
            Action::Save {
                id: Some(id.clone()),
                name: "New".into(),
                rules: Rules {
                    source: Source::Plex {
                        server_id: "server-a".into(),
                        library_id: "7".into(),
                    },
                    ..r
                },
            },
        )
        .unwrap();
        store.validate().unwrap();
        let restored: Store = serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.playlists[0].id, id);
        mutate(&mut store, Action::Delete { id }).unwrap();
        assert!(store.playlists.is_empty());
    }
    #[test]
    fn corrupt_or_unsafe_smart_store_is_preserved_and_normal_playlists_are_untouched() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let dir = std::env::temp_dir().join(format!("sonante-smart-owned-{}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let file = dir.join(FILE);
        let normal = dir.join("playlists.json");
        std::fs::write(&normal, b"existing manual playlist fixture").unwrap();
        assert!(load(&file).unwrap().playlists.is_empty());
        std::fs::write(&file, b"corrupt").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"corrupt");
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&normal, &file).unwrap();
        assert!(load(&file).is_err());
        std::fs::remove_file(&file).unwrap();
        let bytes = serde_json::to_vec(&Store {
            version: 1,
            playlists: vec![],
        })
        .unwrap();
        std::fs::write(&file, &bytes).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(&file).is_err());
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load(&file).is_ok());
        assert_eq!(
            std::fs::read(&normal).unwrap(),
            b"existing manual playlist fixture"
        );
        std::fs::remove_file(file).unwrap();
        std::fs::remove_file(normal).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
