//! Subsonic 1.16.1 subset. Only explicit, configured-origin requests; safe errors.
use super::{AUTH, INVALID, PROTOCOL, UNAVAILABLE};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Read, time::Duration};
pub const PAGE_SIZE: usize = 72;
pub fn id(value: &str) -> bool {
    !value.is_empty()
        && !matches!(value, "." | "..")
        && value.len() <= 192
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}
pub fn random() -> Result<String, String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| UNAVAILABLE)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub connection_id: String,
    pub server: String,
    pub username: String,
    pub password: String,
}
impl Credentials {
    pub fn validate(&self) -> Result<(), String> {
        if !id(&self.connection_id)
            || self.username.trim().is_empty()
            || self.username.len() > 256
            || self.username.chars().any(char::is_control)
            || self.password.is_empty()
            || self.password.len() > 4096
            || self.password.chars().any(char::is_control)
        {
            return Err(INVALID.into());
        }
        server(&self.server)?;
        Ok(())
    }
    pub fn url(&self, endpoint: &str, params: &[(&str, String)]) -> Result<Url, String> {
        let base = server(&self.server)?;
        let mut url = base
            .join(&format!("rest/{endpoint}.view"))
            .map_err(|_| INVALID)?;
        let salt = random()?;
        let token = format!("{:x}", md5::compute(format!("{}{salt}", self.password)));
        url.query_pairs_mut()
            .extend_pairs([
                ("u", self.username.as_str()),
                ("t", token.as_str()),
                ("s", salt.as_str()),
                ("v", "1.16.1"),
                ("c", "Sonante"),
                ("f", "json"),
            ])
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        Ok(url)
    }
}
pub fn server(value: &str) -> Result<Url, String> {
    if value.len() > 2048 || value.chars().any(char::is_control) || value.contains('\\') {
        return Err(INVALID.into());
    }
    let mut url = Url::parse(value.trim()).map_err(|_| INVALID)?;
    if !["http", "https"].contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(INVALID.into());
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}
pub fn client() -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(20))
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .user_agent(concat!("Sonante/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| UNAVAILABLE.into())
}
pub fn http(status: reqwest::StatusCode) -> Result<(), String> {
    match status.as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(AUTH.into()),
        429 => Err("NAVIDROME_RATE_LIMITED".into()),
        _ => Err(UNAVAILABLE.into()),
    }
}
pub async fn bytes(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    http(response.status())?;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(PROTOCOL.into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| UNAVAILABLE)? {
        if bytes.len() + chunk.len() > limit {
            return Err(PROTOCOL.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub fn envelope(value: Value) -> Result<Value, String> {
    let root = value
        .get("subsonic-response")
        .filter(|v| v.is_object())
        .ok_or(PROTOCOL)?;
    if root.get("status").and_then(Value::as_str) == Some("failed") {
        return Err(match root["error"]["code"].as_u64() {
            Some(40 | 41 | 50) => AUTH,
            _ => PROTOCOL,
        }
        .into());
    }
    if root.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(PROTOCOL.into());
    }
    Ok(root.clone())
}
pub async fn request(
    client: &Client,
    credentials: &Credentials,
    endpoint: &str,
    params: &[(&str, String)],
) -> Result<Value, String> {
    Ok(request_counted(client, credentials, endpoint, params).await?.0)
}
pub async fn request_counted(client: &Client, credentials: &Credentials, endpoint: &str, params: &[(&str, String)]) -> Result<(Value, Option<usize>), String> {
    let response = client
        .get(credentials.url(endpoint, params)?)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    let total = total_count(response.headers());
    Ok((envelope(serde_json::from_slice(&bytes(response, 2 * 1024 * 1024).await?).map_err(|_| PROTOCOL)?)?, total))
}
fn text(value: Option<&Value>, required: bool) -> Result<String, String> {
    let value = value.and_then(Value::as_str).unwrap_or("");
    if (required && value.trim().is_empty())
        || value.len() > 2048
        || value.chars().any(char::is_control)
    {
        return Err(PROTOCOL.into());
    }
    Ok(value.to_owned())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub cover_id: Option<String>,
    pub song_count: Option<u32>,
}
pub fn album(value: &Value) -> Result<Album, String> {
    let album = Album {
        id: text(value.get("id"), true)?,
        title: text(value.get("name"), true)?,
        artist: text(value.get("artist"), false)?,
        year: value["year"]
            .as_u64()
            .filter(|n| *n <= 9999)
            .map(|n| n as u32),
        cover_id: value["coverArt"]
            .as_str()
            .filter(|v| id(v))
            .map(str::to_owned),
        song_count: value["songCount"]
            .as_u64()
            .filter(|n| *n <= 100000)
            .map(|n| n as u32),
    };
    if !id(&album.id) {
        return Err(PROTOCOL.into());
    }
    Ok(album)
}
#[derive(Clone, Serialize)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<f64>,
    pub disc: u32,
    pub track: u32,
}
pub fn songs(value: &Value) -> Result<Vec<Song>, String> {
    let rows = value
        .get("song")
        .and_then(Value::as_array)
        .ok_or(PROTOCOL)?;
    if rows.len() > 5000 {
        return Err(PROTOCOL.into());
    }
    let artist = text(value.get("artist"), false)?;
    let title = text(value.get("name"), false)?;
    let mut out = rows
        .iter()
        .map(|v| {
            let song = Song {
                id: text(v.get("id"), true)?,
                title: text(v.get("title"), true)?,
                artist: if v["artist"].as_str().is_some_and(|v| !v.trim().is_empty()) {
                    text(v.get("artist"), false)?
                } else {
                    artist.clone()
                },
                album: title.clone(),
                duration: v["duration"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 86400.0),
                disc: v["discNumber"].as_u64().filter(|n| *n < 1000).unwrap_or(1) as u32,
                track: v["track"].as_u64().filter(|n| *n < 100000).unwrap_or(0) as u32,
            };
            if !id(&song.id) {
                return Err(PROTOCOL.into());
            }
            Ok(song)
        })
        .collect::<Result<Vec<_>, String>>()?;
    out.sort_by_key(|song| (song.disc, song.track));
    Ok(out)
}
pub fn cover_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn credentials() -> Credentials {
        Credentials {
            connection_id: "fixture".into(),
            server: "https://music.invalid/music/".into(),
            username: "user & name".into(),
            password: "secret?token".into(),
        }
    }
    #[test]
    fn authentication_is_salted_encoded_and_preserves_reverse_proxy_prefix() {
        let c = credentials();
        let a = c.url("ping", &[]).unwrap();
        let b = c.url("ping", &[]).unwrap();
        assert_eq!(a.path(), "/music/rest/ping.view");
        let q = a.query_pairs().collect::<std::collections::HashMap<_, _>>();
        assert_eq!(q["u"], "user & name");
        assert_eq!(
            q["t"],
            format!("{:x}", md5::compute(format!("{}{}", c.password, q["s"])))
        );
        assert_ne!(a, b);
        assert!(!a.as_str().contains("secret"));
        for bad in [
            "file:///a",
            "ftp://server",
            "https://user:password@server/",
            "https://server/?token=x",
            "https://server/#x",
        ] {
            assert!(server(bad).is_err());
        }
    }
    #[test]
    fn errors_are_safe_and_missing_envelopes_never_become_empty_success() {
        assert!(envelope(serde_json::json!({})).is_err());
        assert_eq!(envelope(serde_json::json!({"subsonic-response":{"status":"failed","error":{"code":40,"message":"PRIVATE"}}})).unwrap_err(),AUTH);
        assert!(!envelope(serde_json::json!({"subsonic-response":{"status":"failed","error":{"code":70,"message":"PRIVATE"}}})).unwrap_err().contains("PRIVATE"));
    }
    #[test]
    fn catalog_metadata_and_disc_order_are_bounded() {
        let a=album(&serde_json::json!({"id":"al-123","name":"Album","artist":"Artist","coverArt":"al-123"})).unwrap();
        assert_eq!(a.cover_id.as_deref(), Some("al-123"));
        assert!(album(&serde_json::json!({"id":"../bad/id","name":"Album"})).is_err());
        let tracks=songs(&serde_json::json!({"name":"Album","artist":"Parent","song":[{"id":"b","title":"B","discNumber":2,"track":1},{"id":"a","title":"A","discNumber":1,"track":2}]})).unwrap();
        assert_eq!(tracks[0].id, "a");
        assert_eq!(tracks[1].artist, "Parent");
        assert!(songs(&serde_json::json!({"name":"Album"})).is_err());
        assert!(cover_type(b"<svg onload='bad'>").is_none());
    }
}

fn total_count(headers: &reqwest::header::HeaderMap) -> Option<usize> {
    headers.get("x-total-count").and_then(|v|v.to_str().ok()).filter(|v|!v.is_empty()&&v.bytes().all(|b|b.is_ascii_digit())).and_then(|v|v.parse::<usize>().ok()).filter(|v|*v<=10_000_000)
}
#[cfg(test)]
mod total_tests {
    #[test]
    fn total_header_is_optional_and_never_replaced_by_page_length() {
        let mut h=reqwest::header::HeaderMap::new();assert_eq!(super::total_count(&h),None);
        for (raw,expected) in [("4000",Some(4000)),("0",Some(0)),("-1",None),("not-a-count",None),("10000001",None)] {
            h.insert("x-total-count",raw.parse().unwrap());assert_eq!(super::total_count(&h),expected);
        }
    }
}
