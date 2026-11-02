//! Small bounded HTTP surface: same-origin, literal-IP host, paired bearer API.
use crate::remote::{Backend, Control, Guard, Shared};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
const HTML: &str = include_str!("../remote-web/index.html");
const JS: &str = include_str!("../remote-web/app.js");
const CSS: &str = include_str!("../remote-web/style.css");
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}
struct Response {
    status: u16,
    mime: &'static str,
    body: Vec<u8>,
}
fn response(status: u16, value: serde_json::Value) -> Response {
    Response {
        status,
        mime: "application/json; charset=utf-8",
        body: serde_json::to_vec(&value).unwrap_or_default(),
    }
}
fn error(status: u16, code: &str) -> Response {
    response(status, serde_json::json!({"error":code}))
}
fn read_request(mut read: impl Read) -> Result<Request, ()> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut header = Vec::new();
    loop {
        if header.len() >= 8192 || Instant::now() > deadline {
            return Err(());
        }
        let mut b = [0];
        if read.read(&mut b).map_err(|_| ())? != 1 {
            return Err(());
        }
        header.push(b[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&header).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let first = lines.next().ok_or(())?.split(' ').collect::<Vec<_>>();
    if first.len() != 3
        || !matches!(first[0], "GET" | "POST")
        || first[2] != "HTTP/1.1"
        || !first[1].starts_with('/')
        || first[1].len() > 256
        || first[1].bytes().any(|b| b <= 32 || b >= 127)
        || first[1].contains(['#', '\\'])
    {
        return Err(());
    }
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(':').ok_or(())?;
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.chars().any(|c| c.is_control())
        {
            return Err(());
        }
        if headers
            .insert(key.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err(());
        }
    }
    if headers.contains_key("transfer-encoding") || !headers.contains_key("host") {
        return Err(());
    }
    let len = match headers.get("content-length") {
        Some(value) if value.bytes().all(|b| b.is_ascii_digit()) && !value.is_empty() => {
            value.parse::<usize>().map_err(|_| ())?
        }
        None => 0,
        _ => return Err(()),
    };
    if len > 4096
        || first[0] == "GET" && len != 0
        || first[0] == "POST"
            && headers
                .get("content-type")
                .map(|v| v.split(';').next().unwrap_or("").trim())
                != Some("application/json")
    {
        return Err(());
    }
    let mut body = vec![0; len];
    let mut got = 0;
    while got < len {
        if Instant::now() > deadline {
            return Err(());
        }
        let n = read.read(&mut body[got..]).map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        got += n;
    }
    Ok(Request {
        method: first[0].into(),
        path: first[1].into(),
        headers,
        body,
    })
}
fn lan(ip: IpAddr) -> bool {
    matches!(ip,IpAddr::V4(v) if v.is_loopback()||v.is_private()||v.is_link_local())
}
fn origin_allowed(req: &Request, local: SocketAddr) -> bool {
    req.headers
        .get("host")
        .is_some_and(|host| host == &local.to_string())
        && req
            .headers
            .get("origin")
            .is_none_or(|origin| origin == &format!("http://{local}"))
        && req
            .headers
            .get("sec-fetch-site")
            .is_none_or(|v| matches!(v.as_str(), "same-origin" | "none"))
}
fn token(req: &Request) -> Option<String> {
    let value = req.headers.get("authorization")?.strip_prefix("Bearer ")?;
    (value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then(|| value.to_owned())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    code: String,
    name: String,
}
fn route(
    req: Request,
    local: SocketAddr,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    backend: Arc<dyn Backend>,
) -> Response {
    if !origin_allowed(&req, local) {
        return error(403, "origin");
    }
    if req.method == "GET" {
        let asset = match req.path.as_str() {
            "/" => Some(("text/html; charset=utf-8", HTML)),
            "/app.js" => Some(("text/javascript; charset=utf-8", JS)),
            "/style.css" => Some(("text/css; charset=utf-8", CSS)),
            _ => None,
        };
        if let Some((mime, body)) = asset {
            return Response {
                status: 200,
                mime,
                body: body.as_bytes().to_vec(),
            };
        }
    }
    if !backend.active() || stop.load(Ordering::Acquire) {
        return error(503, "unavailable");
    }
    if req.method == "POST" && req.path == "/api/v1/pair" {
        let Ok(pair) = serde_json::from_slice::<Pair>(&req.body) else {
            return error(400, "invalid");
        };
        if pair.name.len() > 192 {
            return error(400, "invalid");
        }
        return match shared
            .pair(&pair.code, &pair.name)
            .map_err(|e| match e.as_str() {
                "invalid" => "invalid",
                "device_limit" => "device_limit",
                "pairing_failed" => "pairing_failed",
                "pairing_closed" => "pairing_closed",
                "busy" => "busy",
                _ => "unavailable",
            }) {
            Ok(token) => response(
                200,
                serde_json::json!({"schema":1,"token":token,"expires_in":crate::remote::CLIENT_LIFETIME}),
            ),
            Err(code) => error(403, code),
        };
    }
    let Some(token) = token(&req) else {
        return error(401, "unauthorized");
    };
    let auth = shared.clone();
    let active = stop.clone();
    let bearer = token.clone();
    let gate = backend.clone();
    let current: Guard = Arc::new(move || {
        !active.load(Ordering::Acquire)
            && gate.active()
            && auth.auth.lock().is_ok_and(|v| v.allowed(&bearer))
    });
    if !current() {
        return error(401, "unauthorized");
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/api/v1/state") => response(
            200,
            serde_json::to_value(shared.snapshot()).unwrap_or_default(),
        ),
        ("GET", path) if path.starts_with("/api/v1/queue?") => {
            let offset = path
                .strip_prefix("/api/v1/queue?offset=")
                .filter(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| *n < 100_000);
            let Some(offset) = offset else {
                return error(400, "invalid");
            };
            match backend.queue(offset) {
                Ok(page) if current() => {
                    response(200, serde_json::to_value(page).unwrap_or_default())
                }
                Ok(_) => error(401, "unauthorized"),
                Err(e) => error(409, &crate::remote::public_error(&e)),
            }
        }
        ("GET", path) if path.starts_with("/api/v1/artwork/") => {
            let key = path.trim_start_matches("/api/v1/artwork/");
            if key.len() != 64
                || !key.bytes().all(|b| b.is_ascii_hexdigit())
                || key != shared.snapshot().media
            {
                return error(404, "artwork_missing");
            }
            match backend.artwork(key, current.clone()) {
                Ok(Some((mime, bytes))) if current() && bytes.len() <= 2 * 1024 * 1024 => {
                    Response {
                        status: 200,
                        mime: match mime.as_str() {
                            "image/png" => "image/png",
                            "image/jpeg" => "image/jpeg",
                            "image/webp" => "image/webp",
                            _ => return error(404, "artwork_missing"),
                        },
                        body: bytes,
                    }
                }
                _ => error(404, "artwork_missing"),
            }
        }
        ("POST", "/api/v1/control") => {
            let Ok(control) = serde_json::from_slice::<Control>(&req.body) else {
                return error(400, "invalid");
            };
            if !control.valid() {
                return error(400, "invalid");
            }
            if !shared.snapshot().available {
                return error(409, "unavailable");
            }
            let claimed = match shared.auth.lock() {
                Ok(mut auth) => auth.claim(&token, &control),
                Err(_) => Err("busy".into()),
            };
            let result = match claimed {
                Ok(Some(result)) => result,
                Err(code) => return error(409, &code),
                Ok(None) => {
                    let result = if current() {
                        backend
                            .control(control.command, current.clone())
                            .map_err(|e| crate::remote::public_error(&e))
                    } else {
                        Err("unauthorized".into())
                    };
                    if let Ok(mut auth) = shared.auth.lock() {
                        auth.finish(&token, &control.request_id, result.clone());
                    }
                    result
                }
            };
            match result {
                Ok(()) => response(200, serde_json::json!({"accepted":true})),
                Err(code) => error(409, &code),
            }
        }
        _ => error(404, "not_found"),
    }
}
fn write_response(mut stream: impl Write, reply: Response) -> std::io::Result<()> {
    write!(stream,"HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Resource-Policy: same-origin\r\nPermissions-Policy: camera=(), microphone=(), geolocation=(), fullscreen=(self)\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' blob: data:; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'\r\n\r\n",reply.status,if reply.status==200{"OK"}else{"Error"},reply.mime,reply.body.len())?;
    stream.write_all(&reply.body)
}
fn connection(
    mut stream: TcpStream,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    backend: Arc<dyn Backend>,
) {
    let Ok(peer) = stream.peer_addr() else {
        return;
    };
    if !lan(peer.ip()) {
        return;
    }
    let Ok(local) = stream.local_addr() else {
        return;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let reply = match read_request(&mut stream) {
        Ok(request) if !stop.load(Ordering::Acquire) => {
            route(request, local, stop, shared, backend)
        }
        _ => error(400, "invalid"),
    };
    let _ = write_response(stream, reply);
}
pub(crate) fn serve(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    backend: Arc<dyn Backend>,
) {
    let mut tasks: Vec<thread::JoinHandle<()>> = Vec::new();
    let mut budgets: BTreeMap<IpAddr, (Instant, u32)> = BTreeMap::new();
    while !stop.load(Ordering::Acquire) {
        let mut index = 0;
        while index < tasks.len() {
            if tasks[index].is_finished() {
                let task = tasks.swap_remove(index);
                let _ = task.join();
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok((stream, peer)) => {
                if !lan(peer.ip()) || tasks.len() >= 4 {
                    continue;
                }
                let now = Instant::now();
                budgets.retain(|_, v| now.duration_since(v.0) < Duration::from_secs(60));
                if !budgets.contains_key(&peer.ip()) && budgets.len() >= 32 {
                    continue;
                }
                let budget = budgets.entry(peer.ip()).or_insert((now, 0));
                if budget.1 >= 180 {
                    continue;
                }
                budget.1 += 1;
                let stop = stop.clone();
                let shared = shared.clone();
                let backend = backend.clone();
                if let Ok(task) = thread::Builder::new()
                    .name("sonante-remote-request".into())
                    .spawn(move || connection(stream, stop, shared, backend))
                {
                    tasks.push(task);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25))
            }
            Err(_) => break,
        }
    }
    stop.store(true, Ordering::Release);
    for task in tasks {
        let _ = task.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::atomic::AtomicUsize};
    struct Fake {
        calls: AtomicUsize,
        revokes: Option<Arc<Shared>>,
        effects: AtomicUsize,
        enabled: AtomicBool,
        disable_on_control: bool,
    }
    impl Backend for Fake {
        fn active(&self) -> bool {
            self.enabled.load(Ordering::Acquire)
        }
        fn queue(&self, offset: usize) -> Result<crate::remote::QueuePage, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(crate::remote::QueuePage {
                revision: format!("q1:{}", "a".repeat(64)),
                total: 0,
                offset,
                tracks: Vec::new(),
            })
        }
        fn control(&self, _command: crate::remote::Command, current: Guard) -> Result<(), String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.disable_on_control {
                self.enabled.store(false, Ordering::Release);
            }
            if let Some(shared) = &self.revokes {
                *shared.auth.lock().unwrap() = crate::remote::Auth::default();
            }
            if !current() {
                return Err("REMOTE_CHANGED".into());
            }
            self.effects.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn artwork(
            &self,
            _key: &str,
            _current: Guard,
        ) -> Result<Option<(String, Vec<u8>)>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }
    fn scene() -> (Arc<Shared>, Arc<Fake>, String) {
        let shared = Arc::new(Shared::default());
        let token = shared.auth.lock().unwrap().test_token();
        *shared.snapshot.lock().unwrap() = Some(crate::remote::Stamped {
            snapshot: crate::remote::Snapshot {
                schema: 1,
                available: true,
                media: "a".repeat(64),
                ..Default::default()
            },
            at: Instant::now(),
        });
        (
            shared,
            Arc::new(Fake {
                calls: AtomicUsize::new(0),
                effects: AtomicUsize::new(0),
                enabled: AtomicBool::new(true),
                disable_on_control: false,
                revokes: None,
            }),
            token,
        )
    }
    fn request(method: &str, path: &str, token: &str, body: Option<serde_json::Value>) -> Request {
        let body = body
            .map(|v| serde_json::to_vec(&v).unwrap())
            .unwrap_or_default();
        let mut headers = BTreeMap::from([("host".into(), "192.168.1.20:8787".into())]);
        if !token.is_empty() {
            headers.insert("authorization".into(), format!("Bearer {token}"));
        }
        Request {
            method: method.into(),
            path: path.into(),
            headers,
            body,
        }
    }
    fn run(req: Request, shared: Arc<Shared>, backend: Arc<Fake>) -> Response {
        route(
            req,
            "192.168.1.20:8787".parse().unwrap(),
            Arc::new(AtomicBool::new(false)),
            shared,
            backend,
        )
    }
    #[test]
    fn remote_http_rejects_duplicate_lengths_chunking_extra_headers_and_oversized_bodies() {
        for bytes in ["GET / HTTP/1.1\r\nHost: x\r\nHost: y\r\n\r\n","POST / HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 4097\r\n\r\n","POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n","POST / HTTP/1.1\r\nHost: x\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{}","GET //outside/#secret HTTP/1.1\r\nHost: x\r\n\r\n","GET / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\n\r\nx"]{assert!(read_request(Cursor::new(bytes.as_bytes())).is_err());}
        assert!(read_request(Cursor::new(
            b"GET / HTTP/1.1\r\nHost: 192.168.1.20:8787\r\n\r\n"
        ))
        .is_ok());
    }
    #[test]
    fn remote_api_requires_bearer_even_on_lan_and_never_calls_backend_for_unauthorized_requests() {
        let (shared, backend, _) = scene();
        for path in [
            "/api/v1/state",
            "/api/v1/queue?offset=0",
            "/api/v1/artwork/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert_eq!(
                run(
                    request("GET", path, "", None),
                    shared.clone(),
                    backend.clone()
                )
                .status,
                401
            );
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            run(request("GET", "/", "", None), shared, backend).status,
            200
        );
    }
    #[test]
    fn remote_dns_rebinding_and_cross_origin_requests_are_refused_before_authentication_or_effects()
    {
        let (shared, backend, token) = scene();
        for (name, value) in [
            ("host", "attacker.invalid:8787"),
            ("host", "192.168.1.20:9999"),
            ("origin", "http://attacker.invalid"),
            ("origin", "null"),
            ("sec-fetch-site", "cross-site"),
        ] {
            let mut req = request("GET", "/api/v1/state", &token, None);
            req.headers.insert(name.into(), value.into());
            assert_eq!(run(req, shared.clone(), backend.clone()).status, 403);
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            run(
                request("GET", "/api/v1/state", &token, None),
                shared,
                backend
            )
            .status,
            200
        );
    }
    #[test]
    fn remote_http_mutation_replay_is_idempotent_and_revoked_requests_cannot_apply() {
        let (shared, backend, token) = scene();
        let body = serde_json::json!({"request_id":"fixture-command","command":{"action":"next","media":"a".repeat(64)}});
        for _ in 0..2 {
            assert_eq!(
                run(
                    request("POST", "/api/v1/control", &token, Some(body.clone())),
                    shared.clone(),
                    backend.clone()
                )
                .status,
                200
            );
        }
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
        *shared.auth.lock().unwrap() = crate::remote::Auth::default();
        assert_eq!(
            run(
                request("POST", "/api/v1/control", &token, Some(body)),
                shared,
                backend.clone()
            )
            .status,
            401
        );
        assert_eq!(backend.effects.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn remote_revocation_between_admission_and_effect_invalidates_the_backend_guard() {
        let (shared, _, token) = scene();
        let backend = Arc::new(Fake {
            calls: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            enabled: AtomicBool::new(true),
            disable_on_control: false,
            revokes: Some(shared.clone()),
        });
        let body = serde_json::json!({"request_id":"fixture-command","command":{"action":"next","media":"a".repeat(64)}});
        assert_eq!(
            run(
                request("POST", "/api/v1/control", &token, Some(body)),
                shared,
                backend.clone()
            )
            .status,
            409
        );
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn remote_invalid_commands_and_arbitrary_media_paths_never_reach_the_backend() {
        let (shared, backend, token) = scene();
        for command in [
            serde_json::json!({"action":"seek","seconds":-1,"media":"a".repeat(64)}),
            serde_json::json!({"action":"play","uri":"/private/file.flac"}),
            serde_json::json!({"action":"queue_play","index":0,"revision":"old"}),
        ] {
            let body = serde_json::json!({"request_id":"fixture-command","command":command});
            assert_eq!(
                run(
                    request("POST", "/api/v1/control", &token, Some(body)),
                    shared.clone(),
                    backend.clone()
                )
                .status,
                400
            );
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn remote_beta_disabled_blocks_pairing_state_queue_and_commands_even_before_worker_cleanup() {
        let (shared, backend, token) = scene();
        backend.enabled.store(false, Ordering::Release);
        for (method, path, body) in [
            ("GET", "/api/v1/state", None),
            ("GET", "/api/v1/queue?offset=0", None),
            (
                "POST",
                "/api/v1/pair",
                Some(serde_json::json!({"code":"123456","name":"Fixture"})),
            ),
            (
                "POST",
                "/api/v1/control",
                Some(
                    serde_json::json!({"request_id":"fixture-beta","command":{"action":"next","media":"a".repeat(64)}}),
                ),
            ),
        ] {
            assert_eq!(
                run(
                    request(method, path, &token, body),
                    shared.clone(),
                    backend.clone()
                )
                .status,
                503
            );
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn remote_disabling_beta_during_a_pending_command_invalidates_its_effect_guard() {
        let (shared, _, token) = scene();
        let backend = Arc::new(Fake {
            calls: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
            revokes: None,
            enabled: AtomicBool::new(true),
            disable_on_control: true,
        });
        let body = serde_json::json!({"request_id":"fixture-beta","command":{"action":"next","media":"a".repeat(64)}});
        assert_eq!(
            run(
                request("POST", "/api/v1/control", &token, Some(body)),
                shared,
                backend.clone()
            )
            .status,
            409
        );
        assert_eq!(backend.effects.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn remote_http_responses_are_no_store_same_origin_and_have_no_cookies_or_cors() {
        let mut bytes = Vec::new();
        write_response(&mut bytes, response(200, serde_json::json!({"schema":1}))).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Cache-Control: no-store"));
        assert!(text.contains("frame-ancestors 'none'"));
        assert!(text.contains("Referrer-Policy: no-referrer"));
        assert!(!text.contains("Access-Control-Allow-Origin"));
        assert!(!text.contains("Set-Cookie"));
        assert!(lan("192.168.1.1".parse().unwrap()));
        assert!(!lan("8.8.8.8".parse().unwrap()));
        assert!(!lan("::1".parse().unwrap()));
    }
}
