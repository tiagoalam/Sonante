use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::State;

const BASE_URL: &str = "https://lrclib.net";
const MAX_BODY: usize = 256 * 1024;
const MAX_RECORDS: usize = 20;
const MAX_TEXT: usize = 32_000;
const MIN_INTERVAL: Duration = Duration::from_millis(350);
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsQuery {
    title: String,
    artist: String,
    album: String,
    duration: Option<u32>,
    #[serde(default)]
    cache_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LookupReply {
    Records {
        records: Vec<LyricsRecord>,
        persisted: bool,
    },
    NotFound,
    Offline,
    Timeout,
    RateLimited,
    ServiceUnavailable,
    InvalidResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsRecord {
    pub id: u64,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: String,
    pub duration: f64,
    pub instrumental: bool,
    pub has_word_sync: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredReply {
    Records { records: Vec<LyricsRecord> },
    NotFound,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LyricsChoice {
    Automatic,
    PreferLocal,
    Manual { record: LyricsRecord },
}

#[derive(Clone, Debug, Deserialize)]
pub struct LegacyCacheEntry {
    key: String,
    at: i64,
    result: StoredReply,
}

#[derive(Serialize, Deserialize)]
struct ResultEnvelope {
    version: u8,
    key: String,
    saved_at: i64,
    result: StoredReply,
}

#[derive(Serialize, Deserialize)]
struct ChoiceEnvelope {
    version: u8,
    media_identity: String,
    choice: LyricsChoice,
}

const NEGATIVE_TTL_SECS: i64 = 3600;
const STORE_FILE_MAX: u64 = 512 * 1024;

#[derive(Clone)]
pub struct LyricsStore {
    root: PathBuf,
    gate: Arc<Mutex<()>>,
}

impl LyricsStore {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        let store = Self {
            root,
            gate: Arc::new(Mutex::new(())),
        };
        store.ensure_layout()?;
        Ok(store)
    }

    pub fn unavailable(root: PathBuf) -> Self {
        Self {
            root,
            gate: Arc::new(Mutex::new(())),
        }
    }

    fn ensure_layout(&self) -> Result<(), String> {
        ensure_private_dir(&self.root)?;
        ensure_private_dir(&self.root.join("results-v1"))?;
        ensure_private_dir(&self.root.join("choices-v1"))
    }

    fn result_path(&self, key: &str) -> PathBuf {
        self.root
            .join("results-v1")
            .join(format!("{}.json", digest(key.as_bytes())))
    }

    fn choice_path(&self, identity: &str) -> PathBuf {
        self.root
            .join("choices-v1")
            .join(format!("{}.json", digest(identity.as_bytes())))
    }

    fn load_result(&self, key: &str) -> Result<Option<StoredReply>, String> {
        validate_cache_key(key)?;
        let _guard = self.gate.lock().map_err(|_| "LYRICS_STORE_LOCK_FAILED")?;
        self.ensure_layout()?;
        let path = self.result_path(key);
        let Some(bytes) = read_private_regular(&path)? else {
            return Ok(None);
        };
        let envelope: ResultEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| "LYRICS_STORE_INVALID")?;
        if envelope.version != 1 || envelope.key != key || !valid_stored_reply(&envelope.result) {
            return Err("LYRICS_STORE_INVALID".into());
        }
        if matches!(envelope.result, StoredReply::NotFound)
            && now_seconds().saturating_sub(envelope.saved_at) >= NEGATIVE_TTL_SECS
        {
            return Ok(None);
        }
        Ok(Some(envelope.result))
    }

    fn save_result(&self, key: &str, result: &StoredReply) -> Result<(), String> {
        validate_cache_key(key)?;
        if !valid_stored_reply(result) {
            return Err("LYRICS_STORE_INVALID".into());
        }
        let _guard = self.gate.lock().map_err(|_| "LYRICS_STORE_LOCK_FAILED")?;
        self.ensure_layout()?;
        let envelope = ResultEnvelope {
            version: 1,
            key: key.to_owned(),
            saved_at: now_seconds(),
            result: result.clone(),
        };
        if matches!(result, StoredReply::NotFound) {
            if let Some(bytes) = read_private_regular(&self.result_path(key))? {
                if serde_json::from_slice::<ResultEnvelope>(&bytes).is_ok_and(|old| {
                    old.version == 1
                        && old.key == key
                        && valid_stored_reply(&old.result)
                        && matches!(old.result, StoredReply::Records { .. })
                }) {
                    return Ok(());
                }
            }
        }
        atomic_json(&self.result_path(key), &envelope)
    }

    fn load_choice(
        &self,
        media: &crate::audio::CurrentMedia,
    ) -> Result<Option<LyricsChoice>, String> {
        let identity = stable_media_identity(media).ok_or("LYRICS_CHOICE_UNSUPPORTED")?;
        let _guard = self.gate.lock().map_err(|_| "LYRICS_STORE_LOCK_FAILED")?;
        self.ensure_layout()?;
        let Some(bytes) = read_private_regular(&self.choice_path(&identity))? else {
            return Ok(None);
        };
        let envelope: ChoiceEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| "LYRICS_STORE_INVALID")?;
        if envelope.version != 1
            || envelope.media_identity != identity
            || !valid_choice(&envelope.choice)
        {
            return Err("LYRICS_STORE_INVALID".into());
        }
        Ok(Some(envelope.choice))
    }

    fn save_choice(
        &self,
        media: &crate::audio::CurrentMedia,
        choice: &LyricsChoice,
    ) -> Result<(), String> {
        let identity = stable_media_identity(media).ok_or("LYRICS_CHOICE_UNSUPPORTED")?;
        if !valid_choice(choice) {
            return Err("LYRICS_STORE_INVALID".into());
        }
        let _guard = self.gate.lock().map_err(|_| "LYRICS_STORE_LOCK_FAILED")?;
        self.ensure_layout()?;
        let path = self.choice_path(&identity);
        atomic_json(
            &path,
            &ChoiceEnvelope {
                version: 1,
                media_identity: identity,
                choice: choice.clone(),
            },
        )
    }

    fn import_legacy(&self, entries: &[LegacyCacheEntry]) -> Result<(), String> {
        if entries.len() > 1000 {
            return Err("LYRICS_MIGRATION_INVALID".into());
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            validate_cache_key(&entry.key).map_err(|_| "LYRICS_MIGRATION_INVALID")?;
            if !seen.insert(entry.key.as_str()) || !valid_stored_reply(&entry.result) {
                return Err("LYRICS_MIGRATION_INVALID".into());
            }
        }
        // Each entry is atomic and repeatable. Keep the old localStorage copy until
        // the caller confirms every write, so interrupted migrations can resume.
        for entry in entries {
            if matches!(entry.result, StoredReply::NotFound)
                && now_seconds().saturating_sub(entry.at / 1000) >= NEGATIVE_TTL_SECS
            {
                continue;
            }
            self.import_result(&entry.key, &entry.result)?;
        }
        Ok(())
    }

    fn import_result(&self, key: &str, result: &StoredReply) -> Result<(), String> {
        validate_cache_key(key).map_err(|_| "LYRICS_MIGRATION_INVALID")?;
        if !valid_stored_reply(result) {
            return Err("LYRICS_MIGRATION_INVALID".into());
        }
        let _guard = self.gate.lock().map_err(|_| "LYRICS_STORE_LOCK_FAILED")?;
        self.ensure_layout()?;
        let path = self.result_path(key);
        if let Some(bytes) = read_private_regular(&path)? {
            if let Ok(envelope) = serde_json::from_slice::<ResultEnvelope>(&bytes) {
                if envelope.version == 1
                    && envelope.key == key
                    && valid_stored_reply(&envelope.result)
                    && matches!(envelope.result, StoredReply::Records { .. })
                {
                    return Ok(());
                }
            }
        }
        let envelope = ResultEnvelope {
            version: 1,
            key: key.to_owned(),
            saved_at: now_seconds(),
            result: result.clone(),
        };
        atomic_json(&path, &envelope)
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_cache_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > 2048 || key.chars().any(char::is_control) {
        return Err("LYRICS_CACHE_KEY_INVALID".into());
    }
    Ok(())
}

fn stable_media_identity(media: &crate::audio::CurrentMedia) -> Option<String> {
    let material = match media {
        crate::audio::CurrentMedia::Local { uri, .. }
            if !uri.is_empty() && uri.len() <= 8192 && !uri.chars().any(char::is_control) =>
        {
            serde_json::json!(["local", uri])
        }
        crate::audio::CurrentMedia::Plex {
            server_id,
            part_key,
            ..
        } if !server_id.is_empty()
            && server_id.len() <= 512
            && !server_id.chars().any(char::is_control)
            && !part_key.is_empty()
            && part_key.len() <= 2048
            && !part_key.chars().any(char::is_control) =>
        {
            serde_json::json!(["plex", server_id, part_key])
        }
        _ => return None,
    };
    serde_json::to_string(&material)
        .ok()
        .map(|value| digest(value.as_bytes()))
}

fn valid_choice(choice: &LyricsChoice) -> bool {
    match choice {
        LyricsChoice::Automatic | LyricsChoice::PreferLocal => true,
        LyricsChoice::Manual { record } => valid_record(record),
    }
}

fn valid_record(record: &LyricsRecord) -> bool {
    record.id > 0
        && valid_metadata(&record.track_name, false)
        && valid_metadata(&record.artist_name, false)
        && valid_metadata(&record.album_name, true)
        && record.duration.is_finite()
        && (1.0..=3600.0).contains(&record.duration)
        && record
            .plain_lyrics
            .as_ref()
            .is_none_or(|text| valid_lyrics(text))
        && record
            .synced_lyrics
            .as_ref()
            .is_none_or(|text| valid_lyrics(text))
        && (record.instrumental
            || record
                .plain_lyrics
                .as_ref()
                .is_some_and(|text| !text.trim().is_empty())
            || record
                .synced_lyrics
                .as_ref()
                .is_some_and(|text| !text.trim().is_empty()))
}

fn valid_lyrics(text: &str) -> bool {
    text.len() <= MAX_TEXT
        && !text
            .chars()
            .any(|ch| ch == '\0' || (ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t'))
}

fn valid_stored_reply(reply: &StoredReply) -> bool {
    match reply {
        StoredReply::Records { records } => {
            records.len() <= MAX_RECORDS
                && records.iter().all(valid_record)
                && records
                    .iter()
                    .map(|record| record.id)
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == records.len()
        }
        StoredReply::NotFound => true,
    }
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("LYRICS_STORE_PATH_UNSAFE".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata =
                    fs::symlink_metadata(path).map_err(|_| "LYRICS_STORE_UNAVAILABLE")?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("LYRICS_STORE_PATH_UNSAFE".into());
                }
            }
            Err(_) => return Err("LYRICS_STORE_CREATE_FAILED".into()),
        },
        Err(_) => return Err("LYRICS_STORE_UNAVAILABLE".into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "LYRICS_STORE_PERMISSION_FAILED")?;
    }
    Ok(())
}

fn read_private_regular(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("LYRICS_STORE_READ_FAILED".into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > STORE_FILE_MAX {
        return Err("LYRICS_STORE_INVALID".into());
    }
    let file = fs::File::open(path).map_err(|_| "LYRICS_STORE_READ_FAILED")?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(STORE_FILE_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "LYRICS_STORE_READ_FAILED")?;
    if bytes.len() as u64 > STORE_FILE_MAX {
        return Err("LYRICS_STORE_INVALID".into());
    }
    Ok(Some(bytes))
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let parent = path.parent().ok_or("LYRICS_STORE_WRITE_FAILED")?;
    let bytes = serde_json::to_vec(value).map_err(|_| "LYRICS_STORE_WRITE_FAILED")?;
    if bytes.len() as u64 > STORE_FILE_MAX {
        return Err("LYRICS_STORE_INVALID".into());
    }
    for _ in 0..8 {
        let tmp = parent.join(format!(
            ".lyrics-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&tmp) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("LYRICS_STORE_WRITE_FAILED".into()),
        };
        let write_result = file.write_all(&bytes).and_then(|_| file.sync_all());
        drop(file);
        if write_result.is_err() {
            let _ = fs::remove_file(&tmp);
            return Err("LYRICS_STORE_WRITE_FAILED".into());
        }
        if fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(&tmp);
            return Err("LYRICS_STORE_WRITE_FAILED".into());
        }
        return sync_parent(path);
    }
    Err("LYRICS_STORE_WRITE_FAILED".into())
}

fn sync_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|_| "LYRICS_STORE_WRITE_FAILED")?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_lyrics_choice(
    media: crate::audio::CurrentMedia,
    store: State<'_, LyricsStore>,
) -> Result<Option<LyricsChoice>, String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.load_choice(&media))
        .await
        .map_err(|_| String::from("LYRICS_STORE_READ_FAILED"))?
}

