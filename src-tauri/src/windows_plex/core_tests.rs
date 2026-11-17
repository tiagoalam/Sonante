use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};

const MIXED_ORDER_REMOTE_FIRST: &str =
    include_str!("../../tests/fixtures/plex_resources_remote_first.json");
const MIXED_ORDER_LOCAL_FIRST: &str =
    include_str!("../../tests/fixtures/plex_resources_local_first.json");

fn parse_fixture(payload: &str) -> Vec<DiscoveredPlexServer> {
    let resources: Vec<RawResource> = serde_json::from_str(payload).unwrap();
    parse_discovered_servers(resources)
}

fn spawn_http_response(
    status_line: &'static str,
    content_type: &'static str,
    body: &'static str,
    delay: Duration,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4096];
        let _ = stream.read(&mut request);
        thread::sleep(delay);
        let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        let _ = stream.write_all(response.as_bytes());
    });
    (format!("http://{address}"), handle)
}

fn test_request(
    status_line: &'static str,
    content_type: &'static str,
    body: &'static str,
    delay: Duration,
    timeout: Duration,
) -> Result<serde_json::Value, PlexError> {
    let (base_url, server) = spawn_http_response(status_line, content_type, body, delay);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .unwrap();
    let url = format!("{base_url}/fixture?X-Plex-Token=TEST_SECRET_TOKEN");
    let result = block_on(send_json(client.get(url), "consultar o fixture"));
    server.join().unwrap();
    result
}

struct MockResponse {
    status_line: &'static str,
    content_type: &'static str,
    body: String,
    delay: Duration,
}

impl MockResponse {
    fn json(status_line: &'static str, body: serde_json::Value) -> Self {
        Self {
            status_line,
            content_type: "application/json",
            body: body.to_string(),
            delay: Duration::ZERO,
        }
    }
}

struct MockServer {
    base_url: String,
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&str) -> MockResponse + Send + Sync + 'static,
    {
        Self::start_request(move |path, _request| handler(path))
    }
    fn start_request<F>(handler: F) -> Self
    where
        F: Fn(&str, &str) -> MockResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handler = Arc::new(handler);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 8192];
                        let size = stream.read(&mut request).unwrap_or(0);
                        let request = String::from_utf8_lossy(&request[..size]);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/");
                        let response = handler(path, &request);
                        thread::sleep(response.delay);
                        let payload = format!(
                                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                response.status_line,
                                response.content_type,
                                response.body.len(),
                                response.body
                            );
                        let _ = stream.write_all(payload.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            address,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn pms_server(
    machine_identifier: &'static str,
    content_status: &'static str,
    content_requests: Arc<AtomicUsize>,
) -> MockServer {
    MockServer::start(move |path| {
        if path.starts_with("/identity") {
            MockResponse::json(
                "200 OK",
                serde_json::json!({
                    "MediaContainer": { "machineIdentifier": machine_identifier }
                }),
            )
        } else if path == "/library/sections" {
            MockResponse::json("200 OK", serde_json::json!({"MediaContainer": {}}))
        } else {
            content_requests.fetch_add(1, Ordering::SeqCst);
            MockResponse::json(
                content_status,
                serde_json::json!({"MediaContainer": {"Metadata": []}}),
            )
        }
    })
}

// Retain each non-responding fixture listener until this test thread exits.
// Dropping a port reservation before the request lets a parallel mock server
// reuse it, accidentally turning a failed-route test into a healthy route.
thread_local! { static UNRESPONSIVE: std::cell::RefCell<Vec<TcpListener>> = const { std::cell::RefCell::new(Vec::new()) }; }
fn unreachable_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    UNRESPONSIVE.with(|fixtures|fixtures.borrow_mut().push(listener));
    format!("http://{address}")
}

fn connection_json(base_url: &str, local: bool, relay: bool) -> serde_json::Value {
    let address = base_url.trim_start_matches("http://");
    serde_json::json!({
        "protocol": "http",
        "address": address,
        "port": 80,
        "uri": base_url,
        "local": local,
        "relay": relay
    })
}

fn resource_json(
    machine_identifier: &str,
    name: &str,
    connections: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "clientIdentifier": machine_identifier,
        "provides": "server",
        "accessToken": "TEST_RESOURCE_TOKEN",
        "connections": connections
    })
}

fn resources_server(resources: Vec<serde_json::Value>) -> MockServer {
    let payload = serde_json::Value::Array(resources);
    MockServer::start(move |_| MockResponse::json("200 OK", payload.clone()))
}

fn manager_config(base_url: &str, machine_identifier: Option<&str>) -> AppConfig {
    AppConfig {
        enabled: true,
        plex_url: base_url.to_string(),
        plex_token: "TEST_ACCOUNT_TOKEN".to_string(),
        plex_server_id: machine_identifier.map(str::to_string),
        plex_server_name: machine_identifier.map(|_| "Selected Server".to_string()),
        ..AppConfig::default()
    }
}

