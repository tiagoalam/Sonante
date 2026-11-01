//! Optional first-phase source. Initialized only on explicit Navidrome navigation.
mod api;
mod discovery;
mod library;
pub(crate) use library::{bind_tracks, with_bound_tracks, storage_locator, valid_account, account, smart_catalog, smart_favorites, available, resolve_saved};
mod favorites;
mod proxy;
mod store;
use crate::audio::{MediaLocator, TrackMetadata};
use api::{Album, Credentials, Song};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tauri::{AppHandle, Manager};
pub const INVALID: &str = "NAVIDROME_INVALID";
pub const AUTH: &str = "NAVIDROME_AUTH";
pub const PROTOCOL: &str = "NAVIDROME_PROTOCOL";
pub const UNAVAILABLE: &str = "NAVIDROME_UNAVAILABLE";
pub const STORAGE: &str = "NAVIDROME_STORAGE";
const CHANGED: &str = "NAVIDROME_CONTEXT_CHANGED";
struct Connection {
    credentials: Credentials,
    client: reqwest::Client,
    port: u16,
    secret: String,
    live: AtomicBool,
    stop: Arc<tokio::sync::Notify>,
    covers: tokio::sync::Semaphore,
    details: Mutex<VecDeque<Detail>>,
    catalog: tokio::sync::Mutex<Option<(std::time::Instant, Vec<crate::smart_playlists::Album>)>>,
}
impl Connection {
    fn active(&self) -> bool {
        self.live.load(Ordering::Acquire) && crate::music_sources::navidrome()
    }
    fn retire(&self) {
        self.live.store(false, Ordering::Release);
        self.stop.notify_one();
    }
    fn stream(&self, song: &str) -> Result<String, String> {
        if !self.active() || !api::id(song) {
            return Err(CHANGED.into());
        }
        Ok(format!(
            "http://127.0.0.1:{}/nav/{}/{}",
            self.port, self.secret, song
        ))
    }
    fn open(credentials: Credentials) -> Result<Arc<Self>, String> {
        credentials.validate()?;
        let (listener, port) = proxy::bind()?;
        let connection = Arc::new(Self {
            credentials,
            client: api::client()?,
            port,
            secret: api::random()?,
            live: AtomicBool::new(true),
            stop: Arc::new(tokio::sync::Notify::new()),
            covers: tokio::sync::Semaphore::new(3),
            details: Mutex::new(VecDeque::new()),
            catalog: tokio::sync::Mutex::new(None),
        });
        proxy::start(
            listener,
            Arc::downgrade(&connection),
            connection.stop.clone(),
        );
        Ok(connection)
    }
    fn remember(&self, detail: &Detail) -> Result<(), String> {
        if detail.songs.len() > 500 {
            return Ok(());
        }
        let mut cache = self.details.lock().map_err(|_| UNAVAILABLE)?;
        cache.retain(|old| old.album.id != detail.album.id);
        cache.push_back(detail.clone());
        while cache.len() > 8 || cache.iter().map(Detail::size).sum::<usize>() > 4 * 1024 * 1024 {
            cache.pop_front();
        }
        Ok(())
    }
    fn prepared(&self, album: &str) -> Result<Detail, String> {
        if !self.active() {
            return Err(CHANGED.into());
        }
        self.details
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .iter()
            .find(|detail| detail.album.id == album)
            .cloned()
            .ok_or_else(|| "NAVIDROME_DETAILS_EXPIRED".into())
    }
    async fn request(&self, endpoint: &str, params: &[(&str, String)]) -> Result<Value, String> {
        if !self.active() {
            return Err(CHANGED.into());
        }
        let value = api::request(&self.client, &self.credentials, endpoint, params).await;
        if !self.active() {
            return Err(CHANGED.into());
        }
        value
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.retire();
    }
}
#[derive(Default)]
struct State {
    loaded: AtomicBool,
    current: Mutex<Option<Arc<Connection>>>,
    serial: tokio::sync::Mutex<()>,
}
static STATE: OnceLock<State> = OnceLock::new();
fn state() -> &'static State {
    STATE.get_or_init(State::default)
}
fn current(id: &str) -> Result<Arc<Connection>, String> {
    state()
        .current
        .lock()
        .map_err(|_| UNAVAILABLE)?
        .as_ref()
        .filter(|c| c.active() && c.credentials.connection_id == id)
        .cloned()
        .ok_or(CHANGED.into())
}
pub(crate) fn suspend() {
    let _ = replace(None);
    state().loaded.store(false, Ordering::Release);
}
fn saved_status(credentials: Option<&Credentials>) -> Status {
    Status { connection_id: None, account_id: None, server: credentials.map(|c| c.server.clone()), username: credentials.map(|c| c.username.clone()) }
}
fn path() -> std::path::PathBuf {
    crate::persistence::sonante_config_dir().join("navidrome.json")
}
fn owner(app: &AppHandle) -> Result<(), String> {
    app.state::<crate::profile_lease::State>()
        .lease()
        .and_then(|l| l.validate())
        .map_err(|_| STORAGE.into())
}
fn replace(mut next: Option<Arc<Connection>>) -> Result<(), String> {
    let mut current = state().current.lock().map_err(|_| UNAVAILABLE)?;
    if !crate::music_sources::navidrome() { if let Some(connection)=next.take() { connection.retire(); } }
    if let Some(previous) = std::mem::replace(&mut *current, next) {
        previous.retire()
    }
    Ok(())
}
#[derive(Serialize)]
pub(crate) struct Status {
    pub(crate) connection_id: Option<String>,
    pub(crate) account_id: Option<String>,
    server: Option<String>,
    username: Option<String>,
}
fn status() -> Result<Status, String> {
    let c = state().current.lock().map_err(|_| UNAVAILABLE)?;
    Ok(Status {
        connection_id: c.as_ref().map(|c| c.credentials.connection_id.clone()),
        account_id: c.as_ref().map(|c| favorites::scope(&c.credentials)),
        server: c.as_ref().map(|c| c.credentials.server.clone()),
        username: c.as_ref().map(|c| c.credentials.username.clone()),
    })
}
#[tauri::command]
pub(crate) async fn navidrome_status(app: AppHandle) -> Result<Status, String> {
    let _serial = state().serial.lock().await;
    if !crate::music_sources::navidrome() {
        owner(&app)?;
        let stored=tauri::async_runtime::spawn_blocking(||store::load(&path())).await.map_err(|_|STORAGE)??;
        owner(&app)?;
        suspend();
        return Ok(saved_status(stored.as_ref()));
    }
    if !state().loaded.load(Ordering::Acquire) {
        owner(&app)?;
        let stored = tauri::async_runtime::spawn_blocking(|| store::load(&path()))
            .await
            .map_err(|_| STORAGE)??;
        owner(&app)?;
        if !crate::music_sources::navidrome() { return Ok(saved_status(stored.as_ref())); }
        replace(stored.map(Connection::open).transpose()?)?;
        state().loaded.store(true, Ordering::Release);
    }
    status()
}
#[tauri::command]
pub(crate) async fn navidrome_connect(
    app: AppHandle,
    server: String,
    username: String,
    password: String,
) -> Result<Status, String> {
    let _serial = state().serial.lock().await;
    owner(&app)?;
    let credentials = Credentials {
        connection_id: api::random()?,
        server: api::server(&server)?.to_string(),
        username: username.trim().into(),
        password,
    };
    credentials.validate()?;
    api::request(&api::client()?, &credentials, "ping", &[]).await?;
    owner(&app)?;
    let saved = saved_status(Some(&credentials));
    let next = if crate::music_sources::navidrome() { Some(Connection::open(credentials.clone())?) } else { None };
    if let Err(error) = tauri::async_runtime::spawn_blocking(move || {
        let _existing = store::load(&path())?;
        store::save(&path(), Some(credentials))
    })
    .await
    .map_err(|_| STORAGE)?
    {
        if let Some(next)=&next { next.retire(); }
        return Err(error);
    }
    owner(&app)?;
    if !crate::music_sources::navidrome() { suspend(); return Ok(saved); }
    replace(next)?;
    state().loaded.store(true, Ordering::Release);
    let result=status()?;
    let _ = tauri::Emitter::emit(&app,"navidrome-configuration-changed",());
    Ok(result)
}
#[tauri::command]
pub(crate) async fn navidrome_disconnect(app: AppHandle) -> Result<Status, String> {
    let _serial = state().serial.lock().await;
    owner(&app)?;
    tauri::async_runtime::spawn_blocking(|| {
        let _existing = store::load(&path())?;
        store::save(&path(), None)
    })
    .await
    .map_err(|_| STORAGE)??;
    owner(&app)?;
    replace(None)?;
    state().loaded.store(true, Ordering::Release);
    let result=status()?;
    let _ = tauri::Emitter::emit(&app,"navidrome-configuration-changed",());
    Ok(result)
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sort {
    Name,
    Recent,
}
#[derive(Serialize)]
pub(crate) struct Page {
    albums: Vec<Album>,
    offset: usize,
    more: bool,
    total: Option<usize>,
}
fn album_rows(root: &Value, key: &str) -> Result<Vec<Album>, String> {
    let parent = root.get(key).filter(|v| v.is_object()).ok_or(PROTOCOL)?;
    match parent.get("album") {
        None => Ok(vec![]),
        Some(Value::Array(values)) if values.len() <= api::PAGE_SIZE => {
            values.iter().map(api::album).collect()
        }
        _ => Err(PROTOCOL.into()),
    }
}
#[tauri::command]
pub(crate) async fn navidrome_albums(
    connection_id: String,
    query: String,
    sort: Sort,
    offset: usize,
    page_size: Option<usize>,
) -> Result<Page, String> {
    let size = page_size.unwrap_or(36);
    if !matches!(size, 18 | 36 | 72) { return Err(INVALID.into()); }
    if offset > 1_000_000 || query.len() > 256 || query.chars().any(char::is_control) {
        return Err(INVALID.into());
    }
    let c = current(&connection_id)?;
    let (root, key, total) = if query.trim().is_empty() {
        if offset == 0 { if let Ok(mut cache) = c.catalog.try_lock() { *cache = None; } }
        if !c.active() { return Err(CHANGED.into()); }
        let (root, total) = api::request_counted(&c.client, &c.credentials, "getAlbumList2", &[
            ("type", match sort { Sort::Name => "alphabeticalByName", Sort::Recent => "newest" }.into()),
            ("size", size.to_string()), ("offset", offset.to_string()),
        ]).await?;
        if !c.active() { return Err(CHANGED.into()); }
        (root, "albumList2", total)
    } else {
        (
            c.request(
                "search3",
                &[("query", query.trim().into()),("artistCount", "0".into()),("songCount", "0".into()),("albumCount", size.to_string()),("albumOffset", offset.to_string())],
            ).await?, "searchResult3", None,
        )
    };
    let albums = album_rows(&root, key)?;
    if albums.len() > size { return Err(PROTOCOL.into()); }
    current(&connection_id)?;
    Ok(Page {
        more: total.map_or(albums.len() == size, |total| offset + albums.len() < total),
        total,
        albums,
        offset,
    })
}
// Favorites belong to a durable server/account identity, never to a temporary
// playback connection. No favorites are written to the remote server.
#[tauri::command]
pub(crate) async fn navidrome_favorites(app: AppHandle, connection_id: String) -> Result<Vec<Album>, String> {
    let _serial = state().serial.lock().await;
    owner(&app)?;
    let c = current(&connection_id)?;
    let scope = favorites::scope(&c.credentials);
    let albums = tauri::async_runtime::spawn_blocking(move || favorites::read(&path().with_file_name("navidrome-favorites.json"), &scope))
        .await.map_err(|_| STORAGE)??;
    owner(&app)?;
    current(&connection_id)?;
    Ok(albums)
}
#[tauri::command]
pub(crate) async fn navidrome_set_favorite(app: AppHandle, connection_id: String, id: String, enabled: bool) -> Result<Vec<Album>, String> {
    if !api::id(&id) { return Err(INVALID.into()); }
    let _serial = state().serial.lock().await;
    owner(&app)?;
    let c = current(&connection_id)?;
    // Removing remains possible while offline. Adding uses native server metadata.
    let album = if enabled { Some(detail(&c, &id).await?.album) } else { None };
    current(&connection_id)?;
    owner(&app)?;
    let scope = favorites::scope(&c.credentials);
    let albums = tauri::async_runtime::spawn_blocking(move || favorites::set(&path().with_file_name("navidrome-favorites.json"), &scope, &id, album))
        .await.map_err(|_| STORAGE)??;
    owner(&app)?;
    current(&connection_id)?;
    Ok(albums)
}
#[derive(Clone, Serialize)]
pub(crate) struct Detail {
    album: Album,
    songs: Vec<Song>,
    artist_id: Option<String>,
}
impl Detail {
    fn size(&self) -> usize {
        self.album.id.len()
            + self.album.title.len()
            + self.album.artist.len()
            + self.album.cover_id.as_ref().map_or(0, String::len)
            + self.artist_id.as_ref().map_or(0, String::len)
            + self
                .songs
                .iter()
                .map(|song| {
                    song.id.len() + song.title.len() + song.artist.len() + song.album.len() + 128
                })
                .sum::<usize>()
    }
}
async fn detail(c: &Connection, id: &str) -> Result<Detail, String> {
    if !api::id(id) {
        return Err(INVALID.into());
    }
    let root = c.request("getAlbum", &[("id", id.into())]).await?;
    let value = root.get("album").ok_or(PROTOCOL)?;
    let album = api::album(value)?;
    if album.id != id {
        return Err(PROTOCOL.into());
    }
    Ok(Detail {
        album,
        songs: api::songs(value)?,
        artist_id: value["artistId"].as_str().filter(|id| api::id(id)).map(str::to_owned),
    })
}
#[tauri::command]
pub(crate) async fn navidrome_album(connection_id: String, id: String) -> Result<Detail, String> {
    let c = current(&connection_id)?;
    let value = detail(&c, &id).await?;
    current(&connection_id)?;
    c.remember(&value)?;
    Ok(value)
}
#[derive(Serialize)]
pub(crate) struct PlayerNavigation {
    connection_id: String, song_id: String, account_id: String,
    album_id: String, artist_id: Option<String>,
}
async fn player_navigation(c:&Connection,song_id:&str)->Result<PlayerNavigation,String> {
    if !c.active() {return Err(CHANGED.into());}
    if !api::id(song_id) {return Err(INVALID.into());}
    let root=c.request("getSong",&[("id",song_id.to_owned())]).await?;
    let value=root.get("song").filter(|v|v.is_object()&&v["id"].as_str()==Some(song_id)).ok_or(PROTOCOL)?;
    let album_id=value["albumId"].as_str().filter(|id|api::id(id)).ok_or(PROTOCOL)?.to_owned();
    let artist_id=value["artistId"].as_str().filter(|id|api::id(id)).map(str::to_owned);
    if !c.active() {return Err(CHANGED.into());}
    Ok(PlayerNavigation{connection_id:c.credentials.connection_id.clone(),song_id:song_id.into(),
        account_id:favorites::scope(&c.credentials),album_id,artist_id})
}
#[tauri::command]
pub(crate) async fn navidrome_player_navigation(connection_id:String,song_id:String)->Result<PlayerNavigation,String> {
    let c=current(&connection_id)?;
    let result=player_navigation(&c,&song_id).await?;
    current(&connection_id)?;
    Ok(result)
}
// Playback artwork is a separate, optional read. Never wait for it while
// holding AudioState or committing an intent; no image bytes enter the queue.
async fn song_cover_id(c: &Connection, song: &str) -> Result<Option<String>, String> {
    if !api::id(song) { return Err(INVALID.into()); }
    if !c.active() { return Err(CHANGED.into()); }
    let cached = {
        let details = c.details.lock().map_err(|_| UNAVAILABLE)?;
        details.iter().rev().find(|album| album.songs.iter().any(|item| item.id == song))
            .and_then(|album| album.album.cover_id.clone())
    };
    if !c.active() { return Err(CHANGED.into()); }
    if cached.is_some() { return Ok(cached); }
    // The prepared album may have been evicted while another album was browsed.
    // Resolve the exact song from this connection instead of guessing a cover ID.
    let _slot = c.covers.acquire().await.map_err(|_| UNAVAILABLE)?;
    let root = c.request("getSong", &[("id", song.into())]).await?;
    let item = root.get("song").filter(|value| value.is_object()).ok_or(PROTOCOL)?;
    if item["id"].as_str() != Some(song) { return Err(PROTOCOL.into()); }
    match item.get("coverArt") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if api::id(id) => Ok(Some(id.clone())),
        _ => Err(PROTOCOL.into()),
    }
}
#[tauri::command]
pub(crate) async fn navidrome_song_cover_id(connection_id: String, song_id: String) -> Result<Option<String>, String> {
    let c = current(&connection_id)?;
    let cover = song_cover_id(&c, &song_id).await?;
    current(&connection_id)?;
    Ok(cover)
}
#[tauri::command]
pub(crate) async fn navidrome_cover(connection_id: String, id: String) -> Result<String, String> {
    use base64::Engine;
    if !api::id(&id) {
        return Err(INVALID.into());
    }
    let c = current(&connection_id)?;
    let _slot = c.covers.acquire().await.map_err(|_| UNAVAILABLE)?;
    let response = c
        .client
        .get(
            c.credentials
                .url("getCoverArt", &[("id", id), ("size", "256".into())])?,
        )
        .timeout(std::time::Duration::from_secs(12))
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    let bytes = api::bytes(response, 512 * 1024).await?;
    let mime = api::cover_type(&bytes).ok_or(PROTOCOL)?;
    current(&connection_id)?;
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
pub(crate) fn resolve(connection_id: &str, song_id: &str) -> Result<String, String> {
    current(connection_id)?.stream(song_id)
}
pub(crate) fn shutdown() {
    if let Ok(mut c) = state().current.lock() {
        if let Some(connection) = c.take() {
            connection.retire()
        }
    }
}
#[tauri::command]
pub(crate) async fn navidrome_play_album(
    app: AppHandle,
    connection_id: String,
    id: String,
    start_index: usize,
    intent: Option<String>,
) -> Result<(), String> {
    let token = crate::admit_or_validate_playback(
        &app.state::<crate::AudioState>().0,
        &app.state::<crate::ConfigTransactionState>(),
        intent,
    )?;
    if crate::initial_playback_context(&app.state::<crate::ConfigTransactionState>(), Some(&token))?
        .is_some()
    {
        return Err(crate::initial_plex_playback::WAIT.into());
    }
    if !api::id(&id) {
        return Err(INVALID.into());
    }
    let c = current(&connection_id)?;
    // The UI must open a validated album first. No network wait holds the
    // foreground control queue, and no renderer-supplied track data is trusted.
    let album = c.prepared(&id)?;
    if album.songs.is_empty() || album.songs.len() > 500 || start_index >= album.songs.len() {
        return Err(INVALID.into());
    }
    let mut uris = Vec::new();
    let mut tracks = Vec::new();
    for song in album.songs {
        uris.push(c.stream(&song.id)?);
        tracks.push(TrackMetadata {
            title: song.title,
            artist: song.artist,
            album: song.album,
            thumb: None,
            plex_image: None,
            media_locator: Some(MediaLocator::Navidrome {
                connection_id: connection_id.clone(),
                song_id: song.id,
                account_id: Some(favorites::scope(&c.credentials)),
            }),
            uri: String::new(),
            duration: song.duration,
        });
    }
    tauri::async_runtime::spawn_blocking(move || {
        crate::finish_playback_intent(
            &app.state::<crate::AudioState>().0,
            &app.state::<crate::ConfigTransactionState>(),
            &token,
            |audio| {
                let guard = state().current.lock().map_err(|_| UNAVAILABLE)?;
                if !guard
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &c) && current.active())
                {
                    return Err(CHANGED.into());
                }
                audio.play_tracks(tracks, uris, start_index)
            },
        )
    })
    .await
    .map_err(|_| UNAVAILABLE.to_string())?
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn album_page_sizes_are_validated_before_any_connection_or_server_access() {
        for size in [None,Some(18),Some(36),Some(72)] {
            assert_eq!(navidrome_albums("missing-page-fixture".into(),"".into(),Sort::Name,0,size).await.err().as_deref(),Some(CHANGED));
        }
        for size in [0,1,19,60,73,500,usize::MAX] {
            assert_eq!(navidrome_albums("missing-page-fixture".into(),"".into(),Sort::Name,0,Some(size)).await.err().as_deref(),Some(INVALID));
        }
        let album = serde_json::json!({"id":"album","name":"Album"});
        assert!(album_rows(&serde_json::json!({"albumList2":{"album":vec![album;73]}}),"albumList2").is_err());
    }
    #[test]
    fn malformed_catalog_is_not_a_successful_empty_library() {
        assert!(album_rows(&serde_json::json!({}), "albumList2").is_err());
        assert!(album_rows(
            &serde_json::json!({"albumList2":{"album":null}}),
            "albumList2"
        )
        .is_err());
        assert!(
            album_rows(&serde_json::json!({"albumList2":{}}), "albumList2")
                .unwrap()
                .is_empty()
        );
        assert!(album_rows(
            &serde_json::json!({"albumList2":{"album":[{"name":"No ID"}]}}),
            "albumList2"
        )
        .is_err());
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };
    #[tokio::test]
    async fn real_http_subset_and_media_bridge_preserve_bytes_ranges_and_hide_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let capture = observed.clone();
        let peer = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut served = 0;
            while served < 6 && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap_or(0) != 1 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                capture.lock().unwrap().push(request.clone());
                let target = request
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let url = reqwest::Url::parse(&format!("http://fixture{target}")).unwrap();
                let params = url
                    .query_pairs()
                    .collect::<std::collections::HashMap<_, _>>();
                assert!(params.contains_key("t"));
                assert!(params.contains_key("s"));
                assert!(!target.contains("fixture-password"));
                let (status, mime, extra, body) = if url.path().contains("getAlbumList2") {
                    ("200 OK","application/json","",br#"{"subsonic-response":{"status":"ok","albumList2":{"album":[{"id":"album-a","name":"Album","artist":"Artist","coverArt":"cover-a"}]}}}"#.to_vec())
                } else if url.path().contains("getAlbum.view") {
                    ("200 OK","application/json","",br#"{"subsonic-response":{"status":"ok","album":{"id":"album-a","name":"Album","artist":"Artist","song":[{"id":"song-a","title":"Track","duration":60}]}}}"#.to_vec())
                } else if url.path().contains("getCoverArt") {
                    (
                        "200 OK",
                        "image/png",
                        "",
                        b"\x89PNG\r\n\x1a\nfixture".to_vec(),
                    )
                } else if url.path().contains("stream") {
                    assert_eq!(params["format"], "raw");
                    if request.to_lowercase().contains("range: bytes=2-5") {
                        (
                            "206 Partial Content",
                            "audio/flac",
                            "Content-Range: bytes 2-5/10\r\n",
                            b"2345".to_vec(),
                        )
                    } else {
                        ("200 OK", "audio/flac", "", b"0123456789".to_vec())
                    }
                } else {
                    (
                        "200 OK",
                        "application/json",
                        "",
                        br#"{"subsonic-response":{"status":"ok","version":"1.16.1"}}"#.to_vec(),
                    )
                };
                write!(stream,"HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",body.len()).unwrap();
                stream.write_all(&body).unwrap();
                served += 1;
            }
        });
        let c = Connection::open(Credentials {
            connection_id: "fixture-connection".into(),
            server: format!("http://{addr}/"),
            username: "fixture-user".into(),
            password: "fixture-password".into(),
        })
        .unwrap();
        c.request("ping", &[]).await.unwrap();
        let root = c
            .request(
                "getAlbumList2",
                &[("type", "newest".into()), ("size", "60".into())],
            )
            .await
            .unwrap();
        assert_eq!(album_rows(&root, "albumList2").unwrap()[0].id, "album-a");
        let album = detail(&c, "album-a").await.unwrap();
        assert_eq!(album.songs[0].id, "song-a");
        let response = c
            .client
            .get(
                c.credentials
                    .url("getCoverArt", &[("id", "cover-a".into())])
                    .unwrap(),
            )
            .send()
            .await
            .unwrap();
        let bytes = api::bytes(response, 512 * 1024).await.unwrap();
        assert_eq!(api::cover_type(&bytes), Some("image/png"));
        let uri = c.stream("song-a").unwrap();
        assert!(!uri.contains("fixture-user"));
        assert!(!uri.contains("fixture-password"));
        assert!(!uri.contains(&addr.to_string()));
        let response = c.client.get(&uri).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"0123456789");
        let response = c
            .client
            .get(&uri)
            .header("Range", "bytes=2-5")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 206);
        assert_eq!(response.headers()["content-range"], "bytes 2-5/10");
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"2345");
        let wrong = uri.replace(&c.secret, "wrong-capability");
        assert_eq!(
            c.client.get(wrong).send().await.unwrap().status().as_u16(),
            404
        );
        c.retire();
        assert!(c.stream("song-a").is_err());
        peer.join().unwrap();
        assert_eq!(observed.lock().unwrap().len(), 6);
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    fn credentials(id: &str, server: String) -> Credentials {
        Credentials {
            connection_id: id.into(),
            server,
            username: "fixture-user".into(),
            password: "fixture-password".into(),
        }
    }
    include!("player_navigation_tests.rs");
    fn fixture_album(id: &str) -> Detail {
        Detail {
            artist_id: Some("artist".into()),
            album: Album {
                id: id.into(),
                title: "Album".into(),
                artist: "Artist".into(),
                year: None,
                cover_id: None,
                song_count: Some(1),
            },
            songs: vec![Song {
                id: "song".into(),
                title: "Track".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration: Some(60.0),
                disc: 1,
                track: 1,
            }],
        }
    }
    #[tokio::test]
    async fn playback_cover_uses_prepared_album_without_network_and_rejects_retired_connection() {
        let c = Connection::open(credentials("cover-cache", "http://127.0.0.1:1/".into())).unwrap();
        let mut album = fixture_album("album");
        album.album.cover_id = Some("album-art".into());
        c.remember(&album).unwrap();
        assert_eq!(song_cover_id(&c,"song").await.unwrap().as_deref(),Some("album-art"));
        assert_eq!(song_cover_id(&c,"../wrong").await.err().as_deref(),Some(INVALID));
        c.retire();
        assert_eq!(song_cover_id(&c,"song").await.err().as_deref(),Some(CHANGED));
    }
    #[tokio::test]
    async fn playback_cover_resolves_exact_evicted_song_and_preserves_missing_or_malformed_results() {
        use std::io::{Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address=listener.local_addr().unwrap();
        let peer=std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream,_)=listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
                let mut request=Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len()<8192 {
                    let mut byte=[0];if stream.read(&mut byte).unwrap_or(0)!=1 { break; }request.push(byte[0]);
                }
                let request=String::from_utf8(request).unwrap();
                let target=request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
                let url=reqwest::Url::parse(&format!("http://fixture{target}")).unwrap();
                assert!(url.path().ends_with("/getSong.view"));
                let song=url.query_pairs().find(|(key,_)|key=="id").unwrap().1.into_owned();
                let item=match song.as_str() {
                    "present"=>serde_json::json!({"id":"present","coverArt":"song-art"}),
                    "absent"=>serde_json::json!({"id":"absent"}),
                    "wrong"=>serde_json::json!({"id":"another-song","coverArt":"wrong-art"}),
                    _=>panic!("unexpected fixture song"),
                };
                let body=serde_json::json!({"subsonic-response":{"status":"ok","song":item}}).to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
        });
        let c=Connection::open(credentials("cover-http",format!("http://{address}/"))).unwrap();
        assert_eq!(song_cover_id(&c,"present").await.unwrap().as_deref(),Some("song-art"));
        assert!(song_cover_id(&c,"absent").await.unwrap().is_none());
        assert_eq!(song_cover_id(&c,"wrong").await.err().as_deref(),Some(PROTOCOL));
        c.retire();peer.join().unwrap();
    }
    #[tokio::test]
    async fn validated_details_are_bounded_connection_scoped_and_do_not_require_http_at_play_time()
    {
        let c = Connection::open(credentials("one", "http://127.0.0.1:1/".into())).unwrap();
        assert!(c.prepared("album").is_err());
        for index in 0..20 {
            c.remember(&fixture_album(&format!("album-{index}")))
                .unwrap();
        }
        assert_eq!(c.details.lock().unwrap().len(), 8);
        assert!(c.prepared("album-0").is_err());
        assert_eq!(c.prepared("album-19").unwrap().songs[0].id, "song");
        let other = Connection::open(credentials("two", "http://127.0.0.1:1/".into())).unwrap();
        assert!(other.prepared("album-19").is_err());
        c.retire();
        assert_eq!(c.prepared("album-19").err().as_deref(), Some(CHANGED));
        other.retire();
    }
    #[tokio::test]
    async fn retirement_wins_over_a_delayed_authentication_error_from_the_old_connection() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (ready, when_ready) = tokio::sync::oneshot::channel();
        let (release, wait_release) = std::sync::mpsc::channel();
        let peer = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                if stream.read(&mut byte).unwrap() != 1 {
                    break;
                }
                request.push(byte[0]);
            }
            ready.send(()).unwrap();
            wait_release
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            let body=br#"{"subsonic-response":{"status":"failed","error":{"code":40,"message":"private response"}}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        let c = Connection::open(credentials("retired", format!("http://{addr}/"))).unwrap();
        let pending = c.clone();
        let task = tokio::spawn(async move { pending.request("ping", &[]).await });
        tokio::time::timeout(std::time::Duration::from_secs(3), when_ready)
            .await
            .unwrap()
            .unwrap();
        c.retire();
        release.send(()).unwrap();
        assert_eq!(task.await.unwrap().unwrap_err(), CHANGED);
        peer.join().unwrap();
    }
}

#[tauri::command]
pub(crate) async fn navidrome_song_navigation(app: AppHandle, locator: MediaLocator) -> Result<library::SongNavigation,String> {
    library::song_navigation(app,locator).await
}

#[tauri::command]
pub(crate) async fn navidrome_configuration(app: AppHandle) -> Result<bool,String> {
    if !crate::music_sources::navidrome() {return Ok(false);}
    owner(&app)?;
    let configured=tauri::async_runtime::spawn_blocking(||store::load(&path()).map(|v|v.is_some())).await.map_err(|_|STORAGE)??;
    owner(&app)?;Ok(configured&&crate::music_sources::navidrome())
}
#[tauri::command]
pub(crate) async fn navidrome_search(connection_id:String,query:String)->Result<discovery::SearchResults,String>{discovery::search(connection_id,query).await}
#[tauri::command]
pub(crate) async fn navidrome_artist(connection_id:String,id:String)->Result<discovery::ArtistDetail,String>{discovery::artist_detail(connection_id,id).await}
#[tauri::command]
pub(crate) async fn navidrome_explore(connection_id:String)->Result<discovery::Explore,String>{discovery::explore(connection_id).await}