#[tauri::command]
pub async fn set_lyrics_choice(
    media: crate::audio::CurrentMedia,
    choice: LyricsChoice,
    store: State<'_, LyricsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.save_choice(&media, &choice))
        .await
        .map_err(|_| String::from("LYRICS_STORE_WRITE_FAILED"))?
}

#[tauri::command]
pub async fn migrate_legacy_lyrics_cache(
    entries: Vec<LegacyCacheEntry>,
    store: State<'_, LyricsStore>,
) -> Result<(), String> {
    let store = store.inner().clone();
    tauri::async_runtime::spawn_blocking(move || store.import_legacy(&entries))
        .await
        .map_err(|_| String::from("LYRICS_MIGRATION_FAILED"))?
}

fn valid_metadata(value: &str, allow_empty: bool) -> bool {
    value.len() <= 400
        && (allow_empty || !value.trim().is_empty())
        && !value.chars().any(char::is_control)
        && !value.to_ascii_lowercase().contains("http://")
        && !value.to_ascii_lowercase().contains("https://")
        && !value.to_ascii_lowercase().contains("file:")
        && !value.to_ascii_lowercase().contains("://")
        && !value.to_ascii_lowercase().contains("token=")
        && !value.to_ascii_lowercase().contains("api_key=")
        && !value.to_ascii_lowercase().contains("api-key=")
        && !value.to_ascii_lowercase().contains("auth=")
        && !value.starts_with('/')
        && !value.starts_with("\\\\")
        && !(value.as_bytes().get(1) == Some(&b':')
            && value
                .as_bytes()
                .get(2)
                .is_some_and(|separator| *separator == b'/' || *separator == b'\\'))
}