fn test_manager(config: &AppConfig, resources_url: String) -> PlexConnectionManager {
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    let probe_http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(80))
        .build()
        .unwrap();
    PlexConnectionManager::with_clients(config, http, probe_http, resources_url)
}

#[test]
fn resources_capture_identity_connections_and_resource_token_internally() {
    let servers = parse_fixture(MIXED_ORDER_REMOTE_FIRST);
    assert_eq!(servers.len(), 1);

    let server = &servers[0];
    assert_eq!(server.identity.machine_identifier, "fixture-machine-id");
    assert_eq!(server.identity.display_name, "Fixture Music Server");
    assert_eq!(
        server.resource_access_token.as_deref(),
        Some("TEST_RESOURCE_ACCESS_TOKEN")
    );
    assert_eq!(server.connections.len(), 3);
    assert_eq!(
        server.connections[0].base_url,
        "https://remote.example.invalid:443"
    );
    assert_eq!(server.connections[0].protocol, "https");
    assert!(!server.connections[0].local);
    assert!(!server.connections[0].relay);
    assert_eq!(
        server.connections[1].base_url,
        "https://relay.example.invalid:443"
    );
    assert_eq!(server.connections[1].protocol, "https");
    assert!(!server.connections[1].local);
    assert!(server.connections[1].relay);
    assert_eq!(
        server.connections[2].base_url,
        "https://lan.example.invalid:32400"
    );
    assert_eq!(server.connections[2].protocol, "https");
    assert!(server.connections[2].local);
    assert!(!server.connections[2].relay);
}

#[test]
fn public_dto_preserves_existing_local_first_selection_for_variable_api_order() {
    for fixture in [MIXED_ORDER_REMOTE_FIRST, MIXED_ORDER_LOCAL_FIRST] {
        let server = parse_fixture(fixture).pop().unwrap();
        let public = into_public_server_resource(server);

        assert_eq!(public.client_identifier, "fixture-machine-id");
        assert_eq!(public.chosen_uri, "https://lan.example.invalid:32400");
        assert!(public.connections[0].local);
    }
}

#[test]
fn public_server_dto_does_not_expose_internal_credentials_or_connection_details() {
    let server = parse_fixture(MIXED_ORDER_REMOTE_FIRST).pop().unwrap();
    let public = serde_json::to_value(into_public_server_resource(server)).unwrap();
    let public_server = public.as_object().unwrap();
    let public_connection = public_server["connections"][0].as_object().unwrap();

    assert!(!public_server.contains_key("resource_access_token"));
    assert!(!public_server.contains_key("accessToken"));
    assert!(!public_connection.contains_key("relay"));
    assert!(!public_connection.contains_key("protocol"));
}

#[test]
fn http_200_with_valid_json_is_parsed() {
    let json = test_request(
        "200 OK",
        "application/json",
        r#"{"MediaContainer":{"size":0}}"#,
        Duration::ZERO,
        Duration::from_secs(1),
    )
    .unwrap();

    assert_eq!(json["MediaContainer"]["size"], 0);
}

#[test]
fn http_401_is_explicit_and_public_error_is_sanitized() {
    let error = test_request(
        "401 Unauthorized",
        "application/json",
        r#"{"error":"TEST_SECRET_TOKEN"}"#,
        Duration::ZERO,
        Duration::from_secs(1),
    )
    .unwrap_err();

    assert_eq!(error.kind, PlexErrorKind::Unauthorized);
    let public = error.public_message();
    assert!(public.contains("401"));
    assert!(!public.contains("TEST_SECRET_TOKEN"));
    assert!(!public.contains("X-Plex-Token"));
    assert!(!public.contains("http://"));
    assert!(!public.contains('?'));
}

#[test]
fn http_404_is_explicit() {
    let error = test_request(
        "404 Not Found",
        "application/json",
        "{}",
        Duration::ZERO,
        Duration::from_secs(1),
    )
    .unwrap_err();

    assert_eq!(error.kind, PlexErrorKind::NotFound);
}

#[test]
fn http_500_is_explicit() {
    let error = test_request(
        "500 Internal Server Error",
        "application/json",
        "{}",
        Duration::ZERO,
        Duration::from_secs(1),
    )
    .unwrap_err();

    assert_eq!(error.kind, PlexErrorKind::ServerError(500));
}

#[test]
fn successful_non_json_response_is_invalid_protocol() {
    let error = test_request(
        "200 OK",
        "text/plain",
        "not json",
        Duration::ZERO,
        Duration::from_secs(1),
    )
    .unwrap_err();

    assert_eq!(error.kind, PlexErrorKind::InvalidResponse);
}

