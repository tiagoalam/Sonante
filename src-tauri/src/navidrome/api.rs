//! Subsonic/OpenSubsonic subset. Only explicit, configured-origin requests; safe errors.
use super::{AUTH, INVALID, PROTOCOL, UNAVAILABLE};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Read,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
pub const PAGE_SIZE: usize = 72;
const API_VERSIONS: &[&str] = &[
    "1.16.1", "1.16.0", "1.15.0", "1.14.0", "1.13.0", "1.12.0", "1.11.0", "1.10.2",
    "1.9.0", "1.8.0",
];
const VERSION_MISMATCH: &str = "NAVIDROME_SERVER_API_VERSION_MISMATCH";
pub fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 192
        && !value.chars().any(char::is_control)
}
pub fn proxy_id(value: &str) -> Option<String> {
    id(value).then(|| {
        value
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    })
}
pub fn decode_proxy_id(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 384 || value.len() % 2 != 0 {
        return None;
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => Some(byte - b'0'),
                b'a'..=b'f' => Some(byte - b'a' + 10),
                b'A'..=b'F' => Some(byte - b'A' + 10),
                _ => None,
            };
            Some((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect::<Option<Vec<_>>>()?;
    let decoded = String::from_utf8(bytes).ok()?;
    id(&decoded).then_some(decoded)
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    #[default]
    Password,
    ApiKey,
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
    /// Local account label for favorites. API-key mode never sends this value.
    pub username: String,
    /// Password for classic token/salt auth, or API key in API-key mode.
    pub password: String,
    #[serde(default)]
    pub auth_method: AuthMethod,
    #[serde(skip)]
    pub api_version_index: Arc<AtomicUsize>,
}
impl Credentials {
    pub fn validate(&self) -> Result<(), String> {
        if !id(&self.connection_id)
            || self.username.len() > 256
            || self.username.chars().any(char::is_control)
            || self.password.is_empty()
            || self.password.len() > 4096
            || self.password.chars().any(char::is_control)
            || (self.auth_method == AuthMethod::Password && self.username.trim().is_empty())
        {
            return Err(INVALID.into());
        }
        server(&self.server)?;
        if self.auth_method == AuthMethod::ApiKey {
            let mut probe = Url::parse("http://localhost/").map_err(|_| INVALID)?;
            probe
                .query_pairs_mut()
                .append_pair("apiKey", &self.password);
            if probe.query().is_some_and(|query| query.len() >= 2048) {
                return Err(INVALID.into());
            }
        }
        Ok(())
    }
    pub fn url(&self, endpoint: &str, params: &[(&str, String)]) -> Result<Url, String> {
        self.url_at(
            endpoint,
            params,
            self.api_version_index.load(Ordering::Acquire),
        )
    }
    fn url_at(
        &self,
        endpoint: &str,
        params: &[(&str, String)],
        version_index: usize,
    ) -> Result<Url, String> {
        let base = server(&self.server)?;
        let mut url = base
            .join(&format!("rest/{endpoint}.view"))
            .map_err(|_| INVALID)?;
        let version = API_VERSIONS
            .get(version_index)
            .copied()
            .unwrap_or(API_VERSIONS[API_VERSIONS.len() - 1]);
        {
            let mut query = url.query_pairs_mut();
            match self.auth_method {
                AuthMethod::Password => {
                    let salt = random()?;
                    let token = format!("{:x}", md5::compute(format!("{}{salt}", self.password)));
                    query
                        .append_pair("u", &self.username)
                        .append_pair("t", &token)
                        .append_pair("s", &salt);
                }
                AuthMethod::ApiKey => {
                    query.append_pair("apiKey", &self.password);
                }
            }
            query
                .append_pair("v", version)
                .append_pair("c", "Sonante")
                .append_pair("f", "json")
                .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        }
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
        401 => Err(AUTH.into()),
        403 => Err(super::PERMISSION.into()),
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
            Some(20) => PROTOCOL,
            Some(30) => VERSION_MISMATCH,
            Some(40 | 44) => AUTH,
            Some(41 | 42) => super::AUTH_METHOD_UNSUPPORTED,
            Some(43) => PROTOCOL,
            Some(50) => super::PERMISSION,
            _ => PROTOCOL,
        }
        .into());
    }
    if root.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(PROTOCOL.into());
    }
    Ok(root.clone())
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ServerInfo {
    pub server_type: Option<String>,
    pub server_version: Option<String>,
    pub api_version: Option<String>,
    pub open_subsonic: bool,
    pub extensions: Vec<Extension>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Extension {
    pub name: String,
    pub versions: Vec<u32>,
}

fn bounded_server_text(value: Option<&Value>, limit: usize) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control))
        .then(|| value.to_owned())
}