fn validate_query(query: &LyricsQuery) -> bool {
    valid_metadata(&query.title, false)
        && valid_metadata(&query.artist, false)
        && valid_metadata(&query.album, true)
        && query
            .duration
            .is_none_or(|duration| (1..=3600).contains(&duration))
}

fn optional_lyrics(value: Option<&Value>) -> Result<Option<String>, ()> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text))
            if text.len() <= MAX_TEXT
                && !text.chars().any(|ch| {
                    ch == '\0' || (ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t')
                }) =>
        {
            let text = text.trim();
            Ok((!text.is_empty()).then(|| text.to_owned()))
        }
        _ => Err(()),
    }
}

fn parse_record(value: &Value) -> Result<LyricsRecord, ()> {
    let object = value.as_object().ok_or(())?;
    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or(())?;
    let track_name = object.get("trackName").and_then(Value::as_str).ok_or(())?;
    let artist_name = object.get("artistName").and_then(Value::as_str).ok_or(())?;
    let album_name = object.get("albumName").and_then(Value::as_str).ok_or(())?;
    let duration = object
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|duration| duration.is_finite() && (1.0..=3600.0).contains(duration))
        .ok_or(())?;
    if !valid_metadata(track_name, false)
        || !valid_metadata(artist_name, false)
        || !valid_metadata(album_name, true)
    {
        return Err(());
    }
    let instrumental = object
        .get("instrumental")
        .and_then(Value::as_bool)
        .ok_or(())?;
    let has_word_sync = object
        .get("hasWordSync")
        .and_then(Value::as_bool)
        .ok_or(())?;
    let plain_lyrics = optional_lyrics(object.get("plainLyrics"))?;
    let synced_lyrics = optional_lyrics(object.get("syncedLyrics"))?;
    if !instrumental && plain_lyrics.is_none() && synced_lyrics.is_none() {
        return Err(());
    }
    Ok(LyricsRecord {
        id,
        track_name: track_name.to_owned(),
        artist_name: artist_name.to_owned(),
        album_name: album_name.to_owned(),
        duration,
        instrumental,
        has_word_sync,
        plain_lyrics,
        synced_lyrics,
    })
}

