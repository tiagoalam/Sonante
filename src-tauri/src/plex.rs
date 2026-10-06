use crate::audio::MediaLocator;
use crate::config::AppConfig;
use reqwest::{header::CONTENT_TYPE, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const PLEX_CLIENT_ID: &str = "sonante-audio-player";
const PLEX_PRODUCT_NAME: &str = "Sonante";
const PLEX_VERSION: &str = "0.4.5";
const PLEX_RESOURCES_URL: &str =
    "https://plex.tv/api/v2/resources?includeHttps=1&includeRelay=1";
const PLEX_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PLEX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const PLEX_IMAGE_TRANSCODE_SIZE: u16 = 600;
const SUPPORTED_IMAGE_CONTENT_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/gif",
];

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
    InvalidImageReference,
    UnsupportedImageType,
    ImageTooLarge,
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
            PlexErrorKind::InvalidImageReference => {
                "A referência da imagem Plex é inválida.".to_string()
            }
            PlexErrorKind::UnsupportedImageType => {
                "O Plex retornou um formato de imagem não suportado.".to_string()
            }
            PlexErrorKind::ImageTooLarge => {
                "A imagem Plex excede o limite permitido.".to_string()
            }
        }
    }

    fn is_route_unavailable(&self) -> bool {
        matches!(self.kind, PlexErrorKind::Timeout | PlexErrorKind::Transport)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PlexImageRef {
    pub server_id: String,
    pub path: String,
}

impl PlexImageRef {
    fn from_connection(connection: &ResolvedPlexConnection, path: &str) -> Option<Self> {
        validate_plex_image_path(path).ok()?;
        Some(Self {
            server_id: connection.server_id.clone(),
            path: path.to_string(),
        })
    }

    pub(crate) fn is_valid(&self) -> bool {
        validate_plex_server_id(&self.server_id).is_ok()
            && validate_plex_image_path(&self.path).is_ok()
    }
}

fn invalid_image_reference() -> PlexError {
    PlexError {
        operation: "buscar a imagem",
        kind: PlexErrorKind::InvalidImageReference,
    }
}

fn validate_plex_server_id(server_id: &str) -> Result<(), PlexError> {
    if server_id.is_empty()
        || server_id.len() > 256
        || server_id.trim() != server_id
        || !server_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return Err(invalid_image_reference());
    }
    Ok(())
}

fn validate_plex_image_path(path: &str) -> Result<(), PlexError> {
    let lower = path.to_ascii_lowercase();
    let decoded_lower = urlencoding::decode(path)
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if path.is_empty()
        || path.len() > 4096
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains(['\\', '\0', '\r', '\n', '#'])
        || path.chars().any(char::is_whitespace)
        || lower.contains("://")
        || lower.contains("x-plex-token")
        || decoded_lower.contains("x-plex-token")
        || lower
            .split('?')
            .next()
            .is_some_and(|value| value.eq_ignore_ascii_case("/photo/:/transcode"))
        || decoded_lower
            .split('?')
            .next()
            .is_some_and(|value| value.eq_ignore_ascii_case("/photo/:/transcode"))
    {
        return Err(invalid_image_reference());
    }
    Ok(())
}

fn plex_image_transcode_path(path: &str) -> String {
    let encoded_path = urlencoding::encode(path);
    format!(
        "/photo/:/transcode?width={0}&height={0}&minSize=1&upscale=1&url={1}",
        PLEX_IMAGE_TRANSCODE_SIZE, encoded_path
    )
}

pub(crate) fn legacy_plex_image_ref(
    value: &str,
    server_id: Option<&str>,
) -> Option<PlexImageRef> {
    let server_id = server_id?.trim();
    validate_plex_server_id(server_id).ok()?;
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url
            .query_pairs()
            .any(|(name, _)| name.eq_ignore_ascii_case("X-Plex-Token"))
    {
        return None;
    }
    let path = url.path();
    validate_plex_image_path(path).ok()?;
    Some(PlexImageRef {
        server_id: server_id.to_string(),
        path: path.to_string(),
    })
}