fn parse_extensions(root: &Value) -> Result<Vec<Extension>, String> {
    let Some(rows) = root.get("openSubsonicExtensions") else {
        return Ok(Vec::new());
    };
    let rows = rows.as_array().filter(|rows| rows.len() <= 64).ok_or(PROTOCOL)?;
    let mut extensions = Vec::with_capacity(rows.len());
    for row in rows {
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 64
                    && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
            .ok_or(PROTOCOL)?;
        let versions = row
            .get("versions")
            .and_then(Value::as_array)
            .filter(|versions| !versions.is_empty() && versions.len() <= 16)
            .ok_or(PROTOCOL)?
            .iter()
            .map(|version| {
                version
                    .as_u64()
                    .filter(|version| (1..=100).contains(version))
                    .map(|version| version as u32)
                    .ok_or(PROTOCOL)
            })
            .collect::<Result<Vec<_>, _>>()?;
        extensions.push(Extension {
            name: name.to_owned(),
            versions,
        });
    }
    Ok(extensions)
}

pub fn server_info(root: &Value, extensions: Vec<Extension>) -> ServerInfo {
    ServerInfo {
        server_type: bounded_server_text(root.get("type"), 128),
        server_version: bounded_server_text(root.get("serverVersion"), 128),
        api_version: bounded_server_text(root.get("version"), 32),
        open_subsonic: root.get("openSubsonic").and_then(Value::as_bool) == Some(true),
        extensions,
    }
}

pub fn api_key_auth_supported(extensions: &[Extension]) -> bool {
    extensions.iter().any(|extension| {
        extension.name == "apiKeyAuthentication" && extension.versions.contains(&1)
    })
}

