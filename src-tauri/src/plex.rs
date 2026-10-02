use crate::config::AppConfig;
use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const PLEX_CLIENT_ID: &str = "sonante-audio-player";
const PLEX_PRODUCT_NAME: &str = "Sonante";
const PLEX_VERSION: &str = "0.2.0";
const PLEX_RESOURCES_URL: &str =
    "https://plex.tv/api/v2/resources?includeHttps=1&includeRelay=1";
const PLEX_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlexErrorKind {
    Timeout,
    Transport,
    Unauthorized,
    NotFound,
    ServerError(u16),
    HttpError(u16),
    InvalidResponse,
    IdentityMismatch,
    ServerSelectionRequired,
    ServerNotFound,
    ConfigurationChanged,
    StateUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlexError {
    operation: &'static str,
    kind: PlexErrorKind,
}

impl PlexError {
    fn from_transport(operation: &'static str, error: &reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            PlexErrorKind::Timeout
        } else {
            PlexErrorKind::Transport
        };
        Self { operation, kind }
    }

    fn from_status(operation: &'static str, status: StatusCode) -> Self {
        let kind = match status {
            StatusCode::UNAUTHORIZED => PlexErrorKind::Unauthorized,
            StatusCode::NOT_FOUND => PlexErrorKind::NotFound,
            status if status.is_server_error() => PlexErrorKind::ServerError(status.as_u16()),
            status => PlexErrorKind::HttpError(status.as_u16()),
        };
        Self { operation, kind }
    }

    fn invalid_response(operation: &'static str) -> Self {
        Self {
            operation,
            kind: PlexErrorKind::InvalidResponse,
        }
    }

    fn public_message(&self) -> String {
        match self.kind {
            PlexErrorKind::Timeout => {
                format!("Tempo limite ao {} no Plex.", self.operation)
            }
            PlexErrorKind::Transport => {
                format!("Não foi possível {} no Plex por erro de conexão.", self.operation)
            }
            PlexErrorKind::Unauthorized => {
                format!("O Plex recusou a autenticação ao {} (HTTP 401).", self.operation)
            }
            PlexErrorKind::NotFound => {
                format!("O recurso solicitado não foi encontrado ao {} no Plex (HTTP 404).", self.operation)
            }
            PlexErrorKind::ServerError(status) => {
                format!("O servidor Plex falhou ao {} (HTTP {}).", self.operation, status)
            }
            PlexErrorKind::HttpError(status) => {
                format!("O Plex recusou a operação de {} (HTTP {}).", self.operation, status)
            }
            PlexErrorKind::InvalidResponse => {
                format!("O Plex retornou uma resposta inválida ao {}.", self.operation)
            }
            PlexErrorKind::IdentityMismatch => {
                "A rota Plex respondeu como outro servidor e foi rejeitada.".to_string()
            }
            PlexErrorKind::ServerSelectionRequired => {
                "Não foi possível identificar com segurança o servidor Plex. Selecione o servidor novamente nas configurações.".to_string()
            }
            PlexErrorKind::ServerNotFound => {
                "O servidor Plex selecionado não foi encontrado na descoberta da conta.".to_string()
            }
            PlexErrorKind::ConfigurationChanged => {
                "A configuração Plex mudou durante a conexão. Tente novamente.".to_string()
            }
            PlexErrorKind::StateUnavailable => {
                "O estado da conexão Plex está indisponível.".to_string()
            }
        }
    }

    fn is_route_unavailable(&self) -> bool {
        matches!(self.kind, PlexErrorKind::Timeout | PlexErrorKind::Transport)
    }
}

async fn send_json<T: DeserializeOwned>(
    request: RequestBuilder,
    operation: &'static str,
) -> Result<T, PlexError> {
    let response = request
        .send()
        .await
        .map_err(|error| PlexError::from_transport(operation, &error))?;

    if !response.status().is_success() {
        return Err(PlexError::from_status(operation, response.status()));
    }

    let body = response
        .bytes()
        .await
        .map_err(|error| PlexError::from_transport(operation, &error))?;

    serde_json::from_slice(&body).map_err(|_| PlexError::invalid_response(operation))
}