pub(crate) fn contains_plex_token(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("x-plex-token")
        || urlencoding::decode(value)
            .map(|decoded| decoded.to_ascii_lowercase().contains("x-plex-token"))
            .unwrap_or(false)
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
    server_id: String,
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
            let server_id = snapshot
                .identity
                .as_ref()
                .map(|identity| identity.machine_identifier.clone())
                .ok_or(PlexError {
                    operation: "resolver o servidor",
                    kind: PlexErrorKind::ServerSelectionRequired,
                })?;
            return Ok(ResolvedPlexConnection {
                server_id,
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
        let server_id = identity.machine_identifier.clone();
        state.identity = Some(identity);
        state.current_base_url = Some(base_url.clone());
        state.resolved_token = Some(token.clone());
        Ok(ResolvedPlexConnection {
            server_id,
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

    async fn request_image(&self, image: &PlexImageRef) -> Result<Vec<u8>, PlexError> {
        validate_plex_server_id(&image.server_id)?;
        validate_plex_image_path(&image.path)?;
        let snapshot = self.snapshot()?;
        if snapshot
            .identity
            .as_ref()
            .map(|identity| identity.machine_identifier.as_str())
            != Some(image.server_id.as_str())
        {
            return Err(invalid_image_reference());
        }

        let connection = self.resolve().await?;
        let first = self.send_authenticated_image(&connection, &image.path).await;
        let (connection, original_result) = match first {
            result @ Ok(_) => (connection, result),
            Err(error) if error.is_route_unavailable() => {
                let refreshed = self.refresh(connection.generation).await?;
                if refreshed.server_id != image.server_id {
                    return Err(invalid_image_reference());
                }
                let result = self.send_authenticated_image(&refreshed, &image.path).await;
                (refreshed, result)
            }
            result @ Err(_) => (connection, result),
        };

        match original_result {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind == PlexErrorKind::ImageTooLarge => {
                let transcode_path = plex_image_transcode_path(&image.path);
                self.send_authenticated_image(&connection, &transcode_path)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn send_authenticated_image(
        &self,
        connection: &ResolvedPlexConnection,
        path: &str,
    ) -> Result<Vec<u8>, PlexError> {
        let operation = "buscar a imagem";
        let mut response = self
            .http
            .get(format!("{}{}", connection.base_url, path))
            .header("X-Plex-Token", &connection.token)
            .header("Accept", "image/jpeg,image/png,image/webp,image/gif")
            .send()
            .await
            .map_err(|error| PlexError::from_transport(operation, &error))?;
        if !response.status().is_success() {
            return Err(PlexError::from_status(operation, response.status()));
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if !SUPPORTED_IMAGE_CONTENT_TYPES.contains(&content_type.as_str()) {
            return Err(PlexError {
                operation,
                kind: PlexErrorKind::UnsupportedImageType,
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_PLEX_IMAGE_BYTES as u64)
        {
            return Err(PlexError {
                operation,
                kind: PlexErrorKind::ImageTooLarge,
            });
        }

        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| PlexError::from_transport(operation, &error))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_PLEX_IMAGE_BYTES {
                return Err(PlexError {
                    operation,
                    kind: PlexErrorKind::ImageTooLarge,
                });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
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

fn is_valid_plex_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains(['\0', '\r', '\n'])
        && !path.contains("://")
}

fn json_boolean(value: Option<&serde_json::Value>) -> Option<bool> {
    match value? {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::Number(value) => value.as_u64().and_then(|value| match value {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }),
        _ => None,
    }
}

fn authenticated_plex_url(base_url: &str, path_and_query: &str, token: &str) -> String {
    let separator = if path_and_query.contains('?') { '&' } else { '?' };
    format!(
        "{}{}{}X-Plex-Token={}",
        base_url.trim_end_matches('/'),
        path_and_query,
        separator,
        token
    )
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
    pub thumb: Option<PlexImageRef>,
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

    fn availability_client_for_test(
        content_status: &'static str,
        content_body: serde_json::Value,
    ) -> (PlexClient, MockServer) {
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
                MockResponse::json(content_status, content_body.clone())
            }
        });
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

    fn media_client_for_test(config: &AppConfig) -> PlexClient {
        let mut path_mappings = HashMap::new();
        if config.playback_mode == "local"
            && !config.remote_share_path.is_empty()
            && !config.local_mount_path.is_empty()
        {
            path_mappings.insert(
                config.remote_share_path.clone(),
                config.local_mount_path.clone(),
            );
        }
        PlexClient {
            connection_manager: Arc::new(test_manager(
                config,
                "http://127.0.0.1:9/api/v2/resources".to_string(),
            )),
            playback_mode: config.playback_mode.clone(),
            path_mappings,
        }
    }

    fn plex_locator(
        server_id: &str,
        part_key: &str,
        rating_key: Option<&str>,
        file_path: Option<&str>,
    ) -> MediaLocator {
        MediaLocator::Plex {
            server_id: server_id.to_string(),
            part_key: part_key.to_string(),
            rating_key: rating_key.map(str::to_string),
            file_path: file_path.map(str::to_string),
        }
    }

    #[test]
    fn parsed_plex_track_contains_stable_reference_instead_of_authenticated_uri() {
        let mut config = manager_config("http://route.invalid", Some("server-1"));
        config.playback_mode = "local".to_string();
        let client = media_client_for_test(&config);
        let connection = ResolvedPlexConnection {
            server_id: "server-1".to_string(),
            base_url: "http://route.invalid".to_string(),
            token: "SECRET".to_string(),
            generation: 0,
        };
        let item = serde_json::json!({
            "ratingKey": "track-1",
            "title": "Faixa",
            "Media": [{
                "Part": [{
                    "key": "/library/parts/1/file.flac",
                    "file": "/srv/music/file.flac"
                }]
            }]
        });

        let track = client.parse_track_item(&connection, &item).unwrap();

        assert_eq!(
            track.media_locator,
            plex_locator(
                "server-1",
                "/library/parts/1/file.flac",
                Some("track-1"),
                Some("/srv/music/file.flac")
            )
        );
        let serialized = serde_json::to_string(&track).unwrap();
        assert!(!serialized.contains("SECRET"));
        assert!(!serialized.contains("route.invalid"));
    }

    #[test]
    fn media_availability_uses_metadata_for_available_and_confirmed_missing() {
        let existing_body = serde_json::json!({
            "MediaContainer": {
                "Metadata": [{
                    "ratingKey": "track-1",
                    "parentThumb": "/library/metadata/album-1/thumb/1",
                    "Media": [{"Part": [{
                        "key": "/library/parts/1/file.flac",
                        "exists": true
                    }]}]
                }]
            }
        });
        let (existing_client, _existing_server) =
            availability_client_for_test("200 OK", existing_body);
        let existing = plex_locator(
            "fixture-machine-id",
            "/library/parts/1/file.flac",
            Some("track-1"),
            None,
        );
        let (availability, artwork) = tauri::async_runtime::block_on(
            existing_client.media_availability_with_artwork(&existing),
        );
        assert_eq!(availability, PlexMediaAvailability::Available);
        assert_eq!(artwork.unwrap().path, "/library/metadata/album-1/thumb/1");

        let (missing_client, _missing_server) = availability_client_for_test(
            "404 Not Found",
            serde_json::json!({"MediaContainer": {}}),
        );
        let missing = plex_locator(
            "fixture-machine-id",
            "/library/parts/404/file.flac",
            Some("track-404"),
            None,
        );
        assert_eq!(
            tauri::async_runtime::block_on(missing_client.media_availability(&missing)),
            PlexMediaAvailability::Missing
        );
    }

    #[test]
    fn plex_server_and_network_errors_are_unavailable_not_missing() {
        let locator = plex_locator(
            "fixture-machine-id",
            "/library/parts/1/file.flac",
            Some("track-1"),
            None,
        );
        let (server_error_client, _server) = availability_client_for_test(
            "500 Internal Server Error",
            serde_json::json!({"MediaContainer": {}}),
        );
        assert!(matches!(
            tauri::async_runtime::block_on(server_error_client.media_availability(&locator)),
            PlexMediaAvailability::Unavailable(_)
        ));

        let config = manager_config(&unreachable_base_url(), Some("fixture-machine-id"));
        let network_client = media_client_for_test(&config);
        assert!(matches!(
            tauri::async_runtime::block_on(network_client.media_availability(&locator)),
            PlexMediaAvailability::Unavailable(_)
        ));
    }

    #[test]
    fn unsupported_or_unexpected_plex_responses_are_unavailable() {
        let locator = plex_locator(
            "fixture-machine-id",
            "/library/parts/1/file.flac",
            Some("track-1"),
            None,
        );
        let (method_client, _method_server) = availability_client_for_test(
            "405 Method Not Allowed",
            serde_json::json!({"MediaContainer": {}}),
        );
        assert!(matches!(
            tauri::async_runtime::block_on(method_client.media_availability(&locator)),
            PlexMediaAvailability::Unavailable(_)
        ));

        let (unexpected_client, _unexpected_server) = availability_client_for_test(
            "200 OK",
            serde_json::json!({
                "MediaContainer": {
                    "size": 1,
                    "Metadata": [{"ratingKey": "different-track"}]
                }
            }),
        );
        assert!(matches!(
            tauri::async_runtime::block_on(unexpected_client.media_availability(&locator)),
            PlexMediaAvailability::Unavailable(_)
        ));
    }

    #[test]
    fn same_media_reference_uses_current_route_and_current_token() {
        let mut config = manager_config("http://lan-route", Some("server-1"));
        config.plex_token = "TOKEN_A".to_string();
        let manager = Arc::new(test_manager(
            &config,
            "http://127.0.0.1:9/api/v2/resources".to_string(),
        ));
        manager
            .publish_connection(
                0,
                PlexServerIdentity {
                    machine_identifier: "server-1".to_string(),
                    display_name: "Server".to_string(),
                },
                "http://lan-route".to_string(),
                "TOKEN_A".to_string(),
            )
            .unwrap();
        let mut client = PlexClient {
            connection_manager: manager.clone(),
            playback_mode: "http".to_string(),
            path_mappings: HashMap::new(),
        };
        let locator = plex_locator("server-1", "/library/parts/1/file.flac", None, None);

        let first = tauri::async_runtime::block_on(client.resolve_media_locator(&locator)).unwrap();

        config.plex_url = "https://remote-route".to_string();
        config.plex_token = "TOKEN_B".to_string();
        client.update_config(&config);
        manager
            .publish_connection(
                1,
                PlexServerIdentity {
                    machine_identifier: "server-1".to_string(),
                    display_name: "Server".to_string(),
                },
                "https://remote-route".to_string(),
                "TOKEN_B".to_string(),
            )
            .unwrap();
        let second =
            tauri::async_runtime::block_on(client.resolve_media_locator(&locator)).unwrap();

        assert!(first.starts_with("http://lan-route"));
        assert!(first.ends_with("X-Plex-Token=TOKEN_A"));
        assert!(second.starts_with("https://remote-route"));
        assert!(second.ends_with("X-Plex-Token=TOKEN_B"));
        assert!(!second.contains("TOKEN_A"));
    }

    #[test]
    fn media_reference_for_different_server_is_rejected_without_sensitive_details() {
        let config = manager_config("http://selected-route.invalid", Some("server-1"));
        let client = media_client_for_test(&config);
        let locator = plex_locator("server-2", "/library/parts/1/file.flac", None, None);

        let error = tauri::async_runtime::block_on(client.resolve_media_locator(&locator))
            .unwrap_err();

        assert!(!error.contains("TEST_ACCOUNT_TOKEN"));
        assert!(!error.contains("selected-route.invalid"));
        assert!(!error.contains("X-Plex-Token"));
    }

    #[test]
    fn legacy_local_playback_mode_maps_file_path_without_forcing_http() {
        let mut config = manager_config("http://unreachable.invalid", Some("server-1"));
        config.playback_mode = "local".to_string();
        config.remote_share_path = "/srv/music".to_string();
        config.local_mount_path = "/mnt/plex".to_string();
        let client = media_client_for_test(&config);
        let locator = plex_locator(
            "server-1",
            "/library/parts/1/file.flac",
            None,
            Some("/srv/music/album/file.flac"),
        );

        let uri = tauri::async_runtime::block_on(client.resolve_media_locator(&locator)).unwrap();

        assert_eq!(uri, "/mnt/plex/album/file.flac");
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

    fn image_client(
        status_line: &'static str,
        content_type: &'static str,
        body: String,
    ) -> (PlexClient, MockServer) {
        image_client_with_handler(move |_| MockResponse {
            status_line,
            content_type,
            body: body.clone(),
            delay: Duration::ZERO,
        })
    }

    fn image_client_with_handler<F>(handler: F) -> (PlexClient, MockServer)
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
        let image = PlexImageRef::from_connection(
            &connection,
            "/library/metadata/42/thumb/1?width=300",
        )
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
        let bytes = tauri::async_runtime::block_on(
            client.get_image(&image_ref("/library/metadata/42/thumb/1")),
        )
        .unwrap();
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

        let bytes = tauri::async_runtime::block_on(client.get_image(&image_ref(original_path)))
            .unwrap();
        let requested_path = transcode_path.lock().unwrap().clone().unwrap();

        assert_eq!(bytes, b"transcoded-image");
        assert_eq!(
            requested_path,
            "/photo/:/transcode?width=600&height=600&minSize=1&upscale=1&url=%2Flibrary%2Fmetadata%2F42%2Fthumb%2F1%3Fquality%3Dhigh%26crop%3D1"
        );
        assert!(!requested_path
            .to_ascii_lowercase()
            .contains("x-plex-token"));
        assert!(!requested_path.contains("TEST_ACCOUNT_TOKEN"));
    }

    #[test]
    fn oversized_transcode_response_is_still_rejected() {
        let oversized = "x".repeat(MAX_PLEX_IMAGE_BYTES + 1);
        let (client, _server) = image_client("200 OK", "image/jpeg", oversized);

        let error = tauri::async_runtime::block_on(
            client.get_image(&image_ref("/library/metadata/42/thumb/1")),
        )
        .unwrap_err();

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

        let error = tauri::async_runtime::block_on(
            client.get_image(&image_ref("/library/metadata/42/thumb/1")),
        )
        .unwrap_err();

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
            let error = tauri::async_runtime::block_on(client.get_image(&image)).unwrap_err();
            assert_eq!(error, "A referência da imagem Plex é inválida.");
            assert!(!error.contains("SECRET"));
            assert!(!error.contains("outside.invalid"));
        }
    }

    #[test]
    fn image_http_errors_are_sanitized() {
        for status in ["401 Unauthorized", "404 Not Found", "500 Internal Server Error"] {
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
            let error = tauri::async_runtime::block_on(
                client.get_image(&image_ref("/library/metadata/42/thumb/1")),
            )
            .unwrap_err();
            assert!(!error.contains("SECRET"));
            assert!(!error.contains("X-Plex-Token"));
            assert!(!error.contains("127.0.0.1"));
            assert_eq!(transcode_requests.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn image_type_and_size_are_limited() {
        let (client, _server) = image_client("200 OK", "text/html", "not an image".to_string());
        let error = tauri::async_runtime::block_on(
            client.get_image(&image_ref("/library/metadata/42/thumb/1")),
        )
        .unwrap_err();
        assert_eq!(error, "O Plex retornou um formato de imagem não suportado.");
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
    pub thumb: Option<PlexImageRef>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexCollection {
    pub server_id: String,
    pub rating_key: String,
    pub title: String,
    pub child_count: u32,
    pub thumb: Option<PlexImageRef>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexTrack {
    pub rating_key: String,
    pub title: String,
    pub album_title: Option<String>,
    pub artist: Option<String>,
    pub thumb: Option<PlexImageRef>,
    pub track_index: u32,
    pub duration_ms: u64,
    pub media_locator: MediaLocator,
}

#[derive(Clone)]
pub struct PlexClient {
    connection_manager: Arc<PlexConnectionManager>,
    playback_mode: String,
    path_mappings: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlexMediaAvailability {
    Available,
    Missing,
    Unavailable(String),
}

impl PlexClient {
    pub async fn media_availability(&self, locator: &MediaLocator) -> PlexMediaAvailability {
        self.media_availability_with_artwork(locator).await.0
    }

    pub async fn media_availability_with_artwork(
        &self,
        locator: &MediaLocator,
    ) -> (PlexMediaAvailability, Option<PlexImageRef>) {
        let mut artwork = None;
        let availability = self.media_availability_inner(locator, &mut artwork).await;
        (availability, artwork)
    }

    async fn media_availability_inner(
        &self,
        locator: &MediaLocator,
        artwork: &mut Option<PlexImageRef>,
    ) -> PlexMediaAvailability {
        let MediaLocator::Plex {
            server_id,
            part_key,
            rating_key,
            ..
        } = locator
        else {
            return PlexMediaAvailability::Unavailable(
                "A referência informada não é uma faixa Plex.".to_string(),
            );
        };
        if !is_valid_plex_path(part_key) || contains_plex_token(part_key) {
            return PlexMediaAvailability::Unavailable(
                "A referência Plex é inválida ou contém credencial.".to_string(),
            );
        }
        let Some(rating_key) = rating_key.as_deref() else {
            return PlexMediaAvailability::Unavailable(
                "A referência Plex legada não possui identidade de metadata para validação segura."
                    .to_string(),
            );
        };
        if rating_key.trim().is_empty()
            || rating_key.contains(['\0', '\r', '\n', '/', '?', '#'])
        {
            return PlexMediaAvailability::Unavailable(
                "A identidade de metadata Plex é inválida.".to_string(),
            );
        }
        let snapshot = match self.connection_manager.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => return PlexMediaAvailability::Unavailable(error.public_message()),
        };
        let Some(selected_server_id) = snapshot
            .identity
            .as_ref()
            .map(|identity| identity.machine_identifier.as_str())
        else {
            return PlexMediaAvailability::Unavailable(
                "Não foi possível identificar o servidor Plex selecionado.".to_string(),
            );
        };
        if selected_server_id != server_id {
            return PlexMediaAvailability::Unavailable(
                "A referência pertence a outro servidor Plex.".to_string(),
            );
        }

        let operation = "validar a disponibilidade da faixa";
        let path = format!(
            "/library/metadata/{}?checkFileAvailability=1",
            urlencoding::encode(rating_key)
        );
        let (json, connection): (serde_json::Value, _) =
            match self.connection_manager.request_json(&path, operation).await {
                Ok(result) => result,
                Err(error)
                    if error.kind == PlexErrorKind::NotFound && error.operation == operation =>
                {
                    return PlexMediaAvailability::Missing;
                }
                Err(error) => {
                    return PlexMediaAvailability::Unavailable(error.public_message());
                }
            };
        if connection.server_id != *server_id {
            return PlexMediaAvailability::Unavailable(
                "A rota Plex respondeu como outro servidor e foi rejeitada.".to_string(),
            );
        }
        let container = match media_container(&json, operation) {
            Ok(container) => container,
            Err(error) => return PlexMediaAvailability::Unavailable(error.public_message()),
        };
        let Some(metadata) = container
            .get("Metadata")
            .and_then(serde_json::Value::as_array)
        else {
            return PlexMediaAvailability::Unavailable(
                PlexError::invalid_response(operation).public_message(),
            );
        };
        let Some(track) = metadata.iter().find(|item| {
            item.get("ratingKey").and_then(serde_json::Value::as_str) == Some(rating_key)
        }) else {
            return if metadata.is_empty()
                && container
                    .get("size")
                    .and_then(|size| size.as_u64().or_else(|| size.as_str()?.parse().ok()))
                    == Some(0)
            {
                PlexMediaAvailability::Missing
            } else {
                PlexMediaAvailability::Unavailable(
                    "O Plex retornou metadata inesperada para a faixa consultada.".to_string(),
                )
            };
        };
        *artwork = track["thumb"]
            .as_str()
            .or_else(|| track["parentThumb"].as_str())
            .and_then(|path| PlexImageRef::from_connection(&connection, path));
        let Some(media_items) = track
            .get("Media")
            .and_then(serde_json::Value::as_array)
        else {
            return PlexMediaAvailability::Unavailable(
                "O Plex retornou metadata sem a lista de mídias da faixa.".to_string(),
            );
        };
        let mut parts = Vec::new();
        for media in media_items {
            let Some(media_parts) = media.get("Part").and_then(serde_json::Value::as_array) else {
                return PlexMediaAvailability::Unavailable(
                    "O Plex retornou metadata sem a lista de arquivos da faixa.".to_string(),
                );
            };
            parts.extend(media_parts);
        }
        let matching_part = parts.into_iter().find(|part| {
            part.get("key").and_then(serde_json::Value::as_str) == Some(part_key.as_str())
        });
        let Some(part) = matching_part else {
            return PlexMediaAvailability::Missing;
        };
        match json_boolean(part.get("exists")) {
            Some(true) => PlexMediaAvailability::Available,
            Some(false) => PlexMediaAvailability::Missing,
            None => PlexMediaAvailability::Unavailable(
                "O Plex não confirmou a existência do arquivo da faixa.".to_string(),
            ),
        }
    }

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
                                    .and_then(|thumb| PlexImageRef::from_connection(&connection, thumb));
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
                                    .and_then(|thumb| PlexImageRef::from_connection(&connection, thumb));

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

    pub async fn resolve_media_locator(
        &self,
        locator: &MediaLocator,
    ) -> Result<String, String> {
        let MediaLocator::Plex {
            server_id,
            part_key,
            file_path,
            ..
        } = locator
        else {
            return match locator {
                MediaLocator::Local { uri } => Ok(uri.clone()),
                MediaLocator::Plex { .. } => unreachable!(),
            };
        };

        let snapshot = self
            .connection_manager
            .snapshot()
            .map_err(|error| error.public_message())?;
        let selected_server_id = snapshot
            .identity
            .as_ref()
            .map(|identity| identity.machine_identifier.as_str())
            .ok_or_else(|| {
                PlexError {
                    operation: "resolver a faixa",
                    kind: PlexErrorKind::ServerSelectionRequired,
                }
                .public_message()
            })?;
        if selected_server_id != server_id {
            return Err(PlexError {
                operation: "resolver a faixa",
                kind: PlexErrorKind::IdentityMismatch,
            }
            .public_message());
        }

        if self.playback_mode == "local" {
            if let Some(file_path) = file_path {
                for (remote, mount) in &self.path_mappings {
                    if file_path.starts_with(remote) {
                        return Ok(file_path.replacen(remote, mount, 1));
                    }
                }
            }
        }

        if !is_valid_plex_path(part_key) {
            return Err(PlexError::invalid_response("resolver a faixa").public_message());
        }
        let connection = self
            .connection_manager
            .resolve()
            .await
            .map_err(|error| error.public_message())?;
        if connection.server_id != *server_id {
            return Err(PlexError {
                operation: "resolver a faixa",
                kind: PlexErrorKind::IdentityMismatch,
            }
            .public_message());
        }
        Ok(authenticated_plex_url(
            &connection.base_url,
            part_key,
            &connection.token,
        ))
    }

    pub async fn get_image(&self, image: &PlexImageRef) -> Result<Vec<u8>, String> {
        self.connection_manager
            .request_image(image)
            .await
            .map_err(|error| error.public_message())
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
        let artist = item["grandparentTitle"].as_str().map(|s| s.to_string());
        let track_index = item["index"].as_u64().unwrap_or(1) as u32;
        let duration_ms = item["duration"].as_u64().unwrap_or(0);

        let thumb = item["thumb"]
            .as_str()
            .or_else(|| item["parentThumb"].as_str())
            .and_then(|thumb| PlexImageRef::from_connection(connection, thumb));

        let media = item["Media"].as_array()?.first()?;
        let part = media["Part"].as_array()?.first()?;
        let file_path = part["file"].as_str().unwrap_or("");
        let part_key = part["key"].as_str().filter(|key| !key.is_empty())?;

        Some(PlexTrack {
            rating_key: rating_key.clone(),
            title,
            album_title,
            artist,
            thumb,
            track_index,
            duration_ms,
            media_locator: MediaLocator::Plex {
                server_id: connection.server_id.clone(),
                part_key: part_key.to_string(),
                rating_key: Some(rating_key.clone()),
                file_path: (self.playback_mode == "local" && !file_path.is_empty())
                    .then(|| file_path.to_string()),
            },
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
                    .and_then(|thumb| PlexImageRef::from_connection(&connection, thumb));

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
                    .and_then(|thumb| PlexImageRef::from_connection(&connection, thumb));

                collections.push(PlexCollection {
                    server_id: connection.server_id.clone(),
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
                    .and_then(|thumb| PlexImageRef::from_connection(&connection, thumb));

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
                    .and_then(|thumb| PlexImageRef::from_connection(&connection, thumb));

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