/// OpenSubsonic specifies this endpoint as public. A missing endpoint is a
/// normal capability result for legacy Subsonic servers, not an empty catalog.
pub async fn open_subsonic_extensions(
    client: &Client,
    server_url: &str,
) -> Result<Vec<Extension>, String> {
    let mut url = server(server_url)?
        .join("rest/getOpenSubsonicExtensions.view")
        .map_err(|_| INVALID)?;
    url.query_pairs_mut()
        .append_pair("v", "1.14.0")
        .append_pair("c", "Sonante")
        .append_pair("f", "json");
    let response = client
        .get(url)
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    if matches!(response.status().as_u16(), 404 | 405) {
        return Ok(Vec::new());
    }
    let root = envelope(
        serde_json::from_slice(&bytes(response, 128 * 1024).await?).map_err(|_| PROTOCOL)?,
    )?;
    parse_extensions(&root)
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
    request_counted_with_timeout(client, credentials, endpoint, params, Duration::from_secs(15))
        .await
}
async fn request_counted_with_timeout(
    client: &Client,
    credentials: &Credentials,
    endpoint: &str,
    params: &[(&str, String)],
    timeout: Duration,
) -> Result<(Value, Option<usize>), String> {
    loop {
        let version_index = credentials.api_version_index.load(Ordering::Acquire);
        let response = client
            .get(credentials.url_at(endpoint, params, version_index)?)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| UNAVAILABLE)?;
        let total = total_count(response.headers());
        let body = bytes(response, 2 * 1024 * 1024).await?;
        let value = serde_json::from_slice(&body).map_err(|_| PROTOCOL)?;
        match envelope(value) {
            Err(error) if error == VERSION_MISMATCH => {
                if version_index + 1 >= API_VERSIONS.len() {
                    return Err(PROTOCOL.into());
                }
                let _ = credentials.api_version_index.compare_exchange(
                    version_index,
                    version_index + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            result => return Ok((result?, total)),
        }
    }
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn credentials() -> Credentials {
        Credentials {
            connection_id: "fixture".into(),
            server: "https://music.invalid/music/".into(),
            username: "user & name".into(),
            password: "secret?token".into(),
            auth_method: AuthMethod::Password,
            api_version_index: Default::default(),
        }
    }
    async fn fixture_server(
        responses: Vec<String>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        fixture_server_with_status(responses.into_iter().map(|body| (200, body)).collect()).await
    }
    async fn fixture_server_with_status(
        responses: Vec<(u16, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        fixture_server_with_details(
            responses
                .into_iter()
                .map(|(status, body)| (status, body, Vec::new(), None))
                .collect(),
        )
        .await
    }
    async fn fixture_server_with_details(
        responses: Vec<(u16, String, Vec<String>, Option<usize>)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (status, body, headers, content_length) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_fixture_request(&mut stream).await);
                let mut response = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
                    content_length.unwrap_or(body.len()),
                );
                for header in headers {
                    response.push_str(&header);
                    response.push_str("\r\n");
                }
                response.push_str(&format!("Connection: close\r\n\r\n{body}"));
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}/music/"), handle)
    }
    async fn read_fixture_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0u8; 1024];
            let count = stream.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
            assert!(request.len() < 8192);
        }
        String::from_utf8(request).unwrap()
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
    fn api_key_auth_omits_password_username_and_token_parameters() {
        let mut c = credentials();
        c.auth_method = AuthMethod::ApiKey;
        c.password = "key / + secret".into();
        let url = c.url("ping", &[]).unwrap();
        assert_eq!(url.path(), "/music/rest/ping.view");
        let query = url.query_pairs().collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query["apiKey"], c.password);
        assert!(!query.contains_key("u"));
        assert!(!query.contains_key("t"));
        assert!(!query.contains_key("s"));
        assert!(!url.as_str().contains("key / + secret"));
    }
    #[test]
    fn remote_ids_are_opaque_but_bounded_and_control_free() {
        for value in ["song-1", "track/β?part=1", "曲目#2", "percent%id", ".", ".."] {
            assert!(id(value));
            assert_eq!(decode_proxy_id(&proxy_id(value).unwrap()).as_deref(), Some(value));
        }
        assert!(!id(""));
        assert!(!id("\u{7f}control"));
        assert!(!id(&"x".repeat(193)));
    }
    #[tokio::test]
    async fn open_subsonic_capabilities_are_public_and_api_key_is_used_only_when_advertised() {
        let extensions = r#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"Gonic","serverVersion":"2.0","openSubsonic":true,"openSubsonicExtensions":[{"name":"apiKeyAuthentication","versions":[1]},{"name":"songLyrics","versions":[1]}]}}"#;
        let ping = r#"{"subsonic-response":{"status":"ok","version":"1.16.1","type":"Gonic","serverVersion":"2.0","openSubsonic":true}}"#;
        let (server, requests) = fixture_server(vec![extensions.into(), ping.into()]).await;
        let client = client().unwrap();
        let extensions = open_subsonic_extensions(&client, &server).await.unwrap();
        assert!(api_key_auth_supported(&extensions));
        assert!(!api_key_auth_supported(&[Extension {
            name: "apiKeyAuthentication".into(),
            versions: vec![2],
        }]));
        let mut c = credentials();
        c.server = server;
        c.username = "local account label".into();
        c.password = "API_KEY_SECRET".into();
        c.auth_method = AuthMethod::ApiKey;
        let ping = request(&client, &c, "ping", &[]).await.unwrap();
        let server_info = server_info(&ping, extensions);
        assert_eq!(server_info.server_type.as_deref(), Some("Gonic"));
        assert_eq!(server_info.server_version.as_deref(), Some("2.0"));
        assert!(server_info.open_subsonic);
        assert!(!serde_json::to_string(&server_info).unwrap().contains("API_KEY_SECRET"));
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /music/rest/getOpenSubsonicExtensions.view?"));
        assert!(!requests[0].contains("API_KEY_SECRET"));
        assert!(requests[1].contains("apiKey=API_KEY_SECRET"));
        for forbidden in ["&u=", "&t=", "&s="] {
            assert!(!requests[1].contains(forbidden));
        }
    }
    #[tokio::test]
    async fn missing_optional_extension_endpoint_is_a_supported_legacy_result() {
        let (server, requests) = fixture_server_with_status(vec![(404, "not found".into())]).await;
        let extensions = open_subsonic_extensions(&client().unwrap(), &server)
            .await
            .unwrap();
        assert!(extensions.is_empty());
        assert!(!api_key_auth_supported(&extensions));
        assert_eq!(requests.await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn http_failures_never_become_empty_catalog_responses() {
        for (status, expected) in [
            (401, AUTH),
            (403, super::super::PERMISSION),
            (404, UNAVAILABLE),
            (429, "NAVIDROME_RATE_LIMITED"),
            (503, UNAVAILABLE),
        ] {
            let (server, requests) =
                fixture_server_with_status(vec![(status, "ignored body".into())]).await;
            let mut c = credentials();
            c.server = server;
            assert_eq!(
                request(&client().unwrap(), &c, "ping", &[])
                    .await
                    .unwrap_err(),
                expected,
                "unexpected mapping for HTTP {status}"
            );
            assert_eq!(requests.await.unwrap().len(), 1);
        }
    }
    #[tokio::test]
    async fn redirects_are_not_followed_and_oversized_json_is_rejected() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let redirect_server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let first_request = read_fixture_request(&mut first).await;
            first
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{address}/music/redirect-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();

            let mut requests = vec![first_request];
            if let Ok(Ok((mut second, _))) = tokio::time::timeout(
                Duration::from_millis(250),
                listener.accept(),
            )
            .await
            {
                requests.push(read_fixture_request(&mut second).await);
                let body = r#"{"subsonic-response":{"status":"ok"}}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                second.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let mut c = credentials();
        c.server = format!("http://{address}/music/");
        assert_eq!(
            request(&client().unwrap(), &c, "ping", &[])
                .await
                .unwrap_err(),
            UNAVAILABLE
        );
        assert_eq!(redirect_server.await.unwrap().len(), 1);

        let (server, requests) = fixture_server_with_details(vec![(
            200,
            String::new(),
            Vec::new(),
            Some(2 * 1024 * 1024 + 1),
        )])
        .await;
        c.server = server;
        assert_eq!(
            request(&client().unwrap(), &c, "ping", &[])
                .await
                .unwrap_err(),
            PROTOCOL
        );
        assert_eq!(requests.await.unwrap().len(), 1);

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let oversized_server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_fixture_request(&mut stream).await;
            let body = vec![b'x'; 2 * 1024 * 1024 + 1];
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n",
                body.len()
            )
            .into_bytes();
            response.extend_from_slice(&body);
            response.extend_from_slice(b"\r\n0\r\n\r\n");
            stream.write_all(&response).await.unwrap();
        });
        c.server = format!("http://{address}/music/");
        assert_eq!(
            request(&client().unwrap(), &c, "ping", &[])
                .await
                .unwrap_err(),
            PROTOCOL
        );
        oversized_server.await.unwrap();
    }
    #[tokio::test]
    async fn older_subsonic_server_retries_only_after_protocol_version_error() {
        let rejected = r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":30,"message":"ignored"}}}"#;
        let accepted = r#"{"subsonic-response":{"status":"ok","version":"1.16.0","type":"Legacy"}}"#;
        let (server, requests) = fixture_server(vec![rejected.into(), accepted.into()]).await;
        let mut c = credentials();
        c.server = server;
        let root = request(&client().unwrap(), &c, "ping", &[]).await.unwrap();
        assert_eq!(root["type"], "Legacy");
        assert_eq!(c.api_version_index.load(Ordering::Acquire), 1);
        let requests = requests.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("v=1.16.1"));
        assert!(requests[1].contains("v=1.16.0"));
    }
    #[tokio::test]
    async fn incompatible_api_errors_never_become_an_empty_success_or_retry_as_version_fallback() {
        let incompatible = r#"{"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":20,"message":"ignored"}}}"#;
        let (server, requests) = fixture_server(vec![incompatible.into()]).await;
        let mut c = credentials();
        c.server = server;
        assert_eq!(request(&client().unwrap(), &c, "ping", &[]).await.unwrap_err(), PROTOCOL);
        assert_eq!(c.api_version_index.load(Ordering::Acquire), 0);
        assert_eq!(requests.await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn invalid_json_and_unavailable_server_are_errors_not_empty_results() {
        let (server, requests) = fixture_server(vec!["not-json".into()]).await;
        let mut c = credentials();
        c.server = server;
        assert_eq!(
            request(&client().unwrap(), &c, "ping", &[])
                .await
                .unwrap_err(),
            PROTOCOL
        );
        assert_eq!(requests.await.unwrap().len(), 1);

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        c.server = format!("http://{address}/");
        assert_eq!(
            request(&client().unwrap(), &c, "ping", &[])
                .await
                .unwrap_err(),
            UNAVAILABLE
        );
    }
    #[tokio::test]
    async fn slow_server_respects_the_request_deadline() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let body = r#"{"subsonic-response":{"status":"ok"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
        let mut c = credentials();
        c.server = format!("http://{address}/");
        assert_eq!(
            request_counted_with_timeout(
                &client().unwrap(),
                &c,
                "ping",
                &[],
                Duration::from_millis(10),
            )
            .await
            .unwrap_err(),
            UNAVAILABLE
        );
        server.await.unwrap();
    }
    #[test]
    fn errors_are_safe_and_missing_envelopes_never_become_empty_success() {
        assert!(envelope(serde_json::json!({})).is_err());
        assert_eq!(envelope(serde_json::json!({"subsonic-response":{"status":"failed","error":{"code":40,"message":"PRIVATE"}}})).unwrap_err(),AUTH);
        assert!(!envelope(serde_json::json!({"subsonic-response":{"status":"failed","error":{"code":70,"message":"PRIVATE"}}})).unwrap_err().contains("PRIVATE"));
        assert_eq!(envelope(serde_json::json!({"subsonic-response":{"status":"failed","error":{"code":50,"message":"PRIVATE"}}})).unwrap_err(), super::super::PERMISSION);
        assert_eq!(envelope(serde_json::json!({"subsonic-response":{"status":"failed","error":{"code":41,"message":"PRIVATE"}}})).unwrap_err(), super::super::AUTH_METHOD_UNSUPPORTED);
    }
    #[test]
    fn http_authentication_and_permission_failures_are_distinct() {
        assert_eq!(http(reqwest::StatusCode::UNAUTHORIZED).unwrap_err(), AUTH);
        assert_eq!(http(reqwest::StatusCode::FORBIDDEN).unwrap_err(), super::super::PERMISSION);
    }
    #[test]
    fn catalog_metadata_and_disc_order_are_bounded() {
        let a=album(&serde_json::json!({"id":"al-123","name":"Album","artist":"Artist","coverArt":"al-123"})).unwrap();
        assert_eq!(a.cover_id.as_deref(), Some("al-123"));
        let opaque = album(&serde_json::json!({"id":"folder/β?id=1","name":"Album"})).unwrap();
        assert_eq!(opaque.id, "folder/β?id=1");
        let invalid_id = "bad\0id";
        assert!(album(&serde_json::json!({"id": invalid_id, "name": "Album"})).is_err());
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