fn media_container<'a>(
    json: &'a serde_json::Value,
    operation: &'static str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, PlexError> {
    json.get("MediaContainer")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| PlexError::invalid_response(operation))
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(10)
            .build()
            .unwrap_or_default()
    })
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexPin {
    pub id: u64,
    pub code: String,
    pub auth_url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexConnection {
    pub uri: String,
    pub local: bool,
    pub address: String,
    pub port: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexServerResource {
    pub name: String,
    pub client_identifier: String,
    pub connections: Vec<PlexConnection>,
    pub chosen_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlexServerIdentity {
    pub machine_identifier: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlexConnectionCandidate {
    pub base_url: String,
    pub local: bool,
    pub relay: bool,
    pub protocol: String,
    address: String,
    port: u16,
}

#[derive(Clone, PartialEq, Eq)]
struct DiscoveredPlexServer {
    identity: PlexServerIdentity,
    connections: Vec<PlexConnectionCandidate>,
    resource_access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConnection {
    uri: String,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    relay: bool,
    #[serde(default)]
    protocol: String,
    address: String,
    port: u16,
}

#[derive(Deserialize)]
struct RawResource {
    name: String,
    #[serde(rename = "clientIdentifier")]
    client_identifier: String,
    provides: String,
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(default)]
    connections: Vec<RawConnection>,
}

fn parse_discovered_servers(resources: Vec<RawResource>) -> Vec<DiscoveredPlexServer> {
    resources
        .into_iter()
        .filter(|resource| {
            resource
                .provides
                .split(',')
                .any(|provided| provided.trim() == "server")
        })
        .map(|resource| DiscoveredPlexServer {
            identity: PlexServerIdentity {
                machine_identifier: resource.client_identifier,
                display_name: resource.name,
            },
            connections: resource
                .connections
                .into_iter()
                .map(|connection| PlexConnectionCandidate {
                    base_url: connection.uri,
                    local: connection.local,
                    relay: connection.relay,
                    protocol: connection.protocol,
                    address: connection.address,
                    port: connection.port,
                })
                .collect(),
            resource_access_token: resource.access_token,
        })
        .collect()
}

fn into_public_server_resource(server: DiscoveredPlexServer) -> PlexServerResource {
    let mut connections: Vec<PlexConnection> = server
        .connections
        .into_iter()
        .map(|candidate| PlexConnection {
            uri: candidate.base_url,
            local: candidate.local,
            address: candidate.address,
            port: candidate.port,
        })
        .collect();

    // Compatibilidade: a API pública continua priorizando a primeira conexão marcada como local.
    connections.sort_by_key(|connection| if connection.local { 0 } else { 1 });
    let chosen_uri = connections
        .first()
        .map(|connection| connection.uri.clone())
        .unwrap_or_default();

    PlexServerResource {
        name: server.identity.display_name,
        client_identifier: server.identity.machine_identifier,
        connections,
        chosen_uri,
    }
}

async fn discover_plex_servers(
    client: &reqwest::Client,
    resources_url: &str,
    auth_token: &str,
) -> Result<Vec<DiscoveredPlexServer>, PlexError> {
    let request = client
        .get(resources_url)
        .header("X-Plex-Client-Identifier", PLEX_CLIENT_ID)
        .header("X-Plex-Token", auth_token)
        .header("Accept", "application/json");
    let resources: Vec<RawResource> = send_json(request, "consultar os servidores").await?;
    Ok(parse_discovered_servers(resources))
}

#[derive(Clone)]
struct ResolvedPlexConnection {
    base_url: String,
    token: String,
    generation: u64,
}

#[derive(Clone)]
struct PlexConnectionSnapshot {
    identity: Option<PlexServerIdentity>,
    current_base_url: Option<String>,
    legacy_base_url: Option<String>,
    account_token: String,
    resolved_token: Option<String>,
    generation: u64,
}

struct PlexConnectionState {
    identity: Option<PlexServerIdentity>,
    current_base_url: Option<String>,
    legacy_base_url: Option<String>,
    account_token: String,
    resolved_token: Option<String>,
    generation: u64,
}

struct PlexConnectionManager {
    state: Mutex<PlexConnectionState>,
    http: reqwest::Client,
    probe_http: reqwest::Client,
    resources_url: String,
}

impl PlexConnectionManager {
    fn from_config(config: &AppConfig) -> Self {
        let probe_http = reqwest::Client::builder()
            .timeout(PLEX_PROBE_TIMEOUT)
            .build()
            .unwrap_or_else(|_| http_client().clone());
        Self::with_clients(
            config,
            http_client().clone(),
            probe_http,
            PLEX_RESOURCES_URL.to_string(),
        )
    }

    fn with_clients(
        config: &AppConfig,
        http: reqwest::Client,
        probe_http: reqwest::Client,
        resources_url: String,
    ) -> Self {
        let identity = config
            .plex_server_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .map(|machine_identifier| PlexServerIdentity {
                machine_identifier: machine_identifier.clone(),
                display_name: config.plex_server_name.clone().unwrap_or_default(),
            });
        let legacy_base_url = normalized_base_url(&config.plex_url);
        Self {
            state: Mutex::new(PlexConnectionState {
                identity,
                current_base_url: None,
                legacy_base_url,
                account_token: config.plex_token.clone(),
                resolved_token: None,
                generation: 0,
            }),
            http,
            probe_http,
            resources_url,
        }
    }

    fn snapshot(&self) -> Result<PlexConnectionSnapshot, PlexError> {
        let state = self.state.lock().map_err(|_| PlexError {
            operation: "acessar o estado da conexão",
            kind: PlexErrorKind::StateUnavailable,
        })?;
        Ok(PlexConnectionSnapshot {
            identity: state.identity.clone(),
            current_base_url: state.current_base_url.clone(),
            legacy_base_url: state.legacy_base_url.clone(),
            account_token: state.account_token.clone(),
            resolved_token: state.resolved_token.clone(),
            generation: state.generation,
        })
    }

    fn update_config(&self, config: &AppConfig) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let identity = config
            .plex_server_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .map(|machine_identifier| PlexServerIdentity {
                machine_identifier: machine_identifier.clone(),
                display_name: config.plex_server_name.clone().unwrap_or_default(),
            });
        let legacy_base_url = normalized_base_url(&config.plex_url);
        let connection_changed = state.identity != identity
            || state.legacy_base_url != legacy_base_url
            || state.account_token != config.plex_token;

        if connection_changed {
            state.generation = state.generation.wrapping_add(1);
            state.current_base_url = None;
            state.resolved_token = None;
        }
        state.identity = identity;
        state.legacy_base_url = legacy_base_url;
        state.account_token = config.plex_token.clone();
    }

    async fn resolve(&self) -> Result<ResolvedPlexConnection, PlexError> {
        let snapshot = self.snapshot()?;
        if let Some(base_url) = snapshot.current_base_url.clone() {
            return Ok(ResolvedPlexConnection {
                base_url,
                token: snapshot
                    .resolved_token
                    .clone()
                    .unwrap_or_else(|| snapshot.account_token.clone()),
                generation: snapshot.generation,
            });
        }

        let Some(legacy_base_url) = snapshot.legacy_base_url.clone() else {
            return Err(PlexError {
                operation: "resolver o servidor",
                kind: PlexErrorKind::ServerSelectionRequired,
            });
        };
        if snapshot.account_token.trim().is_empty() {
            return Err(PlexError {
                operation: "autenticar no servidor",
                kind: PlexErrorKind::Unauthorized,
            });
        }

        match self
            .validate_candidate(
                &legacy_base_url,
                &snapshot.account_token,
                snapshot.identity.as_ref().map(|identity| identity.machine_identifier.as_str()),
            )
            .await
        {
            Ok(observed_identity) => {
                let identity = snapshot.identity.clone().unwrap_or(PlexServerIdentity {
                    machine_identifier: observed_identity,
                    display_name: String::new(),
                });
                self.publish_connection(
                    snapshot.generation,
                    identity,
                    legacy_base_url,
                    snapshot.account_token.clone(),
                )
            }
            Err(error)
                if snapshot.identity.is_some()
                    && (error.is_route_unavailable()
                        || error.kind == PlexErrorKind::IdentityMismatch) =>
            {
                self.refresh(snapshot.generation).await
            }
            Err(error) if snapshot.identity.is_none() && error.is_route_unavailable() => {
                Err(PlexError {
                    operation: "resolver o servidor legado",
                    kind: PlexErrorKind::ServerSelectionRequired,
                })
            }
            Err(error) => Err(error),
        }
    }

    async fn refresh(&self, expected_generation: u64) -> Result<ResolvedPlexConnection, PlexError> {
        let snapshot = self.snapshot()?;
        if snapshot.generation != expected_generation {
            return Err(PlexError {
                operation: "publicar a conexão resolvida",
                kind: PlexErrorKind::ConfigurationChanged,
            });
        }
        let expected_identity = snapshot.identity.clone().ok_or(PlexError {
            operation: "redescobrir o servidor",
            kind: PlexErrorKind::ServerSelectionRequired,
        })?;

        let servers = discover_plex_servers(
            &self.probe_http,
            &self.resources_url,
            &snapshot.account_token,
        )
        .await?;
        let server = servers
            .into_iter()
            .find(|server| {
                server.identity.machine_identifier == expected_identity.machine_identifier
            })
            .ok_or(PlexError {
                operation: "redescobrir o servidor",
                kind: PlexErrorKind::ServerNotFound,
            })?;
        let credential = server
            .resource_access_token
            .clone()
            .unwrap_or_else(|| snapshot.account_token.clone());
        let candidates = prioritized_candidates(
            server.connections,
            snapshot.current_base_url.as_deref().or(snapshot.legacy_base_url.as_deref()),
        );

        let mut last_route_error = None;
        for candidate in candidates {
            match self
                .validate_candidate(
                    &candidate.base_url,
                    &credential,
                    Some(&expected_identity.machine_identifier),
                )
                .await
            {
                Ok(_) => {
                    return self.publish_connection(
                        expected_generation,
                        expected_identity,
                        candidate.base_url,
                        credential,
                    );
                }
                Err(error)
                    if error.is_route_unavailable()
                        || error.kind == PlexErrorKind::IdentityMismatch =>
                {
                    last_route_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_route_error.unwrap_or(PlexError {
            operation: "conectar ao servidor selecionado",
            kind: PlexErrorKind::Transport,
        }))
    }

    async fn validate_candidate(
        &self,
        base_url: &str,
        token: &str,
        expected_machine_identifier: Option<&str>,
    ) -> Result<String, PlexError> {
        let identity_url = format!("{}/identity", base_url.trim_end_matches('/'));
        let identity_json: serde_json::Value = send_json(
            self.probe_http
                .get(identity_url)
                .header("X-Plex-Token", token)
                .header("Accept", "application/json"),
            "validar a identidade do servidor",
        )
        .await?;
        let container = media_container(&identity_json, "validar a identidade do servidor")?;
        let observed = container
            .get("machineIdentifier")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| PlexError::invalid_response("validar a identidade do servidor"))?;
        if expected_machine_identifier.is_some_and(|expected| expected != observed) {
            return Err(PlexError {
                operation: "validar a identidade do servidor",
                kind: PlexErrorKind::IdentityMismatch,
            });
        }

        let authenticated_url = format!("{}/library/sections", base_url.trim_end_matches('/'));
        let authenticated_json: serde_json::Value = send_json(
            self.probe_http
                .get(authenticated_url)
                .header("X-Plex-Token", token)
                .header("Accept", "application/json"),
            "validar o acesso autenticado ao servidor",
        )
        .await?;
        media_container(
            &authenticated_json,
            "validar o acesso autenticado ao servidor",
        )?;
        Ok(observed.to_string())
    }

    fn publish_connection(
        &self,
        expected_generation: u64,
        identity: PlexServerIdentity,
        base_url: String,
        token: String,
    ) -> Result<ResolvedPlexConnection, PlexError> {
        let mut state = self.state.lock().map_err(|_| PlexError {
            operation: "publicar a conexão resolvida",
            kind: PlexErrorKind::StateUnavailable,
        })?;
        if state.generation != expected_generation {
            return Err(PlexError {
                operation: "publicar a conexão resolvida",
                kind: PlexErrorKind::ConfigurationChanged,
            });
        }
        state.identity = Some(identity);
        state.current_base_url = Some(base_url.clone());
        state.resolved_token = Some(token.clone());
        Ok(ResolvedPlexConnection {
            base_url,
            token,
            generation: expected_generation,
        })
    }

    async fn request_json<T: DeserializeOwned>(
        &self,
        path_and_query: &str,
        operation: &'static str,
    ) -> Result<(T, ResolvedPlexConnection), PlexError> {
        let connection = self.resolve().await?;
        let first = self
            .send_authenticated(&connection, path_and_query, operation)
            .await;
        match first {
            Ok(value) => Ok((value, connection)),
            Err(error) if error.is_route_unavailable() => {
                let refreshed = self.refresh(connection.generation).await?;
                let value = self
                    .send_authenticated(&refreshed, path_and_query, operation)
                    .await?;
                Ok((value, refreshed))
            }
            Err(error) => Err(error),
        }
    }

    async fn send_authenticated<T: DeserializeOwned>(
        &self,
        connection: &ResolvedPlexConnection,
        path_and_query: &str,
        operation: &'static str,
    ) -> Result<T, PlexError> {
        let separator = if path_and_query.contains('?') { '&' } else { '?' };
        let url = format!(
            "{}{}{}X-Plex-Token={}",
            connection.base_url,
            path_and_query,
            separator,
            connection.token
        );
        send_json(
            self.http.get(url).header("Accept", "application/json"),
            operation,
        )
        .await
    }
}

fn normalized_base_url(value: &str) -> Option<String> {
    let normalized = value.trim().trim_end_matches('/');
    (!normalized.is_empty()).then(|| normalized.to_string())
}

fn prioritized_candidates(
    candidates: Vec<PlexConnectionCandidate>,
    previous_base_url: Option<&str>,
) -> Vec<PlexConnectionCandidate> {
    let mut candidates = candidates;
    candidates.sort_by_key(|candidate| {
        let previous_rank = usize::from(previous_base_url != Some(candidate.base_url.as_str()));
        let class_rank = if candidate.local && !candidate.relay {
            0
        } else if !candidate.local && !candidate.relay {
            1
        } else {
            2
        };
        let is_https = candidate.protocol.eq_ignore_ascii_case("https")
            || candidate.base_url.to_ascii_lowercase().starts_with("https://");
        let protocol_rank = usize::from(!is_https);
        (previous_rank, class_rank, protocol_rank)
    });

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| seen.insert(candidate.base_url.clone()))
        .collect()
}

pub async fn request_plex_pin() -> Result<PlexPin, String> {
    let client = http_client();
    let request = client
        .post("https://plex.tv/api/v2/pins")
        .header("X-Plex-Product", PLEX_PRODUCT_NAME)
        .header("X-Plex-Version", PLEX_VERSION)
        .header("X-Plex-Client-Identifier", PLEX_CLIENT_ID)
        .header("X-Plex-Platform", "Linux")
        .header("X-Plex-Device", "PC")
        .header("X-Plex-Device-Name", "Sonante (Linux)")
        .header("Accept", "application/json")
        .query(&[("strong", "true")]);

    #[derive(Deserialize)]
    struct RawPin {
        id: u64,
        code: String,
    }

    let pin: RawPin = send_json(request, "solicitar um PIN")
        .await
        .map_err(|error| error.public_message())?;

    // URL oficial moderna do Plex OAuth (usa '#?' sem exclamação e parâmetros estritamente alinhados)
    let auth_url = format!(
        "https://app.plex.tv/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D={}&context%5Bdevice%5D%5Bplatform%5D=Linux&context%5Bdevice%5D%5Bdevice%5D=PC",
        PLEX_CLIENT_ID, pin.code, PLEX_PRODUCT_NAME
    );

    Ok(PlexPin {
        id: pin.id,
        code: pin.code,
        auth_url,
    })
}

pub async fn check_plex_pin(pin_id: u64) -> Result<Option<String>, String> {
    let client = http_client();
    let url = format!("https://plex.tv/api/v2/pins/{}", pin_id);
    let request = client
        .get(&url)
        .header("X-Plex-Product", PLEX_PRODUCT_NAME)
        .header("X-Plex-Version", PLEX_VERSION)
        .header("X-Plex-Client-Identifier", PLEX_CLIENT_ID)
        .header("Accept", "application/json");

    #[derive(Deserialize)]
    struct RawPinCheck {
        #[serde(rename = "authToken")]
        auth_token: Option<String>,
    }

    let data: RawPinCheck = send_json(request, "consultar o status do PIN")
        .await
        .map_err(|error| error.public_message())?;

    Ok(data.auth_token)
}

pub async fn get_plex_servers(auth_token: &str) -> Result<Vec<PlexServerResource>, String> {
    let servers = discover_plex_servers(http_client(), PLEX_RESOURCES_URL, auth_token)
        .await
        .map_err(|error| error.public_message())?;

    Ok(servers
        .into_iter()
        .map(into_public_server_resource)
        .collect())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexArtistResult {
    pub rating_key: String,
    pub name: String,
    pub thumb: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread::{self, JoinHandle};

    const MIXED_ORDER_REMOTE_FIRST: &str =
        include_str!("../tests/fixtures/plex_resources_remote_first.json");
    const MIXED_ORDER_LOCAL_FIRST: &str =
        include_str!("../tests/fixtures/plex_resources_local_first.json");

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
        let client = reqwest::Client::builder().timeout(timeout).build().unwrap();
        let url = format!("{base_url}/fixture?X-Plex-Token=TEST_SECRET_TOKEN");
        let result = tauri::async_runtime::block_on(send_json(
            client.get(url),
            "consultar o fixture",
        ));
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
                            let response = handler(path);
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

    fn unreachable_base_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
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
            plex_url: base_url.to_string(),
            plex_token: "TEST_ACCOUNT_TOKEN".to_string(),
            plex_server_id: machine_identifier.map(str::to_string),
            plex_server_name: machine_identifier.map(|_| "Selected Server".to_string()),
            ..AppConfig::default()
        }
    }

    fn test_manager(config: &AppConfig, resources_url: String) -> PlexConnectionManager {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(250))
            .build()
            .unwrap();
        let probe_http = reqwest::Client::builder()
            .timeout(Duration::from_millis(80))
            .build()
            .unwrap();
        PlexConnectionManager::with_clients(config, http, probe_http, resources_url)
    }

    fn plex_client_for_test(content_status: &'static str) -> (PlexClient, MockServer) {
        let content_requests = Arc::new(AtomicUsize::new(0));
        let server = pms_server("fixture-machine-id", content_status, content_requests);
        let config = manager_config(&server.base_url, Some("fixture-machine-id"));
        let manager = test_manager(&config, unreachable_base_url());
        (
            PlexClient {
                connection_manager: Arc::new(manager),
                playback_mode: "http".to_string(),
                path_mappings: HashMap::new(),
            },
            server,
        )
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
        let error = media_container(&serde_json::json!({"unexpected": true}), "testar protocolo")
            .unwrap_err();

        assert_eq!(error.kind, PlexErrorKind::InvalidResponse);
    }

    #[test]
    fn collections_do_not_turn_http_404_into_an_empty_list() {
        let (client, _server) = plex_client_for_test("404 Not Found");

        let error = tauri::async_runtime::block_on(client.get_collections("1")).unwrap_err();

        assert!(error.contains("404"));
        assert!(!error.contains("TEST_SECRET_TOKEN"));
        assert!(!error.contains("X-Plex-Token"));
    }

    #[test]
    fn top_tracks_does_not_fall_back_after_http_401() {
        let (client, _server) = plex_client_for_test("401 Unauthorized");

        let error =
            tauri::async_runtime::block_on(client.get_artist_top_tracks("artist-1")).unwrap_err();

        assert!(error.contains("401"));
        assert!(!error.contains("TEST_SECRET_TOKEN"));
        assert!(!error.contains("X-Plex-Token"));
    }

    #[test]
    fn resolver_uses_working_lan_route() {
        let requests = Arc::new(AtomicUsize::new(0));
        let lan = pms_server("selected-server", "200 OK", requests);
        let config = manager_config(&lan.base_url, Some("selected-server"));
        let manager = test_manager(&config, unreachable_base_url());

        let resolved = tauri::async_runtime::block_on(manager.resolve()).unwrap();

        assert_eq!(resolved.base_url, lan.base_url);
        assert_eq!(resolved.token, "TEST_ACCOUNT_TOKEN");
    }

    #[test]
    fn working_legacy_route_learns_identity_without_guessing_a_server() {
        let requests = Arc::new(AtomicUsize::new(0));
        let legacy = pms_server("legacy-server", "200 OK", requests);
        let config = manager_config(&legacy.base_url, None);
        let manager = test_manager(&config, unreachable_base_url());

        let resolved = tauri::async_runtime::block_on(manager.resolve()).unwrap();
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

        let initial = tauri::async_runtime::block_on(manager.resolve()).unwrap();
        assert_eq!(initial.base_url, lan.base_url);
        drop(lan);

        let (_, refreshed) = tauri::async_runtime::block_on(
            manager.request_json::<serde_json::Value>("/library/test", "consultar conteúdo"),
        )
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

        let resolved = tauri::async_runtime::block_on(manager.resolve()).unwrap();

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

        let resolved = tauri::async_runtime::block_on(manager.resolve()).unwrap();

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

        let resolved = tauri::async_runtime::block_on(manager.resolve()).unwrap();

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

        let error = tauri::async_runtime::block_on(manager.resolve())
            .err()
            .unwrap();
        let public = error.public_message();

        assert!(error.is_route_unavailable());
        assert!(!public.contains("TEST_ACCOUNT_TOKEN"));
        assert!(!public.contains("X-Plex-Token"));
        assert!(!public.contains("http://"));
    }

    #[test]
    fn unauthorized_known_route_does_not_trigger_discovery() {
        let discovery_requests = Arc::new(AtomicUsize::new(0));
        let discovery_counter = discovery_requests.clone();
        let resources = MockServer::start(move |_| {
            discovery_counter.fetch_add(1, Ordering::SeqCst);
            MockResponse::json("200 OK", serde_json::json!([]))
        });
        let pms = MockServer::start(|path| {
            if path.starts_with("/identity") {
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

        let error = tauri::async_runtime::block_on(manager.resolve())
            .err()
            .unwrap();

        assert_eq!(error.kind, PlexErrorKind::Unauthorized);
        assert_eq!(discovery_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn content_404_does_not_invalidate_resolved_connection() {
        let content_requests = Arc::new(AtomicUsize::new(0));
        let pms = pms_server(
            "selected-server",
            "404 Not Found",
            content_requests.clone(),
        );
        let config = manager_config(&pms.base_url, Some("selected-server"));
        let manager = test_manager(&config, unreachable_base_url());

        let error = tauri::async_runtime::block_on(
            manager.request_json::<serde_json::Value>("/missing", "consultar conteúdo"),
        )
        .err()
        .unwrap();
        let snapshot = manager.snapshot().unwrap();

        assert_eq!(error.kind, PlexErrorKind::NotFound);
        assert_eq!(snapshot.current_base_url.as_deref(), Some(pms.base_url.as_str()));
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
        let worker = thread::spawn(move || tauri::async_runtime::block_on(worker_manager.resolve()));

        for _ in 0..100 {
            if identity_started.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert!(identity_started.load(Ordering::SeqCst));
        let updated = manager_config("http://new.invalid", Some("server-b"));
        manager.update_config(&updated);

        let error = worker
            .join()
            .unwrap()
            .err()
            .unwrap();
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

        let resolved = tauri::async_runtime::block_on(manager.resolve()).unwrap();

        assert_eq!(resolved.base_url, right.base_url);
        assert_eq!(wrong_requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn legacy_route_failure_without_identity_requires_server_reselection() {
        let legacy = unreachable_base_url();
        let config = manager_config(&legacy, None);
        let manager = test_manager(&config, unreachable_base_url());

        let error = tauri::async_runtime::block_on(manager.resolve())
            .err()
            .unwrap();

        assert_eq!(error.kind, PlexErrorKind::ServerSelectionRequired);
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexSearchResults {
    pub artists: Vec<PlexArtistResult>,
    pub albums: Vec<PlexAlbum>,
    pub tracks: Vec<PlexTrack>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexLibrary {
    pub key: String,
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexAlbum {
    pub rating_key: String,
    pub title: String,
    pub artist: String,
    pub artist_rating_key: Option<String>,
    pub year: Option<u32>,
    pub thumb: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexCollection {
    pub rating_key: String,
    pub title: String,
    pub child_count: u32,
    pub thumb: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexTrack {
    pub rating_key: String,
    pub title: String,
    pub album_title: Option<String>,
    pub thumb: Option<String>,
    pub track_index: u32,
    pub duration_ms: u64,
    pub play_uri: String,
}

#[derive(Clone)]
pub struct PlexClient {
    connection_manager: Arc<PlexConnectionManager>,
    playback_mode: String,
    path_mappings: HashMap<String, String>,
}

impl PlexClient {
    pub async fn search(&self, query: &str, section_key: Option<&str>) -> Result<PlexSearchResults, String> {
        let mut path = format!("/hubs/search?query={}&limit=12", urlencoding::encode(query));

        if let Some(sec) = section_key {
            path.push_str(&format!("&sectionId={}", sec));
        }

        let operation = "pesquisar a biblioteca";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;

        let mut artists = Vec::new();
        let mut albums = Vec::new();
        let mut tracks = Vec::new();

        if let Some(hubs) = container.get("Hub").and_then(serde_json::Value::as_array) {
            for hub in hubs {
                let hub_type = hub["type"].as_str().unwrap_or("");
                if let Some(meta) = hub["Metadata"].as_array() {
                    for item in meta {
                        match hub_type {
                            "artist" => {
                                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                                let name = item["title"].as_str().unwrap_or("").to_string();
                                let thumb = item["thumb"]
                                    .as_str()
                                    .map(|thumb| Self::get_thumb_url(&connection, thumb));
                                artists.push(PlexArtistResult { rating_key, name, thumb });
                            }
                            "album" => {
                                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                                let artist = item["parentTitle"].as_str().unwrap_or("Vários Artistas").to_string();
                                let artist_rating_key = item["parentRatingKey"].as_str().map(|s| s.to_string());
                                let year = item["year"].as_u64().map(|y| y as u32);
                                let thumb = item["thumb"]
                                    .as_str()
                                    .map(|thumb| Self::get_thumb_url(&connection, thumb));

                                albums.push(PlexAlbum {
                                    rating_key,
                                    title,
                                    artist,
                                    artist_rating_key,
                                    year,
                                    thumb,
                                });
                            }
                            "track" => {
                                if let Some(t) = self.parse_track_item(&connection, item) {
                                    tracks.push(t);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(PlexSearchResults { artists, albums, tracks })
    }
    pub fn from_config(cfg: &AppConfig) -> Self {
        let mut path_mappings = HashMap::new();
        if cfg.playback_mode == "local" && !cfg.remote_share_path.is_empty() && !cfg.local_mount_path.is_empty() {
            path_mappings.insert(
                cfg.remote_share_path.clone(),
                cfg.local_mount_path.clone(),
            );
        }
        Self {
            connection_manager: Arc::new(PlexConnectionManager::from_config(cfg)),
            playback_mode: cfg.playback_mode.clone(),
            path_mappings,
        }
    }

    pub fn update_config(&mut self, cfg: &AppConfig) {
        self.connection_manager.update_config(cfg);
        self.playback_mode = cfg.playback_mode.clone();
        self.path_mappings.clear();
        if self.playback_mode == "local" && !cfg.remote_share_path.is_empty() && !cfg.local_mount_path.is_empty() {
            self.path_mappings.insert(
                cfg.remote_share_path.clone(),
                cfg.local_mount_path.clone(),
            );
        }
    }

    fn get_thumb_url(connection: &ResolvedPlexConnection, thumb_path: &str) -> String {
        format!(
            "{}{}?X-Plex-Token={}",
            connection.base_url, thumb_path, connection.token
        )
    }

    /// Helper reutilizável para converter itens brutos do JSON do Plex em PlexTrack
    fn parse_track_item(
        &self,
        connection: &ResolvedPlexConnection,
        item: &serde_json::Value,
    ) -> Option<PlexTrack> {
        let rating_key = item["ratingKey"].as_str()?.to_string();
        let title = item["title"].as_str().unwrap_or("Faixa").to_string();
        let album_title = item["parentTitle"].as_str().map(|s| s.to_string());
        let track_index = item["index"].as_u64().unwrap_or(1) as u32;
        let duration_ms = item["duration"].as_u64().unwrap_or(0);

        let thumb = item["thumb"]
            .as_str()
            .or_else(|| item["parentThumb"].as_str())
            .map(|thumb| Self::get_thumb_url(connection, thumb));

        let media = item["Media"].as_array()?.first()?;
        let part = media["Part"].as_array()?.first()?;
        let file_path = part["file"].as_str().unwrap_or("");
        let part_key = part["key"].as_str().unwrap_or("");

        let play_uri = if self.playback_mode == "local" {
            let mut local = None;
            for (remote, mount) in &self.path_mappings {
                if file_path.starts_with(remote) {
                    local = Some(file_path.replacen(remote, mount, 1));
                    break;
                }
            }
            local.unwrap_or_else(|| {
                format!(
                    "{}{}?X-Plex-Token={}",
                    connection.base_url, part_key, connection.token
                )
            })
        } else {
            format!(
                "{}{}?X-Plex-Token={}",
                connection.base_url, part_key, connection.token
            )
        };

        Some(PlexTrack {
            rating_key,
            title,
            album_title,
            thumb,
            track_index,
            duration_ms,
            play_uri,
        })
    }

    pub async fn get_music_libraries(&self) -> Result<Vec<PlexLibrary>, String> {
        let operation = "consultar as bibliotecas";
        let (json, _connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json("/library/sections", operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut libraries = Vec::new();

        if let Some(dirs) = container
            .get("Directory")
            .and_then(serde_json::Value::as_array)
        {
            for dir in dirs {
                if dir["type"].as_str() == Some("artist") {
                    if let (Some(key), Some(title)) = (dir["key"].as_str(), dir["title"].as_str()) {
                        libraries.push(PlexLibrary {
                            key: key.to_string(),
                            title: title.to_string(),
                        });
                    }
                }
            }
        }

        Ok(libraries)
    }

    pub async fn get_albums(&self, section_key: &str, sort_by: &str) -> Result<Vec<PlexAlbum>, String> {
        let sort_param = match sort_by {
            "title" => "titleSort:asc",
            "year" => "originallyAvailableAt:desc",
            _ => "addedAt:desc",
        };

        let path = format!(
            "/library/sections/{}/all?type=9&sort={}",
            section_key, sort_param
        );

        let operation = "consultar os álbuns";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut albums = Vec::new();

        if let Some(meta) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                let artist = item["parentTitle"].as_str().unwrap_or("Vários Artistas").to_string();
                let artist_rating_key = item["parentRatingKey"].as_str().map(|s| s.to_string());
                let year = item["year"].as_u64().map(|y| y as u32);
                let thumb = item["thumb"]
                    .as_str()
                    .map(|thumb| Self::get_thumb_url(&connection, thumb));

                albums.push(PlexAlbum {
                    rating_key,
                    title,
                    artist,
                    artist_rating_key,
                    year,
                    thumb,
                });
            }
        }

        Ok(albums)
    }

    pub async fn get_collections(&self, section_key: &str) -> Result<Vec<PlexCollection>, String> {
        let path = format!("/library/sections/{}/collections", section_key);

        let operation = "consultar as coleções";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut collections = Vec::new();

        if let Some(meta) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("").to_string();
                let child_count = item["childCount"].as_u64().unwrap_or(0) as u32;
                let thumb = item["thumb"]
                    .as_str()
                    .map(|thumb| Self::get_thumb_url(&connection, thumb));

                collections.push(PlexCollection {
                    rating_key,
                    title,
                    child_count,
                    thumb,
                });
            }
        }

        Ok(collections)
    }

    pub async fn get_collection_albums(&self, collection_rating_key: &str) -> Result<Vec<PlexAlbum>, String> {
        let path = format!("/library/collections/{}/children", collection_rating_key);

        let operation = "consultar os álbuns da coleção";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut albums = Vec::new();

        if let Some(meta) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                let artist = item["parentTitle"].as_str().unwrap_or("Vários Artistas").to_string();
                let artist_rating_key = item["parentRatingKey"].as_str().map(|s| s.to_string());
                let year = item["year"].as_u64().map(|y| y as u32);
                let thumb = item["thumb"]
                    .as_str()
                    .map(|thumb| Self::get_thumb_url(&connection, thumb));

                albums.push(PlexAlbum {
                    rating_key,
                    title,
                    artist,
                    artist_rating_key,
                    year,
                    thumb,
                });
            }
        }

        Ok(albums)
    }

    pub async fn get_artist_albums(&self, artist_rating_key: &str) -> Result<Vec<PlexAlbum>, String> {
        let path = format!("/library/metadata/{}/children", artist_rating_key);

        let operation = "consultar os álbuns do artista";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut albums = Vec::new();

        if let Some(meta) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                let artist = item["parentTitle"].as_str().unwrap_or("").to_string();
                let year = item["year"].as_u64().map(|y| y as u32);
                let thumb = item["thumb"]
                    .as_str()
                    .map(|thumb| Self::get_thumb_url(&connection, thumb));

                albums.push(PlexAlbum {
                    rating_key,
                    title,
                    artist,
                    artist_rating_key: Some(artist_rating_key.to_string()),
                    year,
                    thumb,
                });
            }
        }

        Ok(albums)
    }

    pub async fn get_artist_top_tracks(&self, artist_rating_key: &str) -> Result<Vec<PlexTrack>, String> {
        // Tentativa 1: Endpoint direto nativo do Plex para faixas populares
        let path_top = format!("/library/metadata/{}/topTracks", artist_rating_key);
        let top_operation = "consultar as faixas populares do artista";
        match self
            .connection_manager
            .request_json::<serde_json::Value>(&path_top, top_operation)
            .await
        {
            Ok((json, connection)) => {
                let container = media_container(&json, top_operation)
                    .map_err(|error| error.public_message())?;
                if let Some(meta) = container
                    .get("Metadata")
                    .and_then(serde_json::Value::as_array)
                {
                    let tracks: Vec<PlexTrack> = meta
                        .iter()
                        .filter_map(|track| self.parse_track_item(&connection, track))
                        .collect();
                    if !tracks.is_empty() {
                        return Ok(tracks);
                    }
                }
            }
            Err(error) if error.kind == PlexErrorKind::NotFound => {}
            Err(error) => return Err(error.public_message()),
        }

        // Tentativa 2: Hubs do artista (usado pelo Plex Web para renderizar o card Populares)
        let path_hub = format!("/hubs/metadata/{}", artist_rating_key);
        let hub_operation = "consultar os hubs do artista";
        match self
            .connection_manager
            .request_json::<serde_json::Value>(&path_hub, hub_operation)
            .await
        {
            Ok((json, connection)) => {
                let container = media_container(&json, hub_operation)
                    .map_err(|error| error.public_message())?;
                if let Some(hubs) = container
                    .get("Hub")
                    .and_then(serde_json::Value::as_array)
                {
                    for hub in hubs {
                        let hub_id = hub["hubIdentifier"].as_str().unwrap_or("");
                        let hub_type = hub["type"].as_str().unwrap_or("");
                        if hub_id == "artist.topTracks" || hub_type == "track" {
                            if let Some(meta) = hub["Metadata"].as_array() {
                                let tracks: Vec<PlexTrack> = meta
                                    .iter()
                                    .filter_map(|track| self.parse_track_item(&connection, track))
                                    .collect();
                                if !tracks.is_empty() {
                                    return Ok(tracks);
                                }
                            }
                        }
                    }
                }
            }
            Err(error) if error.kind == PlexErrorKind::NotFound => {}
            Err(error) => return Err(error.public_message()),
        }

        // Tentativa 3: allLeaves com ordenação manual em memória por popularidade
        let path_all = format!("/library/metadata/{}/allLeaves", artist_rating_key);

        let operation = "consultar todas as faixas do artista";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path_all, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut scored_tracks = Vec::new();

        if let Some(meta) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        {
            for item in meta {
                let rating_count = item["ratingCount"].as_u64().unwrap_or(0);
                let view_count = item["viewCount"].as_u64().unwrap_or(0);
                if let Some(track) = self.parse_track_item(&connection, item) {
                    scored_tracks.push((rating_count, view_count, track));
                }
            }
        }

        // Ordena por popularidade global do Last.fm e desempata pelo número de reproduções locais
        scored_tracks.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

        let tracks = scored_tracks.into_iter().map(|(_, _, t)| t).collect();
        Ok(tracks)
    }

    pub async fn get_album_tracks(&self, album_rating_key: &str) -> Result<Vec<PlexTrack>, String> {
        let path = format!("/library/metadata/{}/children", album_rating_key);

        let operation = "consultar as faixas do álbum";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut tracks = Vec::new();

        if let Some(meta) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        {
            for track in meta {
                if let Some(t) = self.parse_track_item(&connection, track) {
                    tracks.push(t);
                }
            }
        }

        Ok(tracks)
    }
}