#[test]
fn slow_response_is_classified_as_timeout() {
    let error = test_request(
        "200 OK",
        "application/json",
        "{}",
        Duration::from_millis(150),
        Duration::from_millis(25),
    )
    .unwrap_err();

    assert_eq!(error.kind, PlexErrorKind::Timeout);
}

#[test]
fn media_container_is_required_for_plex_content_responses() {
    let error =
        media_container(&serde_json::json!({"unexpected": true}), "testar protocolo").unwrap_err();

    assert_eq!(error.kind, PlexErrorKind::InvalidResponse);
}

#[test]
fn resolver_uses_working_lan_route() {
    let requests = Arc::new(AtomicUsize::new(0));
    let lan = pms_server("selected-server", "200 OK", requests);
    let config = manager_config(&lan.base_url, Some("selected-server"));
    let manager = test_manager(&config, unreachable_base_url());

    let resolved = block_on(manager.resolve()).unwrap();

    assert_eq!(resolved.base_url, lan.base_url);
    assert_eq!(resolved.token, "TEST_ACCOUNT_TOKEN");
}

#[test]
fn working_legacy_route_learns_identity_without_guessing_a_server() {
    let requests = Arc::new(AtomicUsize::new(0));
    let legacy = pms_server("legacy-server", "200 OK", requests);
    let config = manager_config(&legacy.base_url, None);
    let manager = test_manager(&config, unreachable_base_url());

    let resolved = block_on(manager.resolve()).unwrap();
    let snapshot = manager.snapshot().unwrap();

    assert_eq!(resolved.base_url, legacy.base_url);
    assert_eq!(
        snapshot.identity.unwrap().machine_identifier,
        "legacy-server"
    );
}