fn parse_records(bytes: &[u8]) -> Result<Vec<LyricsRecord>, ()> {
    if bytes.len() > MAX_BODY {
        return Err(());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let records = value.as_array().ok_or(())?;
    if records.len() > MAX_RECORDS {
        return Err(());
    }
    let mut ids = std::collections::HashSet::new();
    let mut parsed = Vec::with_capacity(records.len());
    for value in records {
        let record = parse_record(value)?;
        if !ids.insert(record.id) {
            return Err(());
        }
        parsed.push(record);
    }
    Ok(parsed)
}

fn search_url(query: &LyricsQuery) -> Result<reqwest::Url, ()> {
    if !validate_query(query) {
        return Err(());
    }
    let mut url = reqwest::Url::parse(&format!("{BASE_URL}/api/search")).map_err(|_| ())?;
    url.query_pairs_mut()
        .append_pair("track_name", query.title.trim())
        .append_pair("artist_name", query.artist.trim());
    if !query.album.trim().is_empty() {
        url.query_pairs_mut()
            .append_pair("album_name", query.album.trim());
    }
    Ok(url)
}

#[derive(Default)]
struct RequestGate {
    next_allowed: Option<Instant>,
    blocked: bool,
}
static REQUEST_GATE: OnceLock<tokio::sync::Mutex<RequestGate>> = OnceLock::new();

fn client() -> Result<reqwest::Client, LookupReply> {
    reqwest::Client::builder()
        .user_agent(crate::online_artwork::USER_AGENT)
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| LookupReply::ServiceUnavailable)
}

