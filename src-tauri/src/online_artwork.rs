use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USER_AGENT: &str = "Sonante/0.4.2 (https://github.com/tiagoalam/Sonante)";
const MUSICBRAINZ_URL: &str = "https://musicbrainz.org/ws/2/release-group/";
const COVER_ART_URL: &str = "https://coverartarchive.org/release-group";
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const NEGATIVE_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);
static LAST_MUSICBRAINZ_REQUEST: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
static CACHE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
pub struct OnlineAlbumCoverRequest {
    album_id: String,
    title: String,
    artist: String,
    year: Option<String>,
}

impl OnlineAlbumCoverRequest {
    fn validated(mut self) -> Result<Option<Self>, String> {
        if self.album_id.len() > 2048
            || self.title.chars().count() > 512
            || self.artist.chars().count() > 512
        {
            return Err("Metadados de álbum excedem o limite para busca de capa.".into());
        }
        self.album_id = self.album_id.trim().to_string();
        self.title = self.title.trim().to_string();
        self.artist = self.artist.trim().to_string();
        self.year = self.year.take().and_then(|year| {
            let year = year.trim().to_string();
            (year.len() == 4
                && year
                    .parse::<u16>()
                    .is_ok_and(|n| (1850..=2100).contains(&n)))
            .then_some(year)
        });
        if self.album_id.is_empty()
            || self.title.is_empty()
            || self.artist.is_empty()
            || matches!(
                normalize(&self.artist).as_str(),
                "various artists" | "artista desconhecido"
            )
        {
            return Ok(None);
        }
        Ok(Some(self))
    }
}

#[derive(Default, Serialize, Deserialize)]
struct CacheIndex {
    version: u8,
    entries: BTreeMap<String, CacheEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum CacheEntry {
    Positive {
        file: String,
        source: String,
        musicbrainz_release_group_id: String,
        mime: String,
        checked_at: u64,
    },
    Negative {
        checked_at: u64,
    },
}

enum CacheHit {
    Image(String),
    Negative,
    Miss,
}

fn cache_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::cache_dir)
        .ok_or_else(|| "Diretório de cache do usuário indisponível.".to_string())?;
    Ok(base.join("sonante").join("artwork"))
}

fn load_index(dir: &Path) -> Result<CacheIndex, String> {
    let path = dir.join("index.json");
    crate::persistence::prepare_private_file_for_load(&path, "índice de capas")?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(CacheIndex {
                version: 1,
                ..Default::default()
            })
        }
        Err(error) => return Err(format!("Falha ao ler índice de capas: {error}")),
    };
    let index: CacheIndex = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Índice de capas inválido; preservado: {error}"))?;
    if index.version != 1 {
        return Err("Versão desconhecida do índice de capas; preservado.".into());
    }
    Ok(index)
}

fn save_index(dir: &Path, index: &CacheIndex) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(index)
        .map_err(|error| format!("Falha ao serializar índice de capas: {error}"))?;
    crate::persistence::atomic_write_private(&dir.join("index.json"), &bytes, "índice de capas")
}