#[test]
fn previously_working_lan_is_refreshed_after_transport_failure() {
    let lan_requests = Arc::new(AtomicUsize::new(0));
    let lan = pms_server("selected-server", "200 OK", lan_requests);
    let remote_requests = Arc::new(AtomicUsize::new(0));
    let remote = pms_server("selected-server", "200 OK", remote_requests);
    let resources = resources_server(vec![resource_json(
        "selected-server",
        "Selected Server",
        vec![
            connection_json(&lan.base_url, true, false),
            connection_json(&remote.base_url, false, false),
        ],
    )]);
    let config = manager_config(&lan.base_url, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    let initial = block_on(manager.resolve()).unwrap();
    assert_eq!(initial.base_url, lan.base_url);
    drop(lan);

    let (_, refreshed) =
        block_on(manager.request_json::<serde_json::Value>("/library/test", "consultar conteúdo"))
            .unwrap();

    assert_eq!(refreshed.base_url, remote.base_url);
    assert_eq!(refreshed.token, "TEST_RESOURCE_TOKEN");
}

#[test]
fn inaccessible_lan_falls_back_to_matching_remote_direct_route() {
    let lan = unreachable_base_url();
    let requests = Arc::new(AtomicUsize::new(0));
    let remote = pms_server("selected-server", "200 OK", requests);
    let resources = resources_server(vec![resource_json(
        "selected-server",
        "Selected Server",
        vec![
            connection_json(&lan, true, false),
            connection_json(&remote.base_url, false, false),
        ],
    )]);
    let config = manager_config(&lan, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    let resolved = block_on(manager.resolve()).unwrap();

    assert_eq!(resolved.base_url, remote.base_url);
    assert_eq!(resolved.token, "TEST_RESOURCE_TOKEN");
}

#[test]
fn resolver_uses_relay_after_lan_and_remote_direct_fail() {
    let lan = unreachable_base_url();
    let remote = unreachable_base_url();
    let requests = Arc::new(AtomicUsize::new(0));
    let relay = pms_server("selected-server", "200 OK", requests);
    let resources = resources_server(vec![resource_json(
        "selected-server",
        "Selected Server",
        vec![
            connection_json(&relay.base_url, false, true),
            connection_json(&remote, false, false),
            connection_json(&lan, true, false),
        ],
    )]);
    let config = manager_config(&lan, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    let resolved = block_on(manager.resolve()).unwrap();

    assert_eq!(resolved.base_url, relay.base_url);
}

#[test]
fn candidate_with_different_identity_is_rejected() {
    let legacy = unreachable_base_url();
    let wrong_requests = Arc::new(AtomicUsize::new(0));
    let wrong = pms_server("different-server", "200 OK", wrong_requests);
    let right_requests = Arc::new(AtomicUsize::new(0));
    let right = pms_server("selected-server", "200 OK", right_requests);
    let resources = resources_server(vec![resource_json(
        "selected-server",
        "Selected Server",
        vec![
            connection_json(&wrong.base_url, true, false),
            connection_json(&right.base_url, false, false),
        ],
    )]);
    let config = manager_config(&legacy, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    let resolved = block_on(manager.resolve()).unwrap();

    assert_eq!(resolved.base_url, right.base_url);
}

#[test]
fn all_unreachable_routes_return_explicit_sanitized_error() {
    let lan = unreachable_base_url();
    let remote = unreachable_base_url();
    let resources = resources_server(vec![resource_json(
        "selected-server",
        "Selected Server",
        vec![
            connection_json(&lan, true, false),
            connection_json(&remote, false, false),
        ],
    )]);
    let config = manager_config(&lan, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    let error = block_on(manager.resolve()).err().unwrap();
    let public = error.public_message();

    assert!(error.is_route_unavailable());
    assert!(!public.contains("TEST_ACCOUNT_TOKEN"));
    assert!(!public.contains("X-Plex-Token"));
    assert!(!public.contains("http://"));
}

#[test]
fn unauthorized_established_route_does_not_trigger_discovery() {
    let discovery_requests = Arc::new(AtomicUsize::new(0));
    let discovery_counter = discovery_requests.clone();
    let resources = MockServer::start(move |_| {
        discovery_counter.fetch_add(1, Ordering::SeqCst);
        MockResponse::json("200 OK", serde_json::json!([]))
    });
    let pms = MockServer::start(|path| {
        if path == "/library/sections" {
            MockResponse::json("200 OK", serde_json::json!({"MediaContainer": {}}))
        } else if path.starts_with("/identity") {
            MockResponse::json(
                "200 OK",
                serde_json::json!({
                    "MediaContainer": { "machineIdentifier": "selected-server" }
                }),
            )
        } else {
            MockResponse::json("401 Unauthorized", serde_json::json!({}))
        }
    });
    let config = manager_config(&pms.base_url, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    block_on(manager.resolve()).unwrap();
    let error = block_on(manager.request_json::<serde_json::Value>("/library/test", "fixture content")).err().unwrap();

    assert_eq!(error.kind, PlexErrorKind::Unauthorized);
    assert_eq!(discovery_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn content_404_does_not_invalidate_resolved_connection() {
    let content_requests = Arc::new(AtomicUsize::new(0));
    let pms = pms_server("selected-server", "404 Not Found", content_requests.clone());
    let config = manager_config(&pms.base_url, Some("selected-server"));
    let manager = test_manager(&config, unreachable_base_url());

    let error =
        block_on(manager.request_json::<serde_json::Value>("/missing", "consultar conteúdo"))
            .err()
            .unwrap();
    let snapshot = manager.snapshot().unwrap();

    assert_eq!(error.kind, PlexErrorKind::NotFound);
    assert_eq!(
        snapshot.current_base_url.as_deref(),
        Some(pms.base_url.as_str())
    );
    assert_eq!(content_requests.load(Ordering::SeqCst), 1);
}

#[test]
fn stale_generation_cannot_publish_connection() {
    let identity_started = Arc::new(AtomicBool::new(false));
    let server_started = identity_started.clone();
    let server = MockServer::start(move |path| {
        if path.starts_with("/identity") {
            server_started.store(true, Ordering::SeqCst);
            MockResponse {
                status_line: "200 OK",
                content_type: "application/json",
                body: serde_json::json!({
                    "MediaContainer": { "machineIdentifier": "server-a" }
                })
                .to_string(),
                delay: Duration::from_millis(80),
            }
        } else {
            MockResponse::json("200 OK", serde_json::json!({"MediaContainer": {}}))
        }
    });
    let initial = manager_config(&server.base_url, Some("server-a"));
    let manager = Arc::new(test_manager(&initial, unreachable_base_url()));
    let worker_manager = manager.clone();
    let worker = thread::spawn(move || block_on(worker_manager.resolve()));

    for _ in 0..100 {
        if identity_started.load(Ordering::SeqCst) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(identity_started.load(Ordering::SeqCst));
    let updated = manager_config("http://new.invalid", Some("server-b"));
    manager.update_config(&updated);

    let error = worker.join().unwrap().err().unwrap();
    let snapshot = manager.snapshot().unwrap();

    assert_eq!(error.kind, PlexErrorKind::ConfigurationChanged);
    assert_eq!(snapshot.identity.unwrap().machine_identifier, "server-b");
    assert!(snapshot.current_base_url.is_none());
    assert!(snapshot.resolved_token.is_none());
}

#[test]
fn resources_order_never_selects_a_different_server() {
    let legacy = unreachable_base_url();
    let wrong_requests = Arc::new(AtomicUsize::new(0));
    let wrong = pms_server("different-server", "200 OK", wrong_requests.clone());
    let right_requests = Arc::new(AtomicUsize::new(0));
    let right = pms_server("selected-server", "200 OK", right_requests);
    let resources = resources_server(vec![
        resource_json(
            "different-server",
            "Wrong Server",
            vec![connection_json(&wrong.base_url, true, false)],
        ),
        resource_json(
            "selected-server",
            "Selected Server",
            vec![connection_json(&right.base_url, false, false)],
        ),
    ]);
    let config = manager_config(&legacy, Some("selected-server"));
    let manager = test_manager(&config, format!("{}/api/v2/resources", resources.base_url));

    let resolved = block_on(manager.resolve()).unwrap();

    assert_eq!(resolved.base_url, right.base_url);
    assert_eq!(wrong_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn legacy_route_failure_without_identity_requires_server_reselection() {
    let legacy = unreachable_base_url();
    let config = manager_config(&legacy, None);
    let manager = test_manager(&config, unreachable_base_url());

    let error = block_on(manager.resolve()).err().unwrap();

    assert_eq!(error.kind, PlexErrorKind::ServerSelectionRequired);
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
        })
        .block_on(future)
}
#[test]
fn windows_source_flag_is_disabled_by_default_and_generation_preserves_credentials() {
    let config = AppConfig::default();
    let manager = test_manager(&config, unreachable_base_url());
    assert!(matches!(
        manager.snapshot(),
        Err(PlexError {
            kind: PlexErrorKind::SourceDisabled,
            ..
        })
    ));
    let mut config = manager_config("http://127.0.0.1:9", Some("selected-server"));
    manager.update_config(&config);
    let before = manager.snapshot().unwrap().generation;
    config.enabled = false;
    manager.update_config(&config);
    assert!(matches!(
        manager.snapshot(),
        Err(PlexError {
            kind: PlexErrorKind::SourceDisabled,
            ..
        })
    ));
    assert_eq!(
        manager.state.lock().unwrap().account_token,
        "TEST_ACCOUNT_TOKEN"
    );
    config.enabled = true;
    manager.update_config(&config);
    assert_ne!(manager.snapshot().unwrap().generation, before);
}
#[test]
fn windows_metadata_length_and_chunked_caps_reject_oversized_or_redirected_bodies() {
    for mode in ["length", "chunked", "redirect"] {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut socket, _) = server.accept().unwrap();
            let mut request = [0; 8192];
            let _ = socket.read(&mut request);
            let response=match mode{
                "length"=>format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",2*1024*1024+1),
                "chunked"=>format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",2*1024*1024+1,"x".repeat(2*1024*1024+1)),
                _=>format!("HTTP/1.1 302 Found\r\nLocation: http://{address}/forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
            };
            let _ = socket.write_all(response.as_bytes());
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let result: Result<serde_json::Value, _> = block_on(send_json(
            client.get(format!("http://{address}/fixture")),
            "fixture",
        ));
        worker.join().unwrap();
        let error = result.unwrap_err();
        assert_eq!(
            error.kind,
            if mode == "redirect" {
                PlexErrorKind::HttpError(302)
            } else {
                PlexErrorKind::InvalidResponse
            }
        );
        assert!(!error.public_message().contains("http"));
    }
}
#[test]
fn windows_settings_scope_and_shutdown_reject_without_any_server_access() {
    let default = super::super::Settings::default();
    assert!(default.validate().is_ok());
    let client = super::super::Client::new(&default).unwrap();
    assert_eq!(
        block_on(client.json("/library/sections")).unwrap_err(),
        "SOURCE_DISABLED"
    );
    for url in [
        "file:///private",
        "http://user:PRIVATE@127.0.0.1/",
        "http://127.0.0.1/?token=PRIVATE",
        "http://127.0.0.1/#PRIVATE",
        "http://127.0.0.1/\\bad",
    ] {
        let mut invalid = default.clone();
        invalid.plex_url = url.into();
        assert!(super::super::Client::new(&invalid).is_err());
    }
    for path in [
        "https://PRIVATE/",
        "//PRIVATE/",
        "/library?X-Plex-Token=PRIVATE",
        "/library\r\nPRIVATE",
        "/library#PRIVATE",
    ] {
        assert_eq!(block_on(client.json(path)).unwrap_err(), "PLEX_INVALID");
    }
    let slots: Vec<_> = (0..4)
        .map(|_| client.network.clone().try_acquire_owned().unwrap())
        .collect();
    assert_eq!(
        block_on(client.json("/library/sections")).unwrap_err(),
        "PLEX_UNAVAILABLE"
    );
    drop(slots);
    client.shutdown();
    assert!(block_on(client.json("/library/sections")).is_err());
    assert!(client.update(&default).is_err());
}
fn image_client(
    status_line: &'static str,
    content_type: &'static str,
    body: String,
) -> (super::super::Client, MockServer) {
    image_client_with_handler(move |_| MockResponse {
        status_line,
        content_type,
        body: body.clone(),
        delay: Duration::ZERO,
    })
}

fn image_client_with_handler<F>(handler: F) -> (super::super::Client, MockServer)
where
    F: Fn(&str) -> MockResponse + Send + Sync + 'static,
{
    let server = MockServer::start(move |path| {
        if path.starts_with("/identity") {
            MockResponse::json(
                "200 OK",
                serde_json::json!({
                    "MediaContainer": { "machineIdentifier": "fixture-machine-id" }
                }),
            )
        } else if path == "/library/sections" {
            MockResponse::json("200 OK", serde_json::json!({"MediaContainer": {}}))
        } else {
            handler(path)
        }
    });
    let config = manager_config(&server.base_url, Some("fixture-machine-id"));
    (media_client_for_test(&config), server)
}

fn image_ref(path: &str) -> PlexImageRef {
    PlexImageRef {
        server_id: "fixture-machine-id".to_string(),
        path: path.to_string(),
    }
}

#[test]
fn public_image_reference_contains_only_server_identity_and_relative_path() {
    let connection = ResolvedPlexConnection {
        server_id: "fixture-machine-id".to_string(),
        base_url: "https://private-route.invalid".to_string(),
        token: "SECRET".to_string(),
        generation: 0,
    };
    let image =
        PlexImageRef::from_connection(&connection, "/library/metadata/42/thumb/1?width=300")
            .unwrap();
    let json = serde_json::to_string(&image).unwrap();

    assert_eq!(image.server_id, "fixture-machine-id");
    assert_eq!(image.path, "/library/metadata/42/thumb/1?width=300");
    assert!(!json.contains("private-route.invalid"));
    assert!(!json.contains("SECRET"));
    assert!(!json.contains("X-Plex-Token"));
}

#[test]
fn valid_image_reference_returns_only_response_bytes() {
    let transcode_requests = Arc::new(AtomicUsize::new(0));
    let observed = transcode_requests.clone();
    let (client, _server) = image_client_with_handler(move |path| {
        if path.starts_with("/photo/:/transcode?") {
            observed.fetch_add(1, Ordering::SeqCst);
        }
        MockResponse {
            status_line: "200 OK",
            content_type: "image/jpeg",
            body: "jpeg-bytes".to_string(),
            delay: Duration::ZERO,
        }
    });
    let bytes = block_on(client.image(&image_ref("/library/metadata/42/thumb/1"))).unwrap();
    assert_eq!(bytes, b"jpeg-bytes");
    assert_eq!(transcode_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn oversized_image_uses_encoded_transcode_path_and_returns_its_bytes() {
    let transcode_path = Arc::new(Mutex::new(None));
    let observed_path = transcode_path.clone();
    let oversized = "x".repeat(MAX_PLEX_IMAGE_BYTES + 1);
    let (client, _server) = image_client_with_handler(move |path| {
        if path.starts_with("/photo/:/transcode?") {
            *observed_path.lock().unwrap() = Some(path.to_string());
            MockResponse {
                status_line: "200 OK",
                content_type: "image/jpeg",
                body: "transcoded-image".to_string(),
                delay: Duration::ZERO,
            }
        } else {
            MockResponse {
                status_line: "200 OK",
                content_type: "image/jpeg",
                body: oversized.clone(),
                delay: Duration::ZERO,
            }
        }
    });
    let original_path = "/library/metadata/42/thumb/1?quality=high&crop=1";

    let bytes = block_on(client.image(&image_ref(original_path))).unwrap();
    let requested_path = transcode_path.lock().unwrap().clone().unwrap();

    assert_eq!(bytes, b"transcoded-image");
    assert_eq!(
            requested_path,
            "/photo/:/transcode?width=600&height=600&minSize=1&upscale=1&url=%2Flibrary%2Fmetadata%2F42%2Fthumb%2F1%3Fquality%3Dhigh%26crop%3D1"
        );
    assert!(!requested_path.to_ascii_lowercase().contains("x-plex-token"));
    assert!(!requested_path.contains("TEST_ACCOUNT_TOKEN"));
}

#[test]
fn oversized_transcode_response_is_still_rejected() {
    let oversized = "x".repeat(MAX_PLEX_IMAGE_BYTES + 1);
    let (client, _server) = image_client("200 OK", "image/jpeg", oversized);

    let error = block_on(client.image(&image_ref("/library/metadata/42/thumb/1"))).unwrap_err();

    assert_eq!(error, "A imagem Plex excede o limite permitido.");
}

#[test]
fn invalid_transcode_content_type_is_rejected() {
    let oversized = "x".repeat(MAX_PLEX_IMAGE_BYTES + 1);
    let (client, _server) = image_client_with_handler(move |path| {
        if path.starts_with("/photo/:/transcode?") {
            MockResponse {
                status_line: "200 OK",
                content_type: "text/html",
                body: "SECRET TRANSCODE BODY".to_string(),
                delay: Duration::ZERO,
            }
        } else {
            MockResponse {
                status_line: "200 OK",
                content_type: "image/jpeg",
                body: oversized.clone(),
                delay: Duration::ZERO,
            }
        }
    });

    let error = block_on(client.image(&image_ref("/library/metadata/42/thumb/1"))).unwrap_err();

    assert_eq!(error, "O Plex retornou um formato de imagem não suportado.");
    assert!(!error.contains("SECRET"));
}

#[test]
fn image_reference_rejects_wrong_server_absolute_url_and_token() {
    let config = manager_config("http://127.0.0.1:9", Some("fixture-machine-id"));
    let client = media_client_for_test(&config);
    for image in [
        PlexImageRef {
            server_id: "another-server".to_string(),
            path: "/library/metadata/42/thumb/1".to_string(),
        },
        image_ref("https://outside.invalid/image.jpg"),
        image_ref("//outside.invalid/image.jpg"),
        image_ref("/library/metadata/42/thumb/1?x-plex-token=SECRET"),
        image_ref("/photo/:/transcode?width=600&height=600&url=%2Flibrary%2Fmetadata%2F42"),
    ] {
        let error = block_on(client.image(&image)).unwrap_err();
        assert_eq!(error, "A referência da imagem Plex é inválida.");
        assert!(!error.contains("SECRET"));
        assert!(!error.contains("outside.invalid"));
    }
}

#[test]
fn image_http_errors_are_sanitized() {
    for status in [
        "401 Unauthorized",
        "404 Not Found",
        "500 Internal Server Error",
    ] {
        let transcode_requests = Arc::new(AtomicUsize::new(0));
        let observed = transcode_requests.clone();
        let (client, _server) = image_client_with_handler(move |path| {
            if path.starts_with("/photo/:/transcode?") {
                observed.fetch_add(1, Ordering::SeqCst);
            }
            MockResponse {
                status_line: status,
                content_type: "text/plain",
                body: "SECRET RESPONSE BODY".to_string(),
                delay: Duration::ZERO,
            }
        });
        let error = block_on(client.image(&image_ref("/library/metadata/42/thumb/1"))).unwrap_err();
        assert!(!error.contains("SECRET"));
        assert!(!error.contains("X-Plex-Token"));
        assert!(!error.contains("127.0.0.1"));
        assert_eq!(transcode_requests.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn image_type_and_size_are_limited() {
    let (client, _server) = image_client("200 OK", "text/html", "not an image".to_string());
    let error = block_on(client.image(&image_ref("/library/metadata/42/thumb/1"))).unwrap_err();
    assert_eq!(error, "O Plex retornou um formato de imagem não suportado.");
}
fn media_client_for_test(config: &AppConfig) -> super::super::Client {
    let (stop, _) = tokio::sync::watch::channel(false);
    super::super::Client {
        manager: Arc::new(test_manager(config, unreachable_base_url())),
        network: Arc::new(tokio::sync::Semaphore::new(4)),
        artwork:Arc::new(tokio::sync::Semaphore::new(2)),
        stop,
    }
}
#[test]
fn windows_image_context_change_and_shutdown_discard_pending_bytes() {
    for closing in [false, true] {
        let started = Arc::new(AtomicBool::new(false));
        let observed = started.clone();
        let (client, server) = image_client_with_handler(move |_| {
            observed.store(true, Ordering::Release);
            MockResponse {
                status_line: "200 OK",
                content_type: "image/jpeg",
                body: "OLD-CONTEXT-BYTES".into(),
                delay: Duration::from_millis(100),
            }
        });
        let reader = client.clone();
        let task = thread::spawn(move || {
            block_on(reader.image(&image_ref("/library/metadata/42/thumb/1")))
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        if closing {
            client.shutdown();
        } else {
            let mut config = manager_config(&server.base_url, Some("fixture-machine-id"));
            config.plex_token = "NEW-TOKEN".into();
            client.update(&config).unwrap();
        }
        assert_eq!(task.join().unwrap().unwrap_err(), "PLEX_CONTEXT_CHANGED");
    }
}

#[test]
fn windows_authentication_query_encodes_the_token_as_one_value() {
    let observed = Arc::new(Mutex::new(None));
    let seen = observed.clone();
    let server = MockServer::start(move |path| {
        *seen.lock().unwrap() = Some(path.to_owned());
        MockResponse::json("200 OK", serde_json::json!({"MediaContainer":{}}))
    });
    let mut config = manager_config(&server.base_url, Some("selected-server"));
    config.plex_token = "TOKEN&injected=PRIVATE +?#".into();
    let manager = test_manager(&config, unreachable_base_url());
    let connection = ResolvedPlexConnection {
        server_id: "selected-server".into(),
        base_url: server.base_url.clone(),
        token: config.plex_token.clone(),
        generation: 0,
    };
    let _: serde_json::Value = block_on(manager.send_authenticated(
        &connection,
        "/library/metadata/1?include=1",
        "fixture",
    ))
    .unwrap();
    let path = observed.lock().unwrap().clone().unwrap();
    let url = reqwest::Url::parse(&format!("http://fixture.invalid{path}")).unwrap();
    let params: Vec<_> = url.query_pairs().collect();
    assert_eq!(
        params
            .iter()
            .filter(|(key, _)| key == "X-Plex-Token")
            .count(),
        1
    );
    assert_eq!(
        params
            .iter()
            .find(|(key, _)| key == "X-Plex-Token")
            .unwrap()
            .1,
        config.plex_token
    );
    assert!(!params.iter().any(|(key, _)| key == "injected"));
}

// A selected shared server can grant a resource credential distinct from the
// login credential. Keep both synthetic secrets only inside this fixture.
#[test]
fn shared_account_initial_validation_uses_exact_selected_resource_credential() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let seen = observed.clone();
    let pms = MockServer::start_request(move |path, request| {
        let token = request.lines().find_map(|line| line.split_once(':')
            .filter(|(name,_)|name.eq_ignore_ascii_case("x-plex-token"))
            .map(|(_,value)|value.trim())).unwrap_or("");
        seen.lock().unwrap().push((path.to_owned(), token.to_owned()));
        if path == "/identity" {
            MockResponse::json("200 OK", serde_json::json!({"MediaContainer":{"machineIdentifier":"selected-server"}}))
        } else if token == "TEST_RESOURCE_TOKEN" {
            MockResponse::json("200 OK", serde_json::json!({"MediaContainer":{"Directory":[]}}))
        } else { MockResponse::json("401 Unauthorized", serde_json::json!({})) }
    });
    let resources = resources_server(vec![resource_json("selected-server", "Shared", vec![connection_json(&pms.base_url,true,false)])]);
    let manager = test_manager(&manager_config(&pms.base_url,Some("selected-server")),format!("{}/api/v2/resources",resources.base_url));
    let resolved = block_on(manager.resolve()).unwrap();
    assert_eq!(resolved.server_id,"selected-server");
    assert_eq!(resolved.token,"TEST_RESOURCE_TOKEN");
    assert_eq!(manager.snapshot().unwrap().account_token,"TEST_ACCOUNT_TOKEN");
    assert_eq!(observed.lock().unwrap().iter().filter(|(path,token)|path=="/library/sections"&&token=="TEST_ACCOUNT_TOKEN").count(),1);
    assert_eq!(observed.lock().unwrap().iter().filter(|(path,token)|path=="/library/sections"&&token=="TEST_RESOURCE_TOKEN").count(),1);
}

#[test]
fn shared_account_initial_denials_remain_bounded_selected_and_unpublished() {
    // Known identity is required. Missing grant, a different server and denied
    // resource authentication never publish a connection or erase the account.
    for case in ["no-selection", "other-server", "resource-denied", "discovery-denied"] {
        let pms = MockServer::start(|path| {
            if path == "/identity" { MockResponse::json("200 OK",serde_json::json!({"MediaContainer":{"machineIdentifier":"selected-server"}})) }
            else { MockResponse::json("401 Unauthorized",serde_json::json!({})) }
        });
        let calls = Arc::new(AtomicUsize::new(0)); let count = calls.clone();
        let payload = resource_json(if case=="other-server" {"unselected-server"}else{"selected-server"},"Synthetic",vec![connection_json(&pms.base_url,true,false)]);
        let resources = MockServer::start(move |_| {
            count.fetch_add(1,Ordering::SeqCst);
            if case=="discovery-denied" { MockResponse::json("401 Unauthorized",serde_json::json!({})) }
            else { MockResponse::json("200 OK",serde_json::json!([payload])) }
        });
        let config=manager_config(&pms.base_url,if case=="no-selection" {None}else{Some("selected-server")});
        let manager=test_manager(&config,format!("{}/api/v2/resources",resources.base_url));
        let error=block_on(manager.resolve()).err().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst),usize::from(case!="no-selection"),"{case}");
        let snapshot=manager.snapshot().unwrap();assert!(snapshot.current_base_url.is_none(),"{case}");assert!(snapshot.resolved_token.is_none(),"{case}");
        assert_eq!(snapshot.account_token,"TEST_ACCOUNT_TOKEN");
        if case=="other-server" {assert_eq!(error.kind,PlexErrorKind::ServerNotFound);}
        else {assert_eq!(error.kind,PlexErrorKind::Unauthorized);}
        let public=error.public_message();assert!(!public.contains("TEST_ACCOUNT_TOKEN"));assert!(!public.contains("TEST_RESOURCE_TOKEN"));assert!(!public.contains(&pms.base_url));
    }
}
