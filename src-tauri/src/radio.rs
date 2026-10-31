//! Radio Browser directory and private station favorites. No audio/network work at startup.
use crate::audio::{MediaLocator, TrackMetadata};
use reqwest::{Client, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const UNAVAILABLE: &str = "RADIO_DIRECTORY_UNAVAILABLE";
const INVALID: &str = "RADIO_INVALID";
const STORE_ERROR: &str = "RADIO_FAVORITES_UNAVAILABLE";
const PAGE_SIZE: usize = 40;
const MAX_BODY: usize = 1_048_576;
const MAX_FAVORITES: usize = 300;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub country_code: String,
    pub tags: Vec<String>,
    pub codec: String,
    pub bitrate: u32,
    pub favicon: Option<String>,
    pub stream_url: String,
    pub manual: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Search {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub offset: usize,
}
#[derive(Clone, Serialize)]
pub struct Page {
    pub stations: Vec<Station>,
    pub has_more: bool,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct Choice {
    pub name: String,
    #[serde(default)]
    pub stationcount: u64,
}
#[derive(Clone, Serialize)]
pub struct Filters {
    pub countries: Vec<Choice>,
    pub genres: Vec<Choice>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Favorites {
    version: u8,
    stations: Vec<Station>,
}
#[derive(Default)]
pub struct Directory {
    mirrors: Vec<String>,
    discovered: Option<Instant>,
    pages: VecDeque<(Search, Instant, Page)>,
    filters: Option<(Instant, Filters)>,
}
pub struct Store {
    path: PathBuf,
    favorites: Option<Favorites>,
    known: HashMap<String, Station>,
}
pub struct State {
    pub directory: Mutex<Directory>,
    pub store: Mutex<Store>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            directory: Mutex::new(Directory::default()),
            store: Mutex::new(Store::new(
                crate::persistence::sonante_config_dir().join("radio-favorites.json"),
            )),
        }
    }
}
fn label(value: &str, max: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}
pub fn stream_url(raw: &str) -> Result<String, String> {
    if raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err(INVALID.into());
    }
    let url = Url::parse(raw.trim()).map_err(|_| INVALID)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(INVALID.into());
    }
    crate::upnp_protocol::media_url(url.as_str()).map_err(|_| INVALID.to_string())
}
pub fn valid_id(id: &str) -> bool {
    id.len() <= 80
        && (id.strip_prefix("rb:").is_some_and(uuid)
            || id
                .strip_prefix("manual:")
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())))
}
fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}
fn valid_mirror(host: &str) -> bool {
    host.ends_with(".api.radio-browser.info")
        && host.len() < 100
        && host
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-')
}
fn client() -> Result<Client, String> {
    Client::builder()
        .user_agent(concat!("Sonante/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(2))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| UNAVAILABLE.into())
}
async fn json<T: DeserializeOwned>(client: &Client, url: Url) -> Result<T, String> {
    let mut response = client.get(url).send().await.map_err(|_| UNAVAILABLE)?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(UNAVAILABLE.into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| UNAVAILABLE)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(UNAVAILABLE.into());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| UNAVAILABLE.into())
}
#[derive(Deserialize)]
struct Mirror {
    name: String,
}
// Fetch mirror names through the documented HTTPS discovery endpoint. Cache them
// for an hour and rotate the starting mirror; never pin all queries to one server.
async fn mirrors(state: &State, client: &Client) -> Vec<String> {
    if let Ok(directory) = state.directory.lock() {
        if directory
            .discovered
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3600))
        {
            return directory.mirrors.clone();
        }
    }
    let mut hosts = vec![];
    for seed in [
        "all.api.radio-browser.info",
        "de1.api.radio-browser.info",
        "nl1.api.radio-browser.info",
    ] {
        let url = Url::parse(&format!("https://{seed}/json/servers")).unwrap();
        if let Ok(found) = json::<Vec<Mirror>>(client, url).await {
            for mirror in found.into_iter().take(100) {
                if valid_mirror(&mirror.name) && !hosts.contains(&mirror.name) {
                    hosts.push(mirror.name);
                }
            }
        }
        if !hosts.is_empty() {
            break;
        }
    }
    if hosts.is_empty() {
        return vec![
            "all.api.radio-browser.info".into(),
            "de1.api.radio-browser.info".into(),
            "nl1.api.radio-browser.info".into(),
        ];
    }
    let shift = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.subsec_nanos() as usize)
        % hosts.len();
    hosts.rotate_left(shift);
    if let Ok(mut directory) = state.directory.lock() {
        directory.mirrors = hosts.clone();
        directory.discovered = Some(Instant::now());
    }
    hosts
}
async fn request<T: DeserializeOwned>(
    state: &State,
    path: &str,
    query: &[(String, String)],
) -> Result<T, String> {
    let client = client()?;
    for host in mirrors(state, &client).await.into_iter().take(3) {
        let mut url = Url::parse(&format!("https://{host}{path}")).map_err(|_| UNAVAILABLE)?;
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        if let Ok(value) = json(&client, url).await {
            return Ok(value);
        }
    }
    Err(UNAVAILABLE.into())
}
impl Search {
    fn validated(mut self) -> Result<Self, String> {
        self.name = self.name.trim().to_string();
        self.genre = self.genre.trim().to_lowercase();
        self.country = self.country.to_ascii_uppercase();
        if self.offset > 10_000
            || self.offset % PAGE_SIZE != 0
            || self.name.chars().count() > 120
            || self.genre.chars().count() > 80
            || self.name.chars().any(char::is_control)
            || self.genre.chars().any(char::is_control)
            || !self.country.is_empty()
                && !(self.country.len() == 2
                    && self.country.bytes().all(|c| c.is_ascii_uppercase()))
        {
            return Err(INVALID.into());
        }
        Ok(self)
    }
    fn parameters(&self) -> Vec<(String, String)> {
        [
            ("name", self.name.clone()),
            ("countrycode", self.country.clone()),
            ("tag", self.genre.clone()),
            ("offset", self.offset.to_string()),
            ("limit", (PAGE_SIZE + 1).to_string()),
            ("hidebroken", "true".into()),
            ("order", "clickcount".into()),
            ("reverse", "true".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect()
    }
}
#[derive(Deserialize)]
struct RawStation {
    stationuuid: String,
    name: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    countrycode: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    favicon: String,
}
impl RawStation {
    fn into_station(self) -> Option<Station> {
        if !uuid(&self.stationuuid) || label(&self.name, 200).is_empty() {
            return None;
        }
        let stream = if self.url_resolved.is_empty() {
            self.url
        } else {
            self.url_resolved
        };
        Some(Station {
            id: format!("rb:{}", self.stationuuid.to_ascii_lowercase()),
            name: label(&self.name, 200),
            country_code: label(&self.countrycode, 2).to_ascii_uppercase(),
            tags: self
                .tags
                .split(',')
                .map(|s| label(s, 60).to_lowercase())
                .filter(|s| !s.is_empty())
                .take(12)
                .collect(),
            codec: label(&self.codec, 24),
            bitrate: self.bitrate.min(10_000),
            favicon: stream_url(&self.favicon).ok(),
            stream_url: stream_url(&stream).ok()?,
            manual: false,
        })
    }
}
pub async fn search(state: &State, search: Search) -> Result<Page, String> {
    let search = search.validated()?;
    let cached = {
        let directory = state.directory.lock().map_err(|_| UNAVAILABLE)?;
        directory
            .pages
            .iter()
            .find(|(q, at, _)| q == &search && at.elapsed() < Duration::from_secs(300))
            .map(|(_, _, page)| page.clone())
    };
    if let Some(page) = cached {
        state
            .store
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .register(&page.stations);
        return Ok(page);
    }

    let raw: Vec<RawStation> =
        request(state, "/json/stations/search", &search.parameters()).await?;
    if raw.len() > PAGE_SIZE + 1 {
        return Err(UNAVAILABLE.into());
    }
    let has_more = raw.len() > PAGE_SIZE;
    let mut ids = std::collections::HashSet::new();
    let stations: Vec<_> = raw
        .into_iter()
        .take(PAGE_SIZE)
        .filter_map(RawStation::into_station)
        .filter(|station| ids.insert(station.id.clone()))
        .collect();
    {
        let mut store = state.store.lock().map_err(|_| UNAVAILABLE)?;
        store.register(&stations);
    }
    let page = Page { stations, has_more };
    {
        let mut directory = state.directory.lock().map_err(|_| UNAVAILABLE)?;
        directory
            .pages
            .push_front((search, Instant::now(), page.clone()));
        directory.pages.truncate(8);
    }
    Ok(page)
}
pub async fn filters(state: &State) -> Result<Filters, String> {
    if let Ok(directory) = state.directory.lock() {
        if let Some((at, filters)) = &directory.filters {
            if at.elapsed() < Duration::from_secs(3600) {
                return Ok(filters.clone());
            }
        }
    }
    let query = vec![
        ("hidebroken".into(), "true".into()),
        ("order".into(), "stationcount".into()),
        ("reverse".into(), "true".into()),
        ("limit".into(), "200".into()),
    ];
    let countries: Vec<Choice> = request(state, "/json/countrycodes", &[]).await?;
    let genres: Vec<Choice> = request(state, "/json/tags", &query).await?;
    let result = Filters {
        countries: countries
            .into_iter()
            .filter(|c| c.name.len() == 2 && c.name.bytes().all(|x| x.is_ascii_uppercase()))
            .take(300)
            .collect(),
        genres: genres
            .into_iter()
            .take(200)
            .map(|c| Choice {
                name: label(&c.name, 80).to_lowercase(),
                stationcount: c.stationcount,
            })
            .filter(|c| !c.name.is_empty())
            .collect(),
    };
    if let Ok(mut directory) = state.directory.lock() {
        directory.filters = Some((Instant::now(), result.clone()));
    }
    Ok(result)
}
pub async fn count_click(state: &State, id: &str) {
    if let Some(uuid) = id.strip_prefix("rb:").filter(|value| self::uuid(value)) {
        let _: Result<serde_json::Value, _> =
            request(state, &format!("/json/url/{uuid}"), &[]).await;
    }
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            favorites: None,
            known: HashMap::new(),
        }
    }
    fn register(&mut self, stations: &[Station]) {
        if self.known.len() + stations.len() > 400 {
            self.known.clear();
        }
        for station in stations {
            self.known.insert(station.id.clone(), station.clone());
        }
    }
    fn load(&mut self) -> Result<(), String> {
        if self.favorites.is_some() {
            return Ok(());
        }
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.favorites = Some(Favorites {
                    version: 1,
                    stations: vec![],
                });
                return Ok(());
            }
            Err(_) => return Err(STORE_ERROR.into()),
        };
        let meta = file.metadata().map_err(|_| STORE_ERROR)?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.nlink() != 1
            || meta.len() > MAX_BODY as u64
        {
            return Err(STORE_ERROR.into());
        }
        let mut bytes = vec![];
        file.take((MAX_BODY + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| STORE_ERROR)?;
        if bytes.len() > MAX_BODY {
            return Err(STORE_ERROR.into());
        }
        let data: Favorites = serde_json::from_slice(&bytes).map_err(|_| STORE_ERROR)?;
        if data.version != 1 || data.stations.len() > MAX_FAVORITES {
            return Err(STORE_ERROR.into());
        }
        let mut ids = std::collections::HashSet::new();
        for station in &data.stations {
            if !station.valid() || !ids.insert(&station.id) {
                return Err(STORE_ERROR.into());
            }
        }
        self.favorites = Some(data);
        Ok(())
    }
    pub fn list(&mut self) -> Result<Vec<Station>, String> {
        self.load()?;
        Ok(self.favorites.as_ref().unwrap().stations.clone())
    }
    pub fn station(&mut self, id: &str) -> Result<Station, String> {
        if !valid_id(id) {
            return Err(INVALID.into());
        }
        if let Some(station) = self.known.get(id) {
            return Ok(station.clone());
        }
        self.list()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| "RADIO_STATION_EXPIRED".into())
    }
    pub fn favorite(&mut self, id: &str, enabled: bool) -> Result<Vec<Station>, String> {
        if !valid_id(id) {
            return Err(INVALID.into());
        }
        let station = if enabled {
            Some(self.station(id)?)
        } else {
            None
        };
        let mut stations = self.list()?;
        stations.retain(|s| s.id != id);
        if let Some(station) = station {
            stations.push(station);
        }
        self.save(stations)
    }
    pub fn manual(&mut self, name: &str, url: &str) -> Result<Vec<Station>, String> {
        self.save_manual(None, name, url)
    }
    pub fn edit_manual(&mut self, id: &str, name: &str, url: &str) -> Result<Vec<Station>, String> {
        if !valid_id(id) { return Err(INVALID.into()); }
        self.save_manual(Some(id), name, url)
    }
    fn save_manual(&mut self, previous: Option<&str>, name: &str, url: &str) -> Result<Vec<Station>, String> {
        let name = label(name, 200);
        if name.is_empty() {
            return Err(INVALID.into());
        }
        let url = stream_url(url)?;
        let id = format!("manual:{:x}", Sha256::digest(url.as_bytes()));
        let mut stations = self.list()?;
        if let Some(previous) = previous {
            let original = stations.iter().find(|s| s.id == previous).ok_or("RADIO_STATION_EXPIRED")?;
            if !original.manual { return Err(INVALID.into()); }
            if previous != id && stations.iter().any(|s| s.id == id) {
                return Err("RADIO_STATION_DUPLICATE".into());
            }
        }
        let position = previous.and_then(|previous| stations.iter().position(|s| s.id == previous));
        stations.retain(|s| s.id != id && Some(s.id.as_str()) != previous);
        let station = Station {
            id,
            name,
            country_code: String::new(),
            tags: vec![],
            codec: String::new(),
            bitrate: 0,
            favicon: None,
            stream_url: url,
            manual: true,
        };
        stations.insert(position.unwrap_or(stations.len()).min(stations.len()), station);
        self.save(stations)
    }
    fn save(&mut self, stations: Vec<Station>) -> Result<Vec<Station>, String> {
        if stations.len() > MAX_FAVORITES {
            return Err("RADIO_FAVORITES_LIMIT".into());
        }
        let data = Favorites {
            version: 1,
            stations,
        };
        let bytes = serde_json::to_vec(&data).map_err(|_| STORE_ERROR)?;
        if bytes.len() > MAX_BODY {
            return Err("RADIO_FAVORITES_LIMIT".into());
        }
        crate::persistence::atomic_write_private(&self.path, &bytes, "radio-favorites.json")
            .map_err(|_| STORE_ERROR)?;
        self.favorites = Some(data.clone());
        Ok(data.stations)
    }
}
impl Station {
    fn valid(&self) -> bool {
        valid_id(&self.id)
            && !self.name.is_empty()
            && self.name.chars().count() <= 200
            && self.name == label(&self.name, 200)
            && stream_url(&self.stream_url).is_ok()
            && self.tags.len() <= 12
            && self.tags.iter().all(|s| s.chars().count() <= 60)
            && self
                .favicon
                .as_ref()
                .is_none_or(|url| stream_url(url).is_ok())
    }
    pub fn track(&self) -> Result<TrackMetadata, String> {
        if !self.valid() {
            return Err(INVALID.into());
        }
        Ok(TrackMetadata {
            title: self.name.clone(),
            artist: self.name.clone(),
            album: String::new(),
            thumb: self.favicon.clone(),
            plex_image: None,
            media_locator: Some(MediaLocator::Radio {
                station_id: self.id.clone(),
            }),
            uri: stream_url(&self.stream_url)?,
            duration: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn own_store() -> Store {
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("sonante-radio-{}-{id}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        Store::new(root.join("radio-favorites.json"))
    }
    fn station() -> Station {
        RawStation {
            stationuuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            name: "Synthetic Jazz".into(),
            url_resolved: "https://stream.fixture.invalid/live?key=PRIVATE".into(),
            url: String::new(),
            countrycode: "BR".into(),
            tags: "jazz, bossa nova".into(),
            codec: "AAC".into(),
            bitrate: 128,
            favicon: "https://art.fixture.invalid/logo.png".into(),
        }
        .into_station()
        .unwrap()
    }
    #[test]
    fn radio_urls_and_ids_reject_protocol_injection_and_local_files() {
        for url in [
            "file:///etc/passwd",
            "https://user:password@fixture.invalid/live",
            "http://127.0.0.1/live",
            "http://localhost/live",
            "http://169.254.169.254/live",
            "https://fixture.invalid/live\nclear",
            "https://fixture.invalid/live#fragment",
        ] {
            assert!(stream_url(url).is_err(), "{url}");
        }
        assert!(stream_url("https://stream.fixture.invalid/live?key=opaque").is_ok());
        assert!(stream_url("http://stream.fixture.invalid/live").is_ok());
        assert!(valid_id(&station().id));
        for id in [
            "",
            "rb:../bad",
            "manual:short",
            "rb:01234567-89ab-cdef-0123-456789abcdeZ",
        ] {
            assert!(!valid_id(id));
        }
        assert!(valid_mirror("de1.api.radio-browser.info"));
        assert!(!valid_mirror("de1.api.radio-browser.info.evil.invalid"));
        assert!(!valid_mirror("user@de1.api.radio-browser.info"));
    }
    #[test]
    fn radio_queries_are_bounded_and_combine_name_country_genre_and_paging() {
        let query = Search {
            name: "  Jazz & Soul  ".into(),
            country: "br".into(),
            genre: " Bossa Nova ".into(),
            offset: 40,
        }
        .validated()
        .unwrap();
        let mut url = Url::parse("https://api.fixture.invalid/json/stations/search").unwrap();
        url.query_pairs_mut().extend_pairs(query.parameters());
        let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["name"], "Jazz & Soul");
        assert_eq!(pairs["countrycode"], "BR");
        assert_eq!(pairs["tag"], "bossa nova");
        assert_eq!(pairs["offset"], "40");
        assert_eq!(pairs["limit"], "41");
        assert_eq!(pairs["hidebroken"], "true");
        for q in [
            Search {
                offset: 1,
                ..query.clone()
            },
            Search {
                offset: 10_040,
                ..query.clone()
            },
            Search {
                country: "Brazil".into(),
                ..query.clone()
            },
            Search {
                name: "bad\nvalue".into(),
                ..query.clone()
            },
            Search {
                genre: "x".repeat(81),
                ..query.clone()
            },
        ] {
            assert!(q.validated().is_err());
        }
    }
    #[test]
    fn radio_station_parser_sanitizes_optional_metadata_and_preserves_stream_identity() {
        let s = station();
        assert_eq!(s.tags, vec!["jazz", "bossa nova"]);
        assert_eq!(s.country_code, "BR");
        assert!(s.valid());
        let track = s.track().unwrap();
        assert_eq!(
            track.media_locator,
            Some(MediaLocator::Radio { station_id: s.id })
        );
        assert_eq!(track.duration, None);
        assert!(track.uri.contains("PRIVATE"));
        let bad:RawStation=serde_json::from_value(serde_json::json!({"stationuuid":"01234567-89ab-cdef-0123-456789abcdef","name":"Bad","url_resolved":"file:///private"})).unwrap();
        assert!(bad.into_station().is_none());
        let s:RawStation=serde_json::from_value(serde_json::json!({"stationuuid":"01234567-89ab-cdef-0123-456789abcdef","name":"Name\nClean","url":"https://stream.fixture.invalid/live","favicon":"javascript:bad"})).unwrap();
        let s = s.into_station().unwrap();
        assert_eq!(s.name, "NameClean");
        assert!(s.favicon.is_none());
    }
    #[test]
    fn radio_favorites_are_atomic_persistent_and_explicit_never_auto_play() {
        let mut store = own_store();
        assert!(store.list().unwrap().is_empty());
        assert!(!store.path.exists());
        let s = station();
        store.register(std::slice::from_ref(&s));
        assert!(!store.path.exists());
        store.favorite(&s.id, true).unwrap();
        store.favorite(&s.id, true).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        assert_eq!(
            std::fs::metadata(&store.path).unwrap().mode() & 0o777,
            0o600
        );
        let mut reopened = Store::new(store.path.clone());
        assert_eq!(reopened.station(&s.id).unwrap(), s);
        assert_eq!(reopened.favorite(&s.id, false).unwrap().len(), 0);
        assert!(reopened.station(&s.id).is_err());
    }
    #[test]
    fn radio_manual_station_deduplicates_canonical_url_and_remains_private() {
        let mut store = own_store();
        store
            .manual(
                "Synthetic",
                "HTTPS://stream.fixture.invalid/live?key=PRIVATE",
            )
            .unwrap();
        store
            .manual("Renamed", "https://stream.fixture.invalid/live?key=PRIVATE")
            .unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].id.starts_with("manual:"));
        assert_eq!(list[0].name, "Renamed");
        assert!(list[0].manual);
        assert!(!list[0].id.contains("PRIVATE"));
        assert!(store.manual("", "https://stream.fixture.invalid/").is_err());
        assert!(store.manual("Invalid", "file:///private").is_err());
        assert_eq!(store.list().unwrap().len(), 1);
    }
    #[test]
    fn radio_manual_edit_is_atomic_persistent_and_delete_preserves_other_favorites() {
        let mut store = own_store();
        let old = store.manual("Original", "https://fixture.invalid/one").unwrap()[0].clone();
        let other = store.manual("Other", "https://fixture.invalid/other").unwrap()[1].clone();
        let before = store.list().unwrap();
        assert!(store.edit_manual(&old.id, "", "https://fixture.invalid/new").is_err());
        assert!(store.edit_manual(&old.id, "Invalid", "file:///private").is_err());
        assert!(store.edit_manual(&old.id, "Duplicate", &other.stream_url).is_err());
        assert_eq!(store.list().unwrap(), before);
        let edited = store.edit_manual(&old.id, "Renamed", "https://fixture.invalid/new").unwrap();
        assert_eq!(edited.len(), 2);
        assert_eq!(edited[0].name, "Renamed");
        assert_ne!(edited[0].id, old.id);
        assert_eq!(edited[1], other);
        let mut reopened = Store::new(store.path.clone());
        assert_eq!(reopened.list().unwrap(), edited);
        assert!(reopened.station(&old.id).is_err());
        assert_eq!(reopened.favorite(&edited[0].id, false).unwrap(), vec![other.clone()]);
        assert_eq!(Store::new(store.path.clone()).list().unwrap(), vec![other]);
        assert!(reopened.edit_manual(&old.id, "Stale", &old.stream_url).is_err());
    }
    #[test]
    fn radio_manual_edit_refuses_directory_entries_and_failed_write_keeps_confirmed_state() {
        let mut store = own_store();
        let directory = station();
        store.register(std::slice::from_ref(&directory));
        store.favorite(&directory.id, true).unwrap();
        assert!(store.edit_manual(&directory.id, "Replacement", "https://fixture.invalid/new").is_err());
        let manual = store.manual("Manual", "https://fixture.invalid/one").unwrap()[1].clone();
        let before = store.list().unwrap();
        let bytes = std::fs::read(&store.path).unwrap();
        std::fs::rename(&store.path, store.path.with_extension("preserved")).unwrap();
        std::fs::create_dir(&store.path).unwrap();
        assert!(store.edit_manual(&manual.id, "New", "https://fixture.invalid/new").is_err());
        assert_eq!(store.list().unwrap(), before);
        assert_eq!(std::fs::read(store.path.with_extension("preserved")).unwrap(), bytes);
    }
    #[test]
    fn radio_favorites_refuse_symlinks_oversized_and_corrupt_without_replacement() {
        for mode in 0..3 {
            let mut store = own_store();
            if mode == 0 {
                let target = store.path.with_extension("target");
                std::fs::write(&target, b"untouched").unwrap();
                std::os::unix::fs::symlink(&target, &store.path).unwrap();
            }
            if mode == 1 {
                let file = std::fs::File::create(&store.path).unwrap();
                file.set_len(MAX_BODY as u64 + 1).unwrap();
            }
            if mode == 2 {
                std::fs::write(&store.path, b"invalid json").unwrap();
            }
            let old = std::fs::symlink_metadata(&store.path).unwrap();
            assert_eq!(store.list().unwrap_err(), STORE_ERROR);
            assert!(store
                .manual("Name", "https://fixture.invalid/live")
                .is_err());
            assert_eq!(
                std::fs::symlink_metadata(&store.path).unwrap().len(),
                old.len()
            );
        }
    }
    #[test]
    fn radio_favorites_failed_write_keeps_last_confirmed_memory() {
        let mut store = own_store();
        store
            .manual("Original", "https://fixture.invalid/one")
            .unwrap();
        let before = store.list().unwrap();
        std::fs::rename(&store.path, store.path.with_extension("preserved")).unwrap();
        std::fs::create_dir(&store.path).unwrap();
        assert!(store.manual("New", "https://fixture.invalid/two").is_err());
        assert_eq!(store.list().unwrap(), before);
    }
    #[test]
    fn radio_cached_page_re_registers_current_station_without_network() {
        let store = own_store();
        let state = State {
            directory: Mutex::new(Directory::default()),
            store: Mutex::new(store),
        };
        let s = station();
        let query = Search {
            name: "cached".into(),
            country: String::new(),
            genre: String::new(),
            offset: 0,
        };
        state.directory.lock().unwrap().pages.push_back((
            query.clone(),
            Instant::now(),
            Page {
                stations: vec![s.clone()],
                has_more: false,
            },
        ));
        let page = tauri::async_runtime::block_on(search(&state, query)).unwrap();
        assert_eq!(page.stations, vec![s.clone()]);
        assert_eq!(state.store.lock().unwrap().station(&s.id).unwrap(), s);
        assert!(!state.store.lock().unwrap().path.exists());
    }
    #[test]
    fn radio_http_protocol_status_redirect_size_and_invalid_json_are_bounded_and_sanitized() {
        use std::io::{Read, Write};
        for mode in ["ok", "status", "redirect", "length", "chunked", "invalid"] {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let thread = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    if let Ok((stream, _)) = listener.accept() {
                        break stream;
                    }
                    assert!(Instant::now() < deadline, "own HTTP peer was not reached");
                    std::thread::sleep(Duration::from_millis(5));
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = vec![];
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                    assert_eq!(stream.read(&mut byte).unwrap(), 1);
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(
                    request.contains(concat!("user-agent: sonante/", env!("CARGO_PKG_VERSION")))
                );
                let response = match mode {
                    "ok" => "HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}".to_owned(),
                    "status" => "HTTP/1.1 503 Unavailable\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE".to_owned(),
                    "redirect" => format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                    "length" => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_BODY + 1),
                    "chunked" => format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", MAX_BODY + 1, "x".repeat(MAX_BODY + 1)),
                    _ => "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPRIVATE".to_owned(),
                };
                let _ = stream.write_all(response.as_bytes());
            });
            let value = tauri::async_runtime::block_on(json::<serde_json::Value>(
                &client().unwrap(),
                Url::parse(&format!("http://{address}/json/fixture")).unwrap(),
            ));
            thread.join().unwrap();
            if mode == "ok" {
                assert_eq!(value.unwrap()["ok"], true);
            } else {
                assert_eq!(value.unwrap_err(), UNAVAILABLE);
            }
        }
    }
}