fn retry_after(response: &reqwest::Response) -> Duration {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok());
    retry_after_value(value)
}

fn retry_after_value(value: Option<&str>) -> Duration {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(60))
}

fn status_reply(status: reqwest::StatusCode) -> Option<LookupReply> {
    if status == reqwest::StatusCode::NOT_FOUND {
        Some(LookupReply::NotFound)
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Some(LookupReply::RateLimited)
    } else if status.is_server_error() {
        Some(LookupReply::ServiceUnavailable)
    } else if status != reqwest::StatusCode::OK {
        Some(LookupReply::InvalidResponse)
    } else {
        None
    }
}

async fn request(query: LyricsQuery) -> LookupReply {
    let Ok(url) = search_url(&query) else {
        return LookupReply::InvalidResponse;
    };
    let gate = REQUEST_GATE.get_or_init(|| tokio::sync::Mutex::new(RequestGate::default()));
    let mut permit = gate.lock().await;
    if permit.blocked {
        return LookupReply::RateLimited;
    }
    if let Some(next) = permit.next_allowed {
        tokio::time::sleep(next.saturating_duration_since(Instant::now())).await;
    }
    let started = Instant::now();
    let result = async {
        let client = client()?;
        let response = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    LookupReply::Timeout
                } else {
                    LookupReply::Offline
                }
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            let wait = retry_after(&response);
            if let Some(next) = Instant::now().checked_add(wait) {
                permit.next_allowed = Some(next);
            } else {
                permit.blocked = true;
            }
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Ok(LookupReply::RateLimited);
        }
        if let Some(reply) = status_reply(status) {
            return Ok(reply);
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.split(';').next().is_some_and(|media_type| {
                    media_type.trim().eq_ignore_ascii_case("application/json")
                })
            })
            || response
                .content_length()
                .is_some_and(|length| length > MAX_BODY as u64)
        {
            return Ok(LookupReply::InvalidResponse);
        }
        let mut response = response;
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) if body.len().saturating_add(chunk.len()) <= MAX_BODY => {
                    body.extend_from_slice(&chunk)
                }
                Ok(Some(_)) => return Ok(LookupReply::InvalidResponse),
                Ok(None) => break,
                Err(error) => {
                    return Err(if error.is_timeout() {
                        LookupReply::Timeout
                    } else {
                        LookupReply::Offline
                    })
                }
            }
        }
        let records = parse_records(&body).map_err(|_| LookupReply::InvalidResponse)?;
        Ok(if records.is_empty() {
            LookupReply::NotFound
        } else {
            LookupReply::Records {
                records,
                persisted: false,
            }
        })
    }
    .await;
    let minimum_next = started + MIN_INTERVAL;
    if permit.next_allowed.is_none_or(|next| next < minimum_next) {
        permit.next_allowed = Some(minimum_next);
    }
    result.unwrap_or_else(|error| error)
}

