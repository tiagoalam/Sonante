//! Original Plex DTOs, track parsing, search, collections, artists and exact
//! navigation adapted to the Windows HTTP-only client. No IPC enabled here.
use super::*;
use super::catalog::PlexAlbum;
use crate::audio::MediaLocator;
pub(crate) struct PlexClient { pub(super) connection_manager: Arc<PlexConnectionManager> }
pub(crate) fn valid_id(value:&str)->bool{navigation_rating_key(value)}
pub(crate) fn validate_reference(server:&str,rating:&str,part:&str)->Result<(),String>{
 crate::windows_mpd::queue_v3::Source::Plex{server_id:server.to_owned(),part_key:part.to_owned(),rating_key:rating.to_owned()}.validate().map_err(str::to_owned)?;
 let decoded=urlencoding::decode(part).map_err(|_|"PLEX_INVALID")?;
 if !decoded.starts_with("/library/parts/") || decoded.starts_with("//")
    || decoded.chars().any(|c|c.is_control()||c=='\\'||c=='?'||c=='#')
    || decoded.split('/').any(|piece|matches!(piece,"."|"..")) {return Err("PLEX_INVALID".into());}
 Ok(())
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_rating_key: Option<String>,
    pub title: String,
    pub album_title: Option<String>,
    pub artist: Option<String>,
    pub thumb: Option<PlexImageRef>,
    pub track_index: u32,
    pub duration_ms: u64,
    pub media_locator: MediaLocator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_info: Option<facts::PlexFileInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexArtistResult {
    pub rating_key: String,
    pub name: String,
    pub thumb: Option<PlexImageRef>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexSearchResults {
    pub artists: Vec<PlexArtistResult>,
    pub albums: Vec<PlexAlbum>,
    pub tracks: Vec<PlexTrack>,
}

/// Navigation only: artwork is a safe server-relative reference, never a playback URL or file path.
#[derive(Debug, Serialize, Clone)]
pub struct PlexTrackNavigation {
    pub album: PlexAlbum,
    pub artist_rating_key: String,
    pub artist_name: String,
}

fn original_plex_album_genres(item: &serde_json::Value, connection: &ResolvedPlexConnection) -> Vec<String> {
    let mut genres = Vec::new();
    if let Some(entries) = item["Genre"].as_array() {
        for entry in entries {
            if let Some(value) = entry["tag"].as_str() {
                if connection.token.is_empty() || !value.contains(&connection.token) {
                    super::catalog::catalog_genres::add_genre(&mut genres, value);
                }
            }
        }
    }
    genres
}

fn navigation_rating_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn navigation_text(value: &serde_json::Value) -> Result<String, String> {
    value
        .as_str()
        .filter(|text| {
            !text.trim().is_empty()
                && text.len() <= 4096
                && !text.chars().any(char::is_control)
                && !contains_plex_token(text)
                && !text.contains("://")
        })
        .map(str::to_string)
        .ok_or_else(|| "PLAYER_NAVIGATION_UNAVAILABLE".to_string())
}

fn parse_track_navigation(
    json: &serde_json::Value,
    rating_key: &str,
    part_key: &str,
    connection: &ResolvedPlexConnection,
) -> Result<PlexTrackNavigation, String> {
    let invalid = || "PLAYER_NAVIGATION_UNAVAILABLE".to_string();
    let metadata = json["MediaContainer"]["Metadata"]
        .as_array()
        .ok_or_else(invalid)?;
    if metadata.len() != 1 {
        return Err(invalid());
    }
    let track = &metadata[0];
    if track["type"].as_str() != Some("track") || track["ratingKey"].as_str() != Some(rating_key) {
        return Err(invalid());
    }
    let mut matches = 0;
    for media in track["Media"].as_array().ok_or_else(invalid)? {
        for part in media["Part"].as_array().ok_or_else(invalid)? {
            if part["key"].as_str() == Some(part_key) {
                matches += 1;
            }
        }
    }
    if matches != 1 {
        return Err(invalid());
    }
    let album_key = track["parentRatingKey"]
        .as_str()
        .filter(|key| navigation_rating_key(key))
        .ok_or_else(invalid)?;
    let artist_key = track["grandparentRatingKey"]
        .as_str()
        .filter(|key| navigation_rating_key(key))
        .ok_or_else(invalid)?;
    if album_key == rating_key || artist_key == rating_key || album_key == artist_key {
        return Err(invalid());
    }
    let artist_name = navigation_text(&track["grandparentTitle"])?;
    Ok(PlexTrackNavigation {
        album: PlexAlbum {
            added_at: None,
            server_id: Some(connection.server_id.clone()),
            library_id: super::catalog::source_info::source_library(track),
            rating_key: album_key.to_string(),
            title: navigation_text(&track["parentTitle"])?,
            artist: artist_name.clone(),
            artist_rating_key: Some(artist_key.to_string()),
            year: None,
            genres: Vec::new(),
            thumb: navigation_album_thumb(track, album_key, connection),
        },
        artist_rating_key: artist_key.to_string(),
        artist_name,
    })
}

fn navigation_album_thumb(
    track: &serde_json::Value,
    album_key: &str,
    connection: &ResolvedPlexConnection,
) -> Option<PlexImageRef> {
    let prefix = format!("/library/metadata/{album_key}/thumb");
    ["parentThumb", "thumb"].into_iter().find_map(|field| {
        let path = track[field].as_str()?;
        // Only an album thumbnail endpoint proves that a fallback belongs to this album.
        // Exclude filesystem paths, query credentials and encoded/traversal suffixes.
        let suffix = path.strip_prefix(&prefix)?;
        if !suffix.is_empty()
            && !suffix.strip_prefix('/').is_some_and(|timestamp| {
                !timestamp.is_empty() && timestamp.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return None;
        }
        if !connection.token.is_empty() && path.contains(&connection.token) {
            return None;
        }
        PlexImageRef::from_connection(connection, path)
    })
}

// Read one exact artist, never an album thumbnail or a nominal search match.
async fn read_artist_portrait<F, Fut, S>(
    server_id: &str,
    rating_key: &str,
    request: F,
    snapshot: S,
) -> Result<Option<PlexImageRef>, String>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<
        Output = Result<(serde_json::Value, ResolvedPlexConnection), PlexError>,
    >,
    S: Fn() -> Result<PlexConnectionSnapshot, PlexError>,
{
    let operation = "resolver o retrato do artista";
    let invalid = || PlexError::invalid_response(operation).public_message();
    if validate_plex_server_id(server_id).is_err() || !navigation_rating_key(rating_key) {
        return Err(invalid());
    }
    let initial = snapshot().map_err(|error| error.public_message())?;
    if initial
        .identity
        .as_ref()
        .map(|identity| identity.machine_identifier.as_str())
        != Some(server_id)
    {
        return Err(invalid());
    }
    let (json, connection) = request(format!(
        "/library/metadata/{}",
        urlencoding::encode(rating_key)
    ))
    .await
    .map_err(|error| error.public_message())?;
    let current = snapshot().map_err(|error| error.public_message())?;
    if initial.generation != current.generation
        || initial.generation != connection.generation
        || connection.server_id != server_id
        || current
            .identity
            .as_ref()
            .map(|identity| identity.machine_identifier.as_str())
            != Some(server_id)
    {
        return Err(PlexError {
            operation,
            kind: PlexErrorKind::ConfigurationChanged,
        }
        .public_message());
    }
    let container = media_container(&json, operation).map_err(|error| error.public_message())?;
    if container
        .get("machineIdentifier")
        .is_some_and(|id| id.as_str() != Some(server_id))
    {
        return Err(invalid());
    }
    let items = container
        .get("Metadata")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(&invalid)?;
    if items.len() != 1
        || items[0]["type"].as_str() != Some("artist")
        || items[0]["ratingKey"].as_str() != Some(rating_key)
    {
        return Err(invalid());
    }
    let path = match items[0].get("thumb") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(value) => value.as_str().ok_or_else(&invalid)?,
    };
    if path.is_empty() {
        return Ok(None);
    }
    let prefix = format!("/library/metadata/{rating_key}/thumb");
    let suffix = path
        .split('?')
        .next()
        .and_then(|path| path.strip_prefix(&prefix))
        .ok_or_else(&invalid)?;
    if !suffix.is_empty()
        && !suffix.strip_prefix('/').is_some_and(|value| {
            !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(invalid());
    }
    let decoded_path = urlencoding::decode(path).map_err(|_| invalid())?;
    for token in [
        connection.token.as_str(),
        initial.account_token.as_str(),
        current.account_token.as_str(),
        current.resolved_token.as_deref().unwrap_or(""),
    ] {
        if !token.is_empty() && (path.contains(token) || decoded_path.contains(token)) {
            return Err(invalid());
        }
    }
    PlexImageRef::from_connection(&connection, path)
        .map(Some)
        .ok_or_else(invalid)
}


impl PlexClient {
    pub(crate) fn from_manager(connection_manager:Arc<PlexConnectionManager>)->Self{Self{connection_manager}}
    /// Helper reutilizável para converter itens brutos do JSON do Plex em PlexTrack
    pub(super) fn parse_track_item(
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
        let part_key = part["key"].as_str().filter(|key| !key.is_empty())?;

        if validate_reference(&connection.server_id, &rating_key, part_key).is_err() { return None; }
        Some(PlexTrack {
            album_rating_key: item["parentRatingKey"].as_str().filter(|key| navigation_rating_key(key) && (connection.token.is_empty() || !key.contains(&connection.token))).map(str::to_owned),
            audio_info: facts::track_file_info(item, &rating_key, part_key),
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
                file_path: None,
            },
        })
    }

    pub fn navigation_server_is_selected(&self, server_id: &str) -> bool {
        self.connection_manager
            .snapshot()
            .ok()
            .and_then(|snapshot| snapshot.identity)
            .is_some_and(|identity| identity.machine_identifier == server_id)
    }

    pub async fn resolve_track_navigation(
        &self,
        locator: &MediaLocator,
    ) -> Result<PlexTrackNavigation, String> {
        let invalid = || "PLAYER_NAVIGATION_UNAVAILABLE".to_string();
        let MediaLocator::Plex {
            server_id,
            part_key,
            rating_key: Some(rating_key),
            ..
        } = locator
        else {
            return Err(invalid());
        };
        if validate_plex_server_id(server_id).is_err()
            || !is_valid_plex_path(part_key)
            || part_key.len() > 4096
            || part_key.contains(['?', '#'])
            || contains_plex_token(part_key)
            || !navigation_rating_key(rating_key)
        {
            return Err(invalid());
        }
        let snapshot = self.connection_manager.snapshot().map_err(|_| invalid())?;
        if snapshot
            .identity
            .as_ref()
            .map(|identity| identity.machine_identifier.as_str())
            != Some(server_id.as_str())
        {
            return Err(invalid());
        }
        let path = format!("/library/metadata/{}", urlencoding::encode(rating_key));
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, "resolver a navegação da faixa")
            .await
            .map_err(|_| invalid())?;
        // Both the resolved route and the current selection must still belong to this server.
        let current = self.connection_manager.snapshot().map_err(|_| invalid())?;
        if connection.server_id != *server_id
            || current.generation != connection.generation
            || current
                .identity
                .as_ref()
                .map(|identity| identity.machine_identifier.as_str())
                != Some(server_id.as_str())
        {
            return Err(invalid());
        }
        if let Some(identity) = json["MediaContainer"].get("machineIdentifier") {
            if identity.as_str() != Some(server_id.as_str()) {
                return Err(invalid());
            }
        }
        let mut result = parse_track_navigation(&json, rating_key, part_key, &connection)?;
        // Do not publish a credential even if a malformed server places it in a display field.
        let fields = [
            result.album.title.as_str(),
            result.album.artist.as_str(),
            result.album.rating_key.as_str(),
            result.artist_rating_key.as_str(),
            part_key.as_str(),
        ];
        for token in [
            connection.token.as_str(),
            current.account_token.as_str(),
            current.resolved_token.as_deref().unwrap_or(""),
        ] {
            if !token.is_empty() && fields.iter().any(|field| field.contains(token)) {
                return Err(invalid());
            }
            if !token.is_empty()
                && result
                    .album
                    .thumb
                    .as_ref()
                    .is_some_and(|image| image.path.contains(token))
            {
                result.album.thumb = None;
            }
        }
        Ok(result)
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
                    added_at: item["addedAt"].as_u64().filter(|v| *v > 0 && *v <= 253_402_300_799),
                    server_id: Some(connection.server_id.clone()),
                    library_id: super::catalog::source_info::source_library(item),
                                    rating_key,
                                    title,
                                    artist,
                                    artist_rating_key,
                                    year,
                                    genres: original_plex_album_genres(item, &connection),
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
                    added_at: item["addedAt"].as_u64().filter(|v| *v > 0 && *v <= 253_402_300_799),
                    server_id: Some(connection.server_id.clone()),
                    library_id: super::catalog::source_info::source_library(item),
                    rating_key,
                    title,
                    artist,
                    artist_rating_key,
                    year,
                    genres: original_plex_album_genres(item, &connection),
                    thumb,
                });
            }
        }

        Ok(albums)
    }
    pub async fn get_artist_portrait(
        &self,
        server_id: &str,
        rating_key: &str,
    ) -> Result<Option<PlexImageRef>, String> {
        read_artist_portrait(
            server_id,
            rating_key,
            |path| async move {
                self.connection_manager
                    .request_json(&path, "resolver o retrato do artista")
                    .await
            },
            || self.connection_manager.snapshot(),
        )
        .await
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
                    added_at: item["addedAt"].as_u64().filter(|v| *v > 0 && *v <= 253_402_300_799),
                    server_id: Some(connection.server_id.clone()),
                    library_id: super::catalog::source_info::source_library(item),
                    rating_key,
                    title,
                    artist,
                    artist_rating_key: Some(artist_rating_key.to_string()),
                    year,
                    genres: original_plex_album_genres(item, &connection),
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
        if !valid_id(album_rating_key) {return Err("PLEX_INVALID".into());}
        let path = format!("/library/metadata/{}/children?X-Plex-Container-Start=0&X-Plex-Container-Size=500", album_rating_key);

        let operation = "consultar as faixas do álbum";
        let (json, connection): (serde_json::Value, _) = self
            .connection_manager
            .request_json(&path, operation)
            .await
            .map_err(|error| error.public_message())?;
        let container = media_container(&json, operation)
            .map_err(|error| error.public_message())?;
        let mut tracks = Vec::new();
        let meta = match container.get("Metadata") {
            Some(serde_json::Value::Array(rows))=>rows.as_slice(),
            None if container.get("size").and_then(serde_json::Value::as_u64)==Some(0)=>&[],
            _=>return Err("PLEX_DETAILS_INCOMPLETE".into()),
        };
        if meta.len()==500 && container.get("totalSize").is_none(){return Err("PLEX_DETAILS_INCOMPLETE".into());}
        if meta.len()>500 || container.get("totalSize").is_some_and(|v|v.as_u64()!=Some(meta.len() as u64))
            || container.get("size").is_some_and(|v|v.as_u64()!=Some(meta.len() as u64))
            || container.get("offset").is_some_and(|v|v.as_u64()!=Some(0)) {
            return Err("PLEX_DETAILS_INCOMPLETE".into());
        }
        for item in meta {
            if item.get("parentRatingKey").is_some_and(|value|value.as_str()!=Some(album_rating_key))
                || item.get("type").is_some_and(|value|value.as_str()!=Some("track"))
                || self.parse_track_item(&connection,item).is_none() { return Err("PLEX_UNSUPPORTED_MEDIA_REFERENCE".into()); }
        }
        self.connection_manager.validate_source_connection(&connection).map_err(|e|e.public_message())?;

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

#[cfg(test)]
#[path="content_tests.rs"]
mod tests;

#[path="facts.rs"]
mod facts;

pub(crate) struct VerifiedMedia{pub(crate) exists:Option<bool>,pub(crate) image:Option<PlexImageRef>,pub(crate) duration:Option<f64>}
impl PlexClient {
    pub(crate) async fn verify_reference(&self,source:&crate::windows_mpd::queue_v3::Source)->Result<Option<bool>,String>{self.inspect_reference(source).await.map(|v|v.exists)}
    /// None is exact metadata/Part identity without an existence observation;
    /// it can be attempted explicitly, but is not playlist availability proof.
    pub(crate) async fn inspect_reference(&self,source:&crate::windows_mpd::queue_v3::Source)->Result<VerifiedMedia,String>{
        let crate::windows_mpd::queue_v3::Source::Plex{server_id,part_key,rating_key}=source else{return Err("PLEX_INVALID".into())};
        validate_reference(server_id,rating_key,part_key)?;
        if !self.navigation_server_is_selected(server_id){return Err("PLEX_CONTEXT_CHANGED".into());}
        let operation="validar a disponibilidade da faixa";
        let path=format!("/library/metadata/{}?checkFileAvailability=1",urlencoding::encode(rating_key));
        let(json,connection):(serde_json::Value,_)=match self.connection_manager.request_json(&path,operation).await{
            Ok(value)=>value,
            Err(error) if error.kind==PlexErrorKind::NotFound&&error.operation==operation=>return Ok(VerifiedMedia{exists:Some(false),image:None,duration:None}),
            Err(error)=>return Err(error.public_message()),
        };
        if connection.server_id!=*server_id || !self.navigation_server_is_selected(server_id){return Err("PLEX_CONTEXT_CHANGED".into());}
        let container=media_container(&json,operation).map_err(|e|e.public_message())?;
        if container.get("machineIdentifier").is_some_and(|v|v.as_str()!=Some(server_id.as_str())){return Err("PLEX_CONTEXT_CHANGED".into());}
        let rows=container.get("Metadata").and_then(serde_json::Value::as_array).ok_or("PLEX_PROTOCOL")?;
        if rows.is_empty(){return Ok(VerifiedMedia{exists:Some(false),image:None,duration:None});}
        if rows.len()!=1 || rows[0]["ratingKey"].as_str()!=Some(rating_key.as_str()) || rows[0]["type"].as_str()!=Some("track"){return Err("PLEX_PROTOCOL".into());}
        let media=rows[0]["Media"].as_array().ok_or("PLEX_PROTOCOL")?;let mut matching=Vec::new();
        for media in media {for part in media["Part"].as_array().ok_or("PLEX_PROTOCOL")? {if part["key"].as_str()==Some(part_key.as_str()){matching.push(part);}}}
        if matching.is_empty(){return Ok(VerifiedMedia{exists:Some(false),image:None,duration:None});}if matching.len()!=1{return Err("PLEX_PROTOCOL".into());}
        let image=rows[0]["thumb"].as_str().or_else(||rows[0]["parentThumb"].as_str())
            .filter(|path|urlencoding::decode(path).is_ok_and(|decoded|connection.token.is_empty()||(!path.contains(&connection.token)&&!decoded.contains(&connection.token))))
            .and_then(|path|PlexImageRef::from_connection(&connection,path));
        Ok(VerifiedMedia{exists:json_boolean(matching[0].get("exists")),image,duration:crate::windows_mpd::library::presentation_duration(rows[0]["duration"].as_u64().map(|v|v as f64/1000.0))})
    }
}