fn cache_filename(id: &str) -> String {
    fn fnv(bytes: &[u8], seed: u64) -> u64 {
        bytes.iter().fold(seed, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
    }
    format!(
        "{:016x}{:016x}.img",
        fnv(id.as_bytes(), 0xcbf29ce484222325),
        fnv(id.as_bytes(), 0x84222325cbf29ce4)
    )
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn read_cache(dir: &Path, id: &str, now: u64) -> Result<CacheHit, String> {
    let _guard = CACHE_LOCK
        .lock()
        .map_err(|_| "Cache de capas indisponível.".to_string())?;
    let mut index = load_index(dir)?;
    match index.entries.get(id).cloned() {
        Some(CacheEntry::Negative { checked_at })
            if now.saturating_sub(checked_at) < NEGATIVE_TTL_SECS =>
        {
            Ok(CacheHit::Negative)
        }
        Some(CacheEntry::Negative { .. }) => Ok(CacheHit::Miss),
        Some(CacheEntry::Positive { file, mime, .. }) => {
            // Filenames from the index are data, even when the index is valid JSON.
            if Path::new(&file).file_name().and_then(|n| n.to_str()) != Some(file.as_str())
                || !file.ends_with(".img")
            {
                return Err("Índice de capas contém nome de arquivo inválido; preservado.".into());
            }
            let path = dir.join(&file);
            crate::persistence::prepare_private_file_for_load(&path, "imagem de capa")?;
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(format!("Falha ao ler capa em cache: {error}")),
            };
            if !bytes.is_empty()
                && bytes.len() <= MAX_IMAGE_BYTES
                && image_mime(&bytes) == Some(mime.as_str())
            {
                return Ok(CacheHit::Image(data_uri(&mime, &bytes)));
            }
            index.entries.remove(id);
            save_index(dir, &index)?;
            Ok(CacheHit::Miss)
        }
        None => Ok(CacheHit::Miss),
    }
}

fn save_negative(dir: &Path, id: &str, now: u64) -> Result<(), String> {
    let _guard = CACHE_LOCK
        .lock()
        .map_err(|_| "Cache de capas indisponível.".to_string())?;
    let mut index = load_index(dir)?;
    index
        .entries
        .insert(id.to_string(), CacheEntry::Negative { checked_at: now });
    save_index(dir, &index)
}

fn save_positive(
    dir: &Path,
    id: &str,
    mbid: &str,
    mime: &str,
    bytes: &[u8],
    now: u64,
) -> Result<String, String> {
    let _guard = CACHE_LOCK
        .lock()
        .map_err(|_| "Cache de capas indisponível.".to_string())?;
    let mut index = load_index(dir)?;
    let base = cache_filename(id);
    let mut file = base.clone();
    let mut suffix = 2;
    while index.entries.iter().any(|(other_id, entry)| {
        other_id != id
            && matches!(entry, CacheEntry::Positive { file: existing, .. } if existing == &file)
    }) {
        file = format!("{}-{suffix}.img", base.trim_end_matches(".img"));
        suffix += 1;
    }
    crate::persistence::atomic_write_private(&dir.join(&file), bytes, "imagem de capa")?;
    index.entries.insert(
        id.to_string(),
        CacheEntry::Positive {
            file: file.clone(),
            source: "cover_art_archive".into(),
            musicbrainz_release_group_id: mbid.into(),
            mime: mime.into(),
            checked_at: now,
        },
    );
    save_index(dir, &index)?;
    match read_image_file(dir, &file, mime)? {
        Some(uri) => Ok(uri),
        None => Err("Capa persistida não pôde ser validada.".into()),
    }
}

fn read_image_file(dir: &Path, file: &str, mime: &str) -> Result<Option<String>, String> {
    let bytes = fs::read(dir.join(file))
        .map_err(|error| format!("Falha ao reler capa em cache: {error}"))?;
    Ok(
        (bytes.len() <= MAX_IMAGE_BYTES && image_mime(&bytes) == Some(mime))
            .then(|| data_uri(mime, &bytes)),
    )
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(rename = "release-groups")]
    release_groups: Vec<ReleaseGroup>,
}
#[derive(Deserialize)]
struct ReleaseGroup {
    id: String,
    title: String,
    score: u8,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<ArtistCredit>,
    #[serde(default, rename = "first-release-date")]
    first_release_date: Option<String>,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum ArtistCredit {
    Text(String),
    Person {
        name: String,
        #[serde(default, rename = "joinphrase")]
        join_phrase: String,
    },
}

fn valid_mbid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if [8, 13, 18, 23].contains(&i) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn select_release_group(
    results: &SearchResponse,
    title: &str,
    artist: &str,
    year: Option<&str>,
) -> Option<String> {
    let title = normalize(title);
    let artist = normalize(artist);
    let mut candidates: Vec<_> = results
        .release_groups
        .iter()
        .filter(|group| {
            if group.score < 90 || !valid_mbid(&group.id) || normalize(&group.title) != title {
                return false;
            }
            let credit = group
                .artist_credit
                .iter()
                .map(|item| match item {
                    ArtistCredit::Text(text) => text.clone(),
                    ArtistCredit::Person { name, join_phrase } => format!("{name}{join_phrase}"),
                })
                .collect::<String>();
            normalize(&credit) == artist
        })
        .collect();
    candidates.sort_by_key(|group| std::cmp::Reverse(group.score));
    let best_score = candidates.first()?.score;
    candidates.retain(|group| group.score == best_score);
    if candidates.len() > 1 {
        if let Some(year) = year {
            let matching: Vec<_> = candidates
                .iter()
                .filter(|group| {
                    group
                        .first_release_date
                        .as_deref()
                        .is_some_and(|date| date.starts_with(year))
                })
                .collect();
            if matching.len() == 1 {
                return Some(matching[0].id.clone());
            }
        }
        return None;
    }
    Some(candidates[0].id.clone())
}

fn next_request_at(last: Option<Instant>, now: Instant) -> Instant {
    last.and_then(|last| last.checked_add(MIN_REQUEST_INTERVAL))
        .map_or(now, |next| next.max(now))
}

fn wait_for_musicbrainz() -> Result<(), String> {
    let limiter = LAST_MUSICBRAINZ_REQUEST.get_or_init(|| Mutex::new(None));
    let mut last = limiter
        .lock()
        .map_err(|_| "Limitador MusicBrainz indisponível.".to_string())?;
    let next = next_request_at(*last, Instant::now());
    if let Some(wait) = next.checked_duration_since(Instant::now()) {
        std::thread::sleep(wait);
    }
    *last = Some(Instant::now());
    Ok(())
}

fn now_secs() -> Result<u64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Relógio do sistema inválido.".to_string())?
        .as_secs())
}