#[tauri::command]
pub async fn search_lrclib(
    query: LyricsQuery,
    refresh: Option<bool>,
    store: State<'_, LyricsStore>,
) -> Result<LookupReply, String> {
    let cache_key = query
        .cache_key
        .as_deref()
        .filter(|key| validate_cache_key(key).is_ok())
        .map(str::to_owned);
    let store = store.inner().clone();
    if !refresh.unwrap_or(false) {
        if let Some(key) = cache_key.as_deref() {
            let reader = store.clone();
            let key = key.to_owned();
            if let Ok(Ok(Some(cached))) =
                tauri::async_runtime::spawn_blocking(move || reader.load_result(&key)).await
            {
                return match cached {
                    StoredReply::Records { records } => Ok(LookupReply::Records {
                        records,
                        persisted: true,
                    }),
                    StoredReply::NotFound => Ok(LookupReply::NotFound),
                };
            }
        }
    }
    let reply = request(query).await;
    let Some(key) = cache_key else {
        return Ok(reply);
    };
    Ok(match reply {
        LookupReply::Records { records, .. } => {
            let writer = store.clone();
            let cached_records = records.clone();
            let persisted = tauri::async_runtime::spawn_blocking(move || {
                writer.save_result(
                    &key,
                    &StoredReply::Records {
                        records: cached_records,
                    },
                )
            })
            .await
            .is_ok_and(|result| result.is_ok());
            LookupReply::Records { records, persisted }
        }
        LookupReply::NotFound => {
            let writer = store;
            let _ = tauri::async_runtime::spawn_blocking(move || {
                writer.save_result(&key, &StoredReply::NotFound)
            })
            .await;
            LookupReply::NotFound
        }
        failure => failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        br#"[{"id":12,"trackName":"Synthetic","artistName":"Example","albumName":"Studio","duration":123.5,"instrumental":false,"hasWordSync":false,"plainLyrics":"Plain synthetic line","syncedLyrics":"[00:01.00]Synced line"}]"#.to_vec()
    }

    fn record(id: u64, text: &str) -> LyricsRecord {
        LyricsRecord {
            id,
            track_name: "Synthetic".into(),
            artist_name: "Example".into(),
            album_name: "Studio".into(),
            duration: 120.0,
            instrumental: false,
            has_word_sync: false,
            plain_lyrics: None,
            synced_lyrics: Some(format!("[00:01.00]{text}")),
        }
    }

    #[test]
    fn durable_results_survive_reopen_without_count_or_positive_age_eviction() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data/lyrics");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        let store = LyricsStore::open(root.clone()).unwrap();
        for index in 0..25 {
            let key = format!("[\"track {index}\",\"artist\",\"album\",120]");
            store
                .save_result(
                    &key,
                    &StoredReply::Records {
                        records: vec![record(index + 1, "offline persisted line")],
                    },
                )
                .unwrap();
        }
        let aged_key = "[\"track 0\",\"artist\",\"album\",120]";
        let aged_path = store.result_path(aged_key);
        let mut aged: ResultEnvelope =
            serde_json::from_slice(&fs::read(&aged_path).unwrap()).unwrap();
        aged.saved_at -= 40 * 24 * 60 * 60;
        atomic_json(&aged_path, &aged).unwrap();
        drop(store);
        let reopened = LyricsStore::open(root).unwrap();
        for index in 0..25 {
            let key = format!("[\"track {index}\",\"artist\",\"album\",120]");
            assert_eq!(
                reopened.load_result(&key).unwrap(),
                Some(StoredReply::Records {
                    records: vec![record(index + 1, "offline persisted line")]
                })
            );
        }
        let count = fs::read_dir(temp.path().join("data/lyrics/results-v1"))
            .unwrap()
            .count();
        assert_eq!(
            count, 25,
            "records are per-key and no global entry limit evicts old lyrics"
        );
    }

    #[test]
    fn legacy_migration_is_repeatable_and_preserves_positive_content_without_old_ttl() {
        let temp = tempfile::tempdir().unwrap();
        let store = LyricsStore::open(temp.path().join("lyrics")).unwrap();
        let key = "[\"synthetic\",\"artist\",\"album\",120]".to_owned();
        let entries = [LegacyCacheEntry {
            key: key.clone(),
            at: 1,
            result: StoredReply::Records {
                records: vec![record(1, "old positive")],
            },
        }];
        store.import_legacy(&entries).unwrap();
        store.import_legacy(&entries).unwrap();
        assert_eq!(
            store.load_result(&key).unwrap(),
            Some(entries[0].result.clone())
        );

        let newer = StoredReply::Records {
            records: vec![record(2, "newer durable result")],
        };
        store.save_result(&key, &newer).unwrap();
        store.import_legacy(&entries).unwrap();
        assert_eq!(
            store.load_result(&key).unwrap(),
            Some(newer),
            "migration never replaces a valid durable positive with a stale legacy copy"
        );
    }

    #[test]
    fn negative_cache_is_temporary_and_network_failures_have_no_storable_variant() {
        let temp = tempfile::tempdir().unwrap();
        let store = LyricsStore::open(temp.path().join("lyrics")).unwrap();
        let key = "[\"absent\",\"artist\",\"album\",null]";
        store.save_result(key, &StoredReply::NotFound).unwrap();
        assert_eq!(store.load_result(key).unwrap(), Some(StoredReply::NotFound));
        let path = store.result_path(key);
        let mut envelope: ResultEnvelope =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope.saved_at -= NEGATIVE_TTL_SECS + 1;
        atomic_json(&path, &envelope).unwrap();
        assert_eq!(store.load_result(key).unwrap(), None);
        assert!(valid_stored_reply(&StoredReply::Records {
            records: vec![record(1, "positive")]
        }));
    }

    #[test]
    fn a_refresh_with_no_match_does_not_replace_an_existing_positive_result() {
        let temp = tempfile::tempdir().unwrap();
        let store = LyricsStore::open(temp.path().join("lyrics")).unwrap();
        let key = "[\"same track\",\"artist\",\"album\",120]";
        let positive = StoredReply::Records {
            records: vec![record(1, "valid existing lyric")],
        };
        store.save_result(key, &positive).unwrap();
        store.save_result(key, &StoredReply::NotFound).unwrap();
        assert_eq!(store.load_result(key).unwrap(), Some(positive));
    }

    #[test]
    fn manual_choice_survives_reopen_and_is_bound_to_stable_media_not_queue_position() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lyrics");
        let store = LyricsStore::open(root.clone()).unwrap();
        let first = crate::audio::CurrentMedia::Local {
            uri: "sqlite-track://source/track".into(),
            queue_index: 1,
        };
        let moved = crate::audio::CurrentMedia::Local {
            uri: "sqlite-track://source/track".into(),
            queue_index: 8,
        };
        let other = crate::audio::CurrentMedia::Local {
            uri: "sqlite-track://other/track".into(),
            queue_index: 1,
        };
        let choice = LyricsChoice::Manual {
            record: record(44, "manual choice"),
        };
        store.save_choice(&first, &choice).unwrap();
        drop(store);
        let reopened = LyricsStore::open(root).unwrap();
        assert_eq!(reopened.load_choice(&moved).unwrap(), Some(choice));
        assert_eq!(reopened.load_choice(&other).unwrap(), None);
        reopened
            .save_choice(&moved, &LyricsChoice::Automatic)
            .unwrap();
        assert_eq!(
            reopened.load_choice(&first).unwrap(),
            Some(LyricsChoice::Automatic)
        );
    }

    #[test]
    fn invalid_cache_payload_and_unsupported_media_identity_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let store = LyricsStore::open(temp.path().join("lyrics")).unwrap();
        assert!(store.save_result("", &StoredReply::NotFound).is_err());
        assert!(store
            .save_result(
                "ok",
                &StoredReply::Records {
                    records: vec![record(1, "one"), record(1, "duplicate")]
                }
            )
            .is_err());
        let unsupported = crate::audio::CurrentMedia::Radio {
            station_id: "station".into(),
            queue_index: 0,
        };
        assert!(store.load_choice(&unsupported).is_err());
    }

    #[test]
    fn unavailable_storage_is_reported_to_commands_without_requiring_app_startup_to_fail() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("not-a-directory");
        fs::write(&root, b"occupied path").unwrap();
        let store = LyricsStore::unavailable(root);
        assert_eq!(
            store.load_result("valid-key").unwrap_err(),
            "LYRICS_STORE_PATH_UNSAFE"
        );
    }

    #[test]
    #[cfg(unix)]
    fn failed_choice_write_keeps_previous_choice_intact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lyrics");
        let store = LyricsStore::open(root.clone()).unwrap();
        let media = crate::audio::CurrentMedia::Local {
            uri: "sqlite-track://source/track".into(),
            queue_index: 1,
        };
        let previous = LyricsChoice::Manual {
            record: record(44, "previous choice"),
        };
        store.save_choice(&media, &previous).unwrap();
        let choices_dir = root.join("choices-v1");
        let backup_dir = root.join("choices-backup");
        fs::rename(&choices_dir, &backup_dir).unwrap();
        std::os::unix::fs::symlink(&backup_dir, &choices_dir).unwrap();
        let result = store.save_choice(
            &media,
            &LyricsChoice::Manual {
                record: record(45, "new choice"),
            },
        );
        fs::remove_file(&choices_dir).unwrap();
        fs::rename(&backup_dir, &choices_dir).unwrap();
        assert!(result.is_err());
        assert_eq!(store.load_choice(&media).unwrap(), Some(previous));
    }

    #[test]
    fn validates_whitelisted_query_and_encodes_stable_v1_search_parameters() {
        let query = LyricsQuery {
            title: "Live & (synthetic)".into(),
            artist: "A/B".into(),
            album: "Album?".into(),
            duration: Some(120),
            cache_key: None,
        };
        let url = search_url(&query).unwrap();
        assert_eq!(url.host_str(), Some("lrclib.net"));
        assert_eq!(url.path(), "/api/search");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("track_name".into(), "Live & (synthetic)".into()),
                ("artist_name".into(), "A/B".into()),
                ("album_name".into(), "Album?".into())
            ]
        );
        let invalid = LyricsQuery {
            title: "https://private.invalid".into(),
            artist: "A".into(),
            album: String::new(),
            duration: None,
            cache_key: None,
        };
        assert!(search_url(&invalid).is_err());
        for title in [
            "/private/file.flac",
            "C:\\Private\\track.flac",
            "\\\\server\\share\\track.flac",
            "sqlite-track://internal/id",
            "token=PRIVATE",
        ] {
            let invalid = LyricsQuery {
                title: title.into(),
                artist: "Artist".into(),
                album: String::new(),
                duration: None,
                cache_key: None,
            };
            assert!(
                search_url(&invalid).is_err(),
                "disallowed metadata: {title}"
            );
        }
    }

    #[test]
    fn parses_bounded_v1_records_preserving_recording_metadata_and_lyrics() {
        let records = parse_records(&fixture()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].duration, 123.5);
        assert_eq!(
            records[0].plain_lyrics.as_deref(),
            Some("Plain synthetic line")
        );
        assert_eq!(
            records[0].synced_lyrics.as_deref(),
            Some("[00:01.00]Synced line")
        );
        assert!(records[0].has_word_sync == false);
    }

    #[test]
    fn rejects_malformed_duplicate_unbounded_and_empty_lyric_records_atomically() {
        assert!(parse_records(b"{}").is_err());
        let duplicate = fixture();
        let value: Value = serde_json::from_slice(&duplicate).unwrap();
        let mut repeated = value.as_array().unwrap().clone();
        repeated.push(repeated[0].clone());
        assert!(parse_records(serde_json::to_string(&repeated).unwrap().as_bytes()).is_err());
        for raw in [
            br#"[{"id":1,"trackName":"x","artistName":"a","albumName":"","duration":0,"instrumental":false,"hasWordSync":false,"plainLyrics":"x","syncedLyrics":null}]"#.as_slice(),
            br#"[{"id":1,"trackName":"x","artistName":"a","albumName":"","duration":2,"instrumental":false,"hasWordSync":false,"plainLyrics":null,"syncedLyrics":null}]"#.as_slice(),
        ] { assert!(parse_records(raw).is_err()); }
        assert!(parse_records(&vec![b' '; MAX_BODY + 1]).is_err());
    }

    #[test]
    fn distinguishes_instrumental_records_and_rejects_word_sync_metadata_only_as_metadata() {
        let raw = br#"[{"id":2,"trackName":"Instrumental","artistName":"Example","albumName":"Score","duration":80,"instrumental":true,"hasWordSync":false,"plainLyrics":null,"syncedLyrics":null}]"#;
        let value = parse_records(raw).unwrap();
        assert!(value[0].instrumental);
        let mut word = fixture();
        let text = String::from_utf8(word.clone())
            .unwrap()
            .replace("false,\"plainLyrics\"", "true,\"plainLyrics\"");
        word = text.into_bytes();
        assert!(parse_records(&word).unwrap()[0].has_word_sync);
    }

    #[test]
    fn classifies_http_statuses_without_parsing_error_bodies_or_caching_failures() {
        assert_eq!(status_reply(reqwest::StatusCode::OK), None);
        assert_eq!(
            status_reply(reqwest::StatusCode::NOT_FOUND),
            Some(LookupReply::NotFound)
        );
        assert_eq!(
            status_reply(reqwest::StatusCode::TOO_MANY_REQUESTS),
            Some(LookupReply::RateLimited)
        );
        assert_eq!(
            status_reply(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            Some(LookupReply::ServiceUnavailable)
        );
        assert_eq!(
            status_reply(reqwest::StatusCode::BAD_GATEWAY),
            Some(LookupReply::ServiceUnavailable)
        );
        assert_eq!(
            status_reply(reqwest::StatusCode::BAD_REQUEST),
            Some(LookupReply::InvalidResponse)
        );
        assert_eq!(retry_after_value(Some("180")), Duration::from_secs(180));
        assert_eq!(retry_after_value(Some("invalid")), Duration::from_secs(60));
        assert_eq!(retry_after_value(None), Duration::from_secs(60));
    }
}
