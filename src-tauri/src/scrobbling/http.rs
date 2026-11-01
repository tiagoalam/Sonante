//! Fixed HTTPS endpoints and whitelisted payloads. Never log service bodies/keys.
use super::model::{Provider, Track};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
const LASTFM: &str = "https://ws.audioscrobbler.com/2.0/";
const LISTENBRAINZ: &str = "https://api.listenbrainz.org/1/";
const MAX_REPLY: usize = 65_536;
#[derive(Clone, Copy, Debug)]
pub struct Failure {
    pub code: &'static str,
    pub retry: u64,
}
impl Failure {
    pub fn new(code: &'static str) -> Self {
        Self { code, retry: 60 }
    }
    pub fn terminal(self) -> bool {
        matches!(self.code, "auth" | "rejected")
    }
}
pub fn hex_key(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
// Tokens returned by the service are opaque, not API-key hex digests.
// Keep the documented size and printable ASCII boundary; encode at transport.
pub fn lastfm_credential(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_graphic())
}
pub fn lastfm_authorization_url(key: &str, token: &str) -> Result<String, Failure> {
    if !hex_key(key) || !lastfm_credential(token) {
        return Err(Failure::new("invalid"));
    }
    let mut url = reqwest::Url::parse("https://www.last.fm/api/auth/")
        .map_err(|_| Failure::new("invalid"))?;
    url.query_pairs_mut()
        .append_pair("api_key", key)
        .append_pair("token", token);
    Ok(url.into())
}
pub fn user(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
}
pub fn lb_token(s: &str) -> bool {
    (8..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
pub fn signature(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut input = String::new();
    for (key, value) in params {
        if key != "format" && key != "api_sig" {
            input.push_str(key);
            input.push_str(value);
        }
    }
    input.push_str(secret);
    format!("{:x}", md5::compute(input.as_bytes()))
}
pub fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Lastfm => "lastfm",
        Provider::Listenbrainz => "listenbrainz",
    }
}
fn operation_label(method: &str) -> &'static str {
    match method {
        "auth.getToken" => "authorize_token",
        "auth.getSession" => "authorize_session",
        "track.updateNowPlaying" => "now_playing",
        "track.scrobble" => "submit_listen",
        _ => "unknown",
    }
}
fn credential_shape(value: Option<&Value>) -> &'static str {
    match value {
        None => "missing",
        Some(Value::String(s)) if lastfm_credential(s) => "valid",
        Some(Value::String(s)) if s.len() != 32 => "wrong_length",
        Some(Value::String(_)) => "unsafe_characters",
        Some(_) => "wrong_type",
    }
}
// Describe structure using fixed labels only. Never serialize service data.
fn reply_shape(v: &Value) -> String {
    let kind = match v {
        Value::Object(_) => "object",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Null => "null",
        _ => "scalar",
    };
    format!(
        "json_kind={} token_shape={} session_key_shape={}",
        kind,
        credential_shape(v.get("token")),
        credential_shape(v.pointer("/session/key"))
    )
}
fn body_kind(body: &[u8]) -> &'static str {
    let bytes = body
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(16)
        .collect::<Vec<_>>();
    if bytes.is_empty() {
        "empty"
    } else if bytes.starts_with(b"<?xml") || bytes.starts_with(b"<lfm") {
        "xml"
    } else if bytes.starts_with(b"<") {
        "markup"
    } else {
        "other"
    }
}
fn count(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}
pub fn lastfm_reply(v: Value, method: &str) -> Result<Value, Failure> {
    if let Some(code) = v.get("error").and_then(count) {
        return Err(Failure::new(match code {
            4 | 9 | 10 | 13 | 26 => "auth",
            14 => "not_authorized",
            15 => "expired",
            6 => "rejected",
            29 => "rate_limited",
            _ => "temporary",
        }));
    }
    match method {
        "auth.getToken"
            if v.get("token")
                .and_then(Value::as_str)
                .is_some_and(lastfm_credential) =>
        {
            Ok(v)
        }
        "auth.getSession"
            if v.pointer("/session/key")
                .and_then(Value::as_str)
                .is_some_and(lastfm_credential)
                && v.pointer("/session/name")
                    .and_then(Value::as_str)
                    .is_some_and(user) =>
        {
            Ok(v)
        }
        "track.updateNowPlaying" if v.get("nowplaying").is_some_and(Value::is_object) => Ok(v),
        "track.scrobble" => match v.pointer("/scrobbles/@attr/accepted").and_then(count) {
            Some(1) => Ok(v),
            Some(0) if v.pointer("/scrobbles/@attr/ignored").and_then(count) == Some(1) => {
                Err(Failure::new("rejected"))
            }
            _ => Err(Failure::new("protocol")),
        },
        _ => Err(Failure::new("protocol")),
    }
}
pub fn lb_payload(track: &Track, timestamp: Option<u64>) -> Value {
    let mut metadata = json!({"artist_name":track.artist,"track_name":track.title,
        "additional_info":{"duration":track.duration,"submission_client":"Sonante","submission_client_version":env!("CARGO_PKG_VERSION")}});
    if !track.album.is_empty() {
        metadata["release_name"] = json!(track.album);
    }
    let mut listen = json!({"track_metadata":metadata});
    if let Some(ts) = timestamp {
        listen["listened_at"] = json!(ts);
    }
    json!({"listen_type":if timestamp.is_some(){"single"}else{"playing_now"},"payload":[listen]})
}
fn decode_reply(provider: Provider, status: u16, body: &[u8]) -> Result<Value, Failure> {
    let failure = match status {
        401 | 403 => Some("auth"),
        500..=599 => Some("temporary"),
        200..=299 => None,
        _ => Some("rejected"),
    };
    if body.len() > MAX_REPLY {
        return Err(Failure::new(failure.unwrap_or("protocol")));
    }
    let parsed: Value =
        serde_json::from_slice(body).map_err(|_| Failure::new(failure.unwrap_or("protocol")))?;
    // Last.fm's numeric API error is meaningful even on a non-200 response.
    if provider == Provider::Lastfm && parsed.get("error").and_then(count).is_some() {
        return Ok(parsed);
    }
    if let Some(code) = failure {
        return Err(Failure::new(code));
    }
    Ok(parsed)
}
pub struct Api {
    client: reqwest::Client,
    next: [Instant; 2],
}
impl Api {
    pub fn new() -> Result<Self, Failure> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "Sonante/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/tiagoalam/Sonante)"
            ))
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Failure::new("network"))?;
        Ok(Self {
            client,
            next: [Instant::now(); 2],
        })
    }
    async fn request(
        &mut self,
        provider: Provider,
        request: reqwest::RequestBuilder,
    ) -> Result<Value, Failure> {
        let i = provider.index();
        let wait = self.next[i].saturating_duration_since(Instant::now());
        if wait > Duration::from_secs(1) {
            return Err(Failure {
                code: "rate_limited",
                retry: wait.as_secs().saturating_add(1),
            });
        }
        tokio::time::sleep_until(self.next[i].into()).await;
        self.next[i] = Instant::now() + Duration::from_secs(1);
        let mut reply = request.send().await.map_err(|_| Failure::new("network"))?;
        let status = reply.status();
        let retry = reply
            .headers()
            .get("Retry-After")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.parse::<u64>().ok())
            .unwrap_or(60)
            .clamp(1, 86_400);
        if provider == Provider::Listenbrainz
            && reply
                .headers()
                .get("X-RateLimit-Remaining")
                .and_then(|h| h.to_str().ok())
                == Some("0")
        {
            let reset = reply
                .headers()
                .get("X-RateLimit-Reset-In")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.parse::<u64>().ok())
                .unwrap_or(60)
                .clamp(1, 86_400);
            self.next[i] = Instant::now() + Duration::from_secs(reset);
        }
        if status.as_u16() == 429 {
            self.next[i] = Instant::now() + Duration::from_secs(retry);
            return Err(Failure {
                code: "rate_limited",
                retry,
            });
        }
        if reply.content_length().is_some_and(|n| n > MAX_REPLY as u64) {
            return Err(Failure::new("protocol"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = reply.chunk().await.map_err(|_| Failure::new("network"))? {
            if body.len() + chunk.len() > MAX_REPLY {
                return Err(Failure::new("protocol"));
            }
            body.extend_from_slice(&chunk);
        }
        let result = decode_reply(provider, status.as_u16(), &body);
        if result.as_ref().is_err_and(|e| e.code == "protocol") {
            eprintln!("[Scrobbling] provider={} phase=http_reply outcome=protocol http_status={} body_kind={}", provider_label(provider), status.as_u16(), body_kind(&body));
        }
        result
    }
    pub async fn lastfm(
        &mut self,
        key: &str,
        secret: &str,
        session: Option<&str>,
        method: &str,
        values: Vec<(&str, String)>,
    ) -> Result<Value, Failure> {
        let mut params: BTreeMap<String, String> = values
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        params.insert("api_key".into(), key.into());
        params.insert("method".into(), method.into());
        if let Some(sk) = session {
            params.insert("sk".into(), sk.into());
        }
        let sig = signature(&params, secret);
        params.insert("api_sig".into(), sig);
        params.insert("format".into(), "json".into());
        let request = self.client.post(LASTFM).form(&params);
        let response = self.request(Provider::Lastfm, request).await;
        let result = match response {
            Ok(value) => {
                let shape = reply_shape(&value);
                let result = lastfm_reply(value, method);
                if result.as_ref().is_err_and(|e| e.code == "protocol") {
                    eprintln!(
                        "[Scrobbling] provider=lastfm operation={} outcome=protocol {}",
                        operation_label(method),
                        shape
                    );
                }
                result
            }
            Err(error) => {
                eprintln!(
                    "[Scrobbling] provider=lastfm operation={} outcome={}",
                    operation_label(method),
                    error.code
                );
                Err(error)
            }
        };
        if let Err(e) = &result {
            if e.code == "rate_limited" {
                self.next[0] = Instant::now() + Duration::from_secs(e.retry);
            }
        }
        result
    }
    pub async fn validate_lb(&mut self, token: &str) -> Result<String, Failure> {
        let mut header = reqwest::header::HeaderValue::from_str(&format!("Token {token}"))
            .map_err(|_| Failure::new("invalid"))?;
        header.set_sensitive(true);
        let request = self
            .client
            .get(format!("{LISTENBRAINZ}validate-token"))
            .header(reqwest::header::AUTHORIZATION, header);
        let v = self.request(Provider::Listenbrainz, request).await?;
        if v.get("valid").and_then(Value::as_bool) != Some(true) {
            return Err(Failure::new("auth"));
        }
        v.get("user_name")
            .and_then(Value::as_str)
            .filter(|s| user(s))
            .map(str::to_string)
            .ok_or(Failure::new("protocol"))
    }
    pub async fn submit_lb(
        &mut self,
        token: &str,
        track: &Track,
        timestamp: Option<u64>,
    ) -> Result<(), Failure> {
        let mut header = reqwest::header::HeaderValue::from_str(&format!("Token {token}"))
            .map_err(|_| Failure::new("invalid"))?;
        header.set_sensitive(true);
        let request = self
            .client
            .post(format!("{LISTENBRAINZ}submit-listens"))
            .header(reqwest::header::AUTHORIZATION, header)
            .json(&lb_payload(track, timestamp));
        let v = self.request(Provider::Listenbrainz, request).await?;
        if v.get("status").and_then(Value::as_str) == Some("ok") {
            Ok(())
        } else {
            Err(Failure::new("protocol"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lastfm_non_hex_tokens_and_sessions_are_accepted_without_weakening_api_key_checks() {
        let token = "G".repeat(32);
        assert!(!hex_key(&token));
        assert!(lastfm_reply(json!({"token":token}), "auth.getToken").is_ok());
        assert!(lastfm_reply(
            json!({"session":{"key":"Z".repeat(32),"name":"fixture"}}),
            "auth.getSession"
        )
        .is_ok());
        for token in [
            "short".to_string(),
            "a".repeat(33),
            "a".repeat(31) + "\n",
            "a".repeat(31) + " ",
            "a".repeat(30) + "é",
        ] {
            assert!(lastfm_reply(json!({"token":token}), "auth.getToken").is_err());
        }
    }
    #[test]
    fn lastfm_opaque_credentials_round_trip_through_authorization_url_and_signed_form() {
        let key = "a".repeat(32);
        let token = "g+/&?#=%".repeat(4);
        let url = lastfm_authorization_url(&key, &token).unwrap();
        let url = reqwest::Url::parse(&url).unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://www.last.fm");
        assert_eq!(url.path(), "/api/auth/");
        assert!(url.fragment().is_none());
        let pairs = url.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs["api_key"], key);
        assert_eq!(pairs["token"], token);
        assert!(lastfm_authorization_url(&"G".repeat(32), &token).is_err());
        let mut params = BTreeMap::from([
            ("api_key".to_string(), key),
            ("method".to_string(), "auth.getSession".to_string()),
            ("token".to_string(), token.clone()),
        ]);
        let expected = format!(
            "{:x}",
            md5::compute(
                format!(
                    "api_key{}methodauth.getSessiontoken{}secret",
                    "a".repeat(32),
                    token
                )
                .as_bytes()
            )
        );
        assert_eq!(signature(&params, "secret"), expected);
        params.insert("api_sig".into(), expected);
        let request = reqwest::Client::new()
            .post(LASTFM)
            .form(&params)
            .build()
            .unwrap();
        let body = request.body().unwrap().as_bytes().unwrap();
        let form_url = reqwest::Url::parse(&format!(
            "https://fixture.invalid/?{}",
            std::str::from_utf8(body).unwrap()
        ))
        .unwrap();
        assert_eq!(
            form_url
                .query_pairs()
                .find(|(key, _)| key == "token")
                .unwrap()
                .1,
            token
        );
    }
    #[test]
    fn reply_diagnostics_never_include_credentials_bodies_or_untrusted_method_names() {
        let value = json!({"token":"SECRET_TOKEN", "session":{"key":"PRIVATE_KEY", "name":"PRIVATE_USER"}, "https://private.invalid/":"PRIVATE_BODY"});
        let summary = reply_shape(&value);
        assert_eq!(
            summary,
            "json_kind=object token_shape=wrong_length session_key_shape=wrong_length"
        );
        for secret in [
            "SECRET_TOKEN",
            "PRIVATE_KEY",
            "PRIVATE_USER",
            "private.invalid",
            "PRIVATE_BODY",
        ] {
            assert!(!summary.contains(secret));
        }
        assert_eq!(operation_label("SECRET_METHOD"), "unknown");
        assert_eq!(body_kind(b"  <?xml PRIVATE_BODY"), "xml");
        assert_eq!(body_kind(b"<html>PRIVATE_BODY"), "markup");
        assert_eq!(body_kind(b"PRIVATE_BODY"), "other");
        assert_eq!(body_kind(b"   "), "empty");
        assert_eq!(
            reply_shape(&json!({"token":"a".repeat(32)})),
            "json_kind=object token_shape=valid session_key_shape=missing"
        );
    }
    #[test]
    fn lastfm_signature_is_sorted_utf8_and_excludes_format_and_previous_signature() {
        let mut p = BTreeMap::from([("b".into(), "2".into()), ("a".into(), "1".into())]);
        assert_eq!(
            signature(&p, "secret"),
            format!("{:x}", md5::compute(b"a1b2secret"))
        );
        p.insert("format".into(), "json".into());
        p.insert("api_sig".into(), "ignored".into());
        assert_eq!(
            signature(&p, "secret"),
            format!("{:x}", md5::compute(b"a1b2secret"))
        );
        p.insert("artist".into(), "音楽".into());
        assert_ne!(signature(&p, "secret"), signature(&p, "different"));
    }
    #[test]
    fn http_200_lastfm_errors_and_ignored_listens_are_not_success() {
        assert_eq!(
            lastfm_reply(json!({"error":9,"message":"private"}), "track.scrobble")
                .err()
                .unwrap()
                .code,
            "auth"
        );
        assert_eq!(
            lastfm_reply(
                json!({"scrobbles":{"@attr":{"accepted":"0","ignored":"1"}}}),
                "track.scrobble"
            )
            .err()
            .unwrap()
            .code,
            "rejected"
        );
        assert!(lastfm_reply(
            json!({"scrobbles":{"@attr":{"accepted":"1","ignored":"0"}}}),
            "track.scrobble"
        )
        .is_ok());
        assert!(lastfm_reply(json!({"status":"ok"}), "track.scrobble").is_err());
        assert!(lastfm_reply(json!({"token":"not a token"}), "auth.getToken").is_err());
    }
    #[test]
    fn non_200_lastfm_authorization_error_retains_its_actionable_code() {
        let body = serde_json::to_vec(&json!({"error":14,"message":"Not authorized yet"})).unwrap();
        let reply = decode_reply(Provider::Lastfm, 403, &body).unwrap();
        assert_eq!(
            lastfm_reply(reply, "auth.getSession").err().unwrap().code,
            "not_authorized"
        );
        assert_eq!(
            decode_reply(Provider::Listenbrainz, 401, b"{}")
                .err()
                .unwrap()
                .code,
            "auth"
        );
        assert_eq!(
            decode_reply(Provider::Lastfm, 502, b"<html>unavailable</html>")
                .err()
                .unwrap()
                .code,
            "temporary"
        );
        assert!(decode_reply(Provider::Lastfm, 200, &vec![b' '; MAX_REPLY + 1]).is_err());
    }
    #[test]
    fn listenbrainz_payload_has_only_whitelisted_metadata_and_original_timestamp() {
        let track = Track::new("Artist", "Song", "Album", 120.).unwrap();
        let playing = lb_payload(&track, None);
        assert_eq!(playing["listen_type"], "playing_now");
        assert!(playing["payload"][0].get("listened_at").is_none());
        let listen = lb_payload(&track, Some(123456));
        assert_eq!(listen["payload"][0]["listened_at"], 123456);
        let metadata = &listen["payload"][0]["track_metadata"];
        assert_eq!(metadata["artist_name"], "Artist");
        assert_eq!(metadata["additional_info"]["duration"], 120);
        for key in ["uri", "path", "token", "media_locator", "plex_image"] {
            assert!(metadata.get(key).is_none());
        }
    }
}