pub async fn get_online_album_cover(
    request: OnlineAlbumCoverRequest,
    enabled: bool,
) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }
    lookup(
        request,
        enabled,
        cache_dir()?,
        MUSICBRAINZ_URL,
        COVER_ART_URL,
        true,
    )
    .await
}

async fn lookup(
    request: OnlineAlbumCoverRequest,
    enabled: bool,
    dir: PathBuf,
    musicbrainz_url: &str,
    cover_art_url: &str,
    rate_limit: bool,
) -> Result<Option<String>, String> {
    if !enabled {
        return Ok(None);
    }
    let Some(request) = request.validated()? else {
        return Ok(None);
    };
    let now = now_secs()?;
    let id = request.album_id.clone();
    match tauri::async_runtime::spawn_blocking({
        let dir = dir.clone();
        let id = id.clone();
        move || read_cache(&dir, &id, now)
    })
    .await
    .map_err(|error| format!("Falha na tarefa de cache: {error}"))??
    {
        CacheHit::Image(uri) => return Ok(Some(uri)),
        CacheHit::Negative => return Ok(None),
        CacheHit::Miss => {}
    }
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("Falha ao preparar cliente de capas: {error}"))?;
    if rate_limit {
        tauri::async_runtime::spawn_blocking(wait_for_musicbrainz)
            .await
            .map_err(|error| format!("Falha na tarefa MusicBrainz: {error}"))??;
    }
    let query = format!(
        "releasegroup:\"{}\" AND artist:\"{}\"",
        request.title.replace(['\\', '"'], " "),
        request.artist.replace(['\\', '"'], " ")
    );
    let response = client
        .get(musicbrainz_url)
        .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
        .send()
        .await
        .map_err(|error| format!("Busca MusicBrainz falhou: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("MusicBrainz respondeu HTTP {}.", response.status()));
    }
    let results: SearchResponse = response
        .json()
        .await
        .map_err(|error| format!("Resposta MusicBrainz inválida: {error}"))?;
    let Some(mbid) = select_release_group(
        &results,
        &request.title,
        &request.artist,
        request.year.as_deref(),
    ) else {
        tauri::async_runtime::spawn_blocking(move || save_negative(&dir, &id, now))
            .await
            .map_err(|error| format!("Falha na tarefa de cache: {error}"))??;
        return Ok(None);
    };
    let url = format!("{cover_art_url}/{mbid}/front-500");
    let mut response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("Cover Art Archive indisponível: {error}"))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        tauri::async_runtime::spawn_blocking(move || save_negative(&dir, &id, now))
            .await
            .map_err(|error| format!("Falha na tarefa de cache: {error}"))??;
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "Cover Art Archive respondeu HTTP {}.",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_IMAGE_BYTES as u64)
    {
        return Err("Capa online excede 8 MiB.".into());
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .ok_or_else(|| "Capa online sem tipo de imagem válido.".to_string())?;
    if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") {
        return Err("Formato de capa online não suportado.".into());
    }
    let mime = mime.to_string();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Download de capa interrompido: {error}"))?
    {
        if chunk.len() > MAX_IMAGE_BYTES.saturating_sub(bytes.len()) {
            return Err("Capa online excede 8 MiB.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if image_mime(&bytes) != Some(mime.as_str()) {
        return Err("Assinatura de imagem online inválida.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        save_positive(&dir, &id, &mbid, &mime, &bytes, now)
    })
    .await
    .map_err(|error| format!("Falha na tarefa de cache: {error}"))?
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    const ID: &str = "12345678-1234-1234-1234-123456789abc";
    fn request() -> OnlineAlbumCoverRequest {
        OnlineAlbumCoverRequest {
            album_id: "root|album".into(),
            title: " Album ".into(),
            artist: " Artist ".into(),
            year: None,
        }
    }
    fn group(score: u8, title: &str, artist: &str) -> ReleaseGroup {
        ReleaseGroup {
            id: ID.into(),
            score,
            title: title.into(),
            artist_credit: vec![ArtistCredit::Person {
                name: artist.into(),
                join_phrase: String::new(),
            }],
            first_release_date: None,
        }
    }
    #[test]
    fn invalid_input_skips_lookup() {
        assert!(OnlineAlbumCoverRequest {
            title: " ".into(),
            ..request()
        }
        .validated()
        .unwrap()
        .is_none());
        assert!(OnlineAlbumCoverRequest {
            artist: "Various Artists".into(),
            ..request()
        }
        .validated()
        .unwrap()
        .is_none());
        assert!(OnlineAlbumCoverRequest {
            artist: "Artista Desconhecido".into(),
            ..request()
        }
        .validated()
        .unwrap()
        .is_none());
    }
    #[test]
    fn matching_is_conservative() {
        let response = |groups| SearchResponse {
            release_groups: groups,
        };
        assert_eq!(
            select_release_group(
                &response(vec![group(95, "  album  ", "artist")]),
                "Album",
                "Artist",
                None
            ),
            Some(ID.into())
        );
        assert!(select_release_group(
            &response(vec![group(89, "Album", "Artist")]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        assert!(select_release_group(
            &response(vec![group(95, "Other", "Artist")]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        assert!(select_release_group(
            &response(vec![group(95, "Album", "Other")]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        assert!(select_release_group(
            &response(vec![
                group(95, "Album", "Artist"),
                group(95, "Album", "Artist")
            ]),
            "Album",
            "Artist",
            None
        )
        .is_none());
        let mut first = group(95, "Album", "Artist");
        first.first_release_date = Some("2001-01-01".into());
        let mut second = group(95, "Album", "Artist");
        second.id = "87654321-1234-1234-1234-123456789abc".into();
        second.first_release_date = Some("2002".into());
        assert_eq!(
            select_release_group(
                &response(vec![first, second]),
                "Album",
                "Artist",
                Some("2002")
            ),
            Some("87654321-1234-1234-1234-123456789abc".into())
        );
    }
    #[test]
    fn cache_key_and_rate_schedule_are_deterministic() {
        assert_eq!(cache_filename("a"), cache_filename("a"));
        assert_ne!(cache_filename("a"), cache_filename("b"));
        let now = Instant::now();
        assert_eq!(next_request_at(None, now), now);
        assert_eq!(next_request_at(Some(now), now), now + MIN_REQUEST_INTERVAL);
    }
    #[test]
    fn image_formats_require_magic() {
        assert_eq!(image_mime(&[0xff, 0xd8, 0xff]), Some("image/jpeg"));
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
        assert_eq!(image_mime(b"RIFF1234WEBP"), Some("image/webp"));
        assert_eq!(image_mime(b"<svg/>"), None);
    }

    fn test_dir() -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "sonante-online-artwork-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn positive_and_negative_cache_persist() {
        let dir = test_dir();
        let jpeg = [0xff, 0xd8, 0xff, 0xd9];
        let uri = save_positive(&dir, "album-a", ID, "image/jpeg", &jpeg, 100).unwrap();
        assert_eq!(uri, data_uri("image/jpeg", &jpeg));
        assert!(matches!(
            read_cache(&dir, "album-a", 101).unwrap(),
            CacheHit::Image(_)
        ));
        save_negative(&dir, "album-b", 100).unwrap();
        assert!(matches!(
            read_cache(&dir, "album-b", 100 + NEGATIVE_TTL_SECS - 1).unwrap(),
            CacheHit::Negative
        ));
        assert!(matches!(
            read_cache(&dir, "album-b", 100 + NEGATIVE_TTL_SECS).unwrap(),
            CacheHit::Miss
        ));
        let index = load_index(&dir).unwrap();
        assert_eq!(index.version, 1);
        assert_eq!(index.entries.len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_image_is_repaired_and_corrupt_index_is_preserved() {
        let dir = test_dir();
        save_positive(&dir, "album-a", ID, "image/png", b"\x89PNG\r\n\x1a\n", 100).unwrap();
        let file = match load_index(&dir).unwrap().entries.get("album-a").unwrap() {
            CacheEntry::Positive { file, .. } => file.clone(),
            _ => panic!("expected positive cache entry"),
        };
        fs::remove_file(dir.join(file)).unwrap();
        assert!(matches!(
            read_cache(&dir, "album-a", 101).unwrap(),
            CacheHit::Miss
        ));
        assert!(!load_index(&dir).unwrap().entries.contains_key("album-a"));
        fs::write(dir.join("index.json"), b"{invalid").unwrap();
        assert!(save_negative(&dir, "album-b", 102).is_err());
        assert_eq!(fs::read(dir.join("index.json")).unwrap(), b"{invalid");
        fs::remove_dir_all(dir).unwrap();
    }

    fn fake_http(status: &str, mime: &str, body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_string();
        let mime = mime.to_string();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0u8; 4096];
            let len = stream.read(&mut request).unwrap();
            assert!(std::str::from_utf8(&request[..len])
                .unwrap()
                .contains("GET /"));
            let headers = format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(headers.as_bytes()).unwrap();
            let _ = stream.write_all(&body);
        });
        (url, handle)
    }

    fn search_json() -> Vec<u8> {
        format!(r#"{{"release-groups":[{{"id":"{ID}","title":"Album","score":100,"artist-credit":[{{"name":"Artist"}}]}}]}}"#).into_bytes()
    }

    #[test]
    fn cache_hit_and_disabled_preference_do_not_use_http() {
        let dir = test_dir();
        save_positive(
            &dir,
            "root|album",
            ID,
            "image/jpeg",
            &[0xff, 0xd8, 0xff],
            100,
        )
        .unwrap();
        let absent = "http://127.0.0.1:1";
        let hit = tauri::async_runtime::block_on(lookup(
            request(),
            true,
            dir.clone(),
            absent,
            absent,
            false,
        ))
        .unwrap();
        assert!(hit.unwrap().starts_with("data:image/jpeg;base64,"));
        let disabled = tauri::async_runtime::block_on(lookup(
            request(),
            false,
            test_dir(),
            absent,
            absent,
            false,
        ))
        .unwrap();
        assert!(disabled.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn http_404_is_negative_but_503_is_not() {
        for (status, negative) in [("404 Not Found", true), ("503 Service Unavailable", false)] {
            let dir = test_dir();
            let (mb_url, mb_handle) = fake_http("200 OK", "application/json", search_json());
            let (caa_url, caa_handle) = fake_http(status, "text/plain", Vec::new());
            let result = tauri::async_runtime::block_on(lookup(
                request(),
                true,
                dir.clone(),
                &mb_url,
                &caa_url,
                false,
            ));
            mb_handle.join().unwrap();
            caa_handle.join().unwrap();
            if negative {
                assert!(result.unwrap().is_none());
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                matches!(
                    read_cache(&dir, "root|album", now_secs().unwrap()).unwrap(),
                    CacheHit::Negative
                ),
                negative
            );
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn invalid_mime_and_oversize_do_not_enter_cache() {
        for (mime, body) in [
            ("text/html", b"<html>".to_vec()),
            ("image/jpeg", vec![0xff; MAX_IMAGE_BYTES + 1]),
        ] {
            let dir = test_dir();
            let (mb_url, mb_handle) = fake_http("200 OK", "application/json", search_json());
            let (caa_url, caa_handle) = fake_http("200 OK", mime, body);
            let result = tauri::async_runtime::block_on(lookup(
                request(),
                true,
                dir.clone(),
                &mb_url,
                &caa_url,
                false,
            ));
            mb_handle.join().unwrap();
            caa_handle.join().unwrap();
            assert!(result.is_err());
            assert!(matches!(
                read_cache(&dir, "root|album", now_secs().unwrap()).unwrap(),
                CacheHit::Miss
            ));
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn http_image_is_persisted_and_subsequent_call_is_offline() {
        let dir = test_dir();
        let (mb_url, mb_handle) = fake_http("200 OK", "application/json", search_json());
        let (caa_url, caa_handle) =
            fake_http("200 OK", "image/png", b"\x89PNG\r\n\x1a\nimage".to_vec());
        let first = tauri::async_runtime::block_on(lookup(
            request(),
            true,
            dir.clone(),
            &mb_url,
            &caa_url,
            false,
        ))
        .unwrap();
        mb_handle.join().unwrap();
        caa_handle.join().unwrap();
        assert!(first
            .as_deref()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        let second = tauri::async_runtime::block_on(lookup(
            request(),
            true,
            dir.clone(),
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
            false,
        ))
        .unwrap();
        assert_eq!(first, second);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_trustworthy_match_is_negative_cached() {
        let dir = test_dir();
        let (mb_url, mb_handle) = fake_http(
            "200 OK",
            "application/json",
            b"{\"release-groups\":[]}".to_vec(),
        );
        let result = tauri::async_runtime::block_on(lookup(
            request(),
            true,
            dir.clone(),
            &mb_url,
            "http://127.0.0.1:1",
            false,
        ))
        .unwrap();
        mb_handle.join().unwrap();
        assert!(result.is_none());
        assert!(matches!(
            read_cache(&dir, "root|album", now_secs().unwrap()).unwrap(),
            CacheHit::Negative
        ));
        fs::remove_dir_all(dir).unwrap();
    }
}
