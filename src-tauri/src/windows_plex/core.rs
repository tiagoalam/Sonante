//! Original Plex connection/discovery/identity and image client with bounded Windows I/O.
use super::Settings as AppConfig;
use reqwest::{header::CONTENT_TYPE, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const PLEX_CLIENT_ID: &str = "sonante-audio-player";
const PLEX_PRODUCT_NAME: &str = "Sonante";
const PLEX_VERSION: &str = env!("CARGO_PKG_VERSION");
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
    SourceDisabled,
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
pub(super) struct PlexError {
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

    pub(super) fn public_message(&self) -> String {
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
            PlexErrorKind::SourceDisabled => "SOURCE_DISABLED".to_string(),
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

pub use crate::plex::PlexImageRef;

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

pub(super) fn validate_plex_server_id(server_id: &str) -> Result<(), PlexError> {
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

pub(super) fn validate_favorite_image(path:&str)->Result<(),PlexError>{validate_plex_image_path(path)}
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

    let mut response = response;
    const MAX_JSON: usize = 2 * 1024 * 1024;
    if response.content_length().is_some_and(|n| n > MAX_JSON as u64) { return Err(PlexError::invalid_response(operation)); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| PlexError::from_transport(operation,&error))? {
        if body.len().saturating_add(chunk.len()) > MAX_JSON {return Err(PlexError::invalid_response(operation));}
        body.extend_from_slice(&chunk);
    }

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

fn http_client() -> Result<&'static reqwest::Client, PlexError> {
    static CLIENT: OnceLock<Result<reqwest::Client, ()>> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(12))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(10)
        .build().map_err(|_| ()))
        .as_ref().map_err(|_| PlexError{operation:"preparar cliente HTTP",kind:PlexErrorKind::StateUnavailable})
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
pub(super) struct ResolvedPlexConnection {
    pub(super) server_id: String,
    base_url: String,
    token: String,
    pub(super) generation: u64,
}

impl ResolvedPlexConnection {
    pub(super) fn ab_text_safe(&self, text:&str)->bool {
        !contains_plex_token(text) && (self.token.is_empty() || !text.contains(&self.token))
    }
}

#[derive(Clone)]
pub(super) struct PlexConnectionSnapshot {
    identity: Option<PlexServerIdentity>,
    current_base_url: Option<String>,
    legacy_base_url: Option<String>,
    account_token: String,
    resolved_token: Option<String>,
    pub(super) generation: u64,
}

struct PlexConnectionState {
    enabled: bool,
    identity: Option<PlexServerIdentity>,
    current_base_url: Option<String>,
    legacy_base_url: Option<String>,
    account_token: String,
    resolved_token: Option<String>,
    pub(super) generation: u64,
}

pub(super) struct PlexConnectionManager {
    state: Mutex<PlexConnectionState>,
    http: reqwest::Client,
    probe_http: reqwest::Client,
    resources_url: String,
}

impl PlexConnectionManager {
    pub(super) fn from_config(config: &AppConfig) -> Result<Self, PlexError> {
        let probe_http=reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(PLEX_PROBE_TIMEOUT).build()
            .map_err(|error|PlexError::from_transport("preparar cliente HTTP",&error))?;
        Ok(Self::with_clients(config,http_client()?.clone(),probe_http,PLEX_RESOURCES_URL.to_string()))
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
                enabled: config.enabled,
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

    pub(super) fn snapshot(&self) -> Result<PlexConnectionSnapshot, PlexError> {
        let state = self.state.lock().map_err(|_| PlexError {
            operation: "acessar o estado da conexão",
            kind: PlexErrorKind::StateUnavailable,
        })?;
        if !state.enabled { return Err(PlexError {operation:"fonte desabilitada",kind:PlexErrorKind::SourceDisabled}); }
        Ok(PlexConnectionSnapshot {
            identity: state.identity.clone(),
            current_base_url: state.current_base_url.clone(),
            legacy_base_url: state.legacy_base_url.clone(),
            account_token: state.account_token.clone(),
            resolved_token: state.resolved_token.clone(),
            generation: state.generation,
        })
    }

    pub(super) fn update_config(&self, config: &AppConfig) {
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
        let connection_changed = state.enabled != config.enabled || state.identity != identity
            || state.legacy_base_url != legacy_base_url
            || state.account_token != config.plex_token;

        if connection_changed {
            state.generation = state.generation.wrapping_add(1);
            state.current_base_url = None;
            state.resolved_token = None;
        }
        state.enabled = config.enabled;
        state.identity = identity;
        state.legacy_base_url = legacy_base_url;
        state.account_token = config.plex_token.clone();
    }

    pub(super) async fn resolve(&self) -> Result<ResolvedPlexConnection, PlexError> {
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
                        || error.kind == PlexErrorKind::IdentityMismatch
                        // Initial account establishment only: a shared server's
                        // resource grant may differ from the login credential.
                        // refresh selects the exact identity and validates it;
                        // established content 401s remain terminal, without retry.
                        || error.kind == PlexErrorKind::Unauthorized) =>
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

    pub(super) async fn request_json<T: DeserializeOwned>(
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

    pub(super) async fn request_image(&self, image: &PlexImageRef) -> Result<Vec<u8>, PlexError> {
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
        self.validate_source_connection(connection)?;
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

    pub(super) fn validate_source_connection(&self, connection: &ResolvedPlexConnection) -> Result<(),PlexError> {
        let snapshot=self.snapshot()?;
        if snapshot.generation != connection.generation { return Err(PlexError{operation:"fonte alterada",kind:PlexErrorKind::ConfigurationChanged}); }
        Ok(())
    }
    async fn send_authenticated<T: DeserializeOwned>(
        &self,
        connection: &ResolvedPlexConnection,
        path_and_query: &str,
        operation: &'static str,
    ) -> Result<T, PlexError> {
        self.validate_source_connection(connection)?;
        let separator = if path_and_query.contains('?') { '&' } else { '?' };
        let url = format!(
            "{}{}{}X-Plex-Token={}",
            connection.base_url,
            path_and_query,
            separator,
            urlencoding::encode(&connection.token)
        );
        let result=send_json(
            self.http.get(url).header("Accept", "application/json"),
            operation,
        )
        .await;
        self.validate_source_connection(connection)?;
        result
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
        urlencoding::encode(token)
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
    let client = http_client().map_err(|error| error.public_message())?;
    let request = client
        .post("https://plex.tv/api/v2/pins")
        .header("X-Plex-Product", PLEX_PRODUCT_NAME)
        .header("X-Plex-Version", PLEX_VERSION)
        .header("X-Plex-Client-Identifier", PLEX_CLIENT_ID)
        .header("X-Plex-Platform", "Windows")
        .header("X-Plex-Device", "PC")
        .header("X-Plex-Device-Name", "Sonante (Windows)")
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
        "https://app.plex.tv/auth#?clientID={}&code={}&context%5Bdevice%5D%5Bproduct%5D={}&context%5Bdevice%5D%5Bplatform%5D=Windows&context%5Bdevice%5D%5Bdevice%5D=PC",
        PLEX_CLIENT_ID, pin.code, PLEX_PRODUCT_NAME
    );

    Ok(PlexPin {
        id: pin.id,
        code: pin.code,
        auth_url,
    })
}

pub async fn check_plex_pin(pin_id: u64) -> Result<Option<String>, String> {
    let client = http_client().map_err(|error| error.public_message())?;
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
    let servers = discover_plex_servers(http_client().map_err(|error| error.public_message())?, PLEX_RESOURCES_URL, auth_token)
        .await
        .map_err(|error| error.public_message())?;

    Ok(servers
        .into_iter()
        .map(into_public_server_resource)
        .collect())
}


#[cfg(test)]
#[path="core_tests.rs"]
mod tests;

#[path="catalog.rs"]
pub(super) mod catalog;

#[path="content.rs"]
pub(super) mod content;

#[path="shared_discovery.rs"]
pub(super) mod shared_discovery;

impl PlexConnectionManager {
    pub(super) async fn request_media(&self,http:&reqwest::Client,server:&str,part:&str,range:Option<&str>)->Result<reqwest::Response,String>{
        let mut connection=self.resolve().await.map_err(|e|e.public_message())?;
        for attempt in 0..2 {
            self.validate_source_connection(&connection).map_err(|e|e.public_message())?;
            if connection.server_id!=server{return Err("PLEX_CONTEXT_CHANGED".into());}
            let mut request=http.get(format!("{}{}",connection.base_url,part)).header("X-Plex-Token",&connection.token);
            if let Some(range)=range{request=request.header(reqwest::header::RANGE,range);}
            match request.send().await {
                Ok(response)=>{self.validate_source_connection(&connection).map_err(|e|e.public_message())?;return Ok(response);},
                Err(error) if attempt==0=>{let error=PlexError::from_transport("ler o áudio",&error);if !error.is_route_unavailable(){return Err(error.public_message());}connection=self.refresh(connection.generation).await.map_err(|e|e.public_message())?;},
                Err(_)=>return Err("PLEX_UNAVAILABLE".into()),
            }
        }
        Err("PLEX_UNAVAILABLE".into())
    }
}

impl PlexConnectionManager {
 pub(super) fn selected_server_id(&self)->Option<String>{self.snapshot().ok().and_then(|s|s.identity.map(|i|i.machine_identifier))}
 #[cfg(test)]pub(super) fn fixture(settings:&AppConfig,http:reqwest::Client)->Self{Self::with_clients(settings,http.clone(),http,"http://127.0.0.1:9/unused".into())}
}
