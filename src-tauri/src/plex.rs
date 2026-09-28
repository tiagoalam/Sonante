use crate::config::AppConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexArtistResult {
    pub rating_key: String,
    pub name: String,
    pub thumb: Option<String>,
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
    base_url: String,
    token: String,
    playback_mode: String,
    http: reqwest::Client,
    path_mappings: HashMap<String, String>,
}

impl PlexClient {
    pub async fn search(&self, query: &str, section_key: Option<&str>) -> Result<PlexSearchResults, String> {
        let mut url = format!(
            "{}/hubs/search?query={}&limit=12&X-Plex-Token={}",
            self.base_url,
            urlencoding::encode(query),
            self.token
        );

        if let Some(sec) = section_key {
            url.push_str(&format!("&sectionId={}", sec));
        }

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

        let mut artists = Vec::new();
        let mut albums = Vec::new();
        let mut tracks = Vec::new();

        if let Some(hubs) = json["MediaContainer"]["Hub"].as_array() {
            for hub in hubs {
                let hub_type = hub["type"].as_str().unwrap_or("");
                if let Some(meta) = hub["Metadata"].as_array() {
                    for item in meta {
                        match hub_type {
                            "artist" => {
                                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                                let name = item["title"].as_str().unwrap_or("").to_string();
                                let thumb = item["thumb"].as_str().map(|t| self.get_thumb_url(t));
                                artists.push(PlexArtistResult { rating_key, name, thumb });
                            }
                            "album" => {
                                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                                let artist = item["parentTitle"].as_str().unwrap_or("Vários Artistas").to_string();
                                let artist_rating_key = item["parentRatingKey"].as_str().map(|s| s.to_string());
                                let year = item["year"].as_u64().map(|y| y as u32);
                                let thumb = item["thumb"].as_str().map(|t| self.get_thumb_url(t));

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
                                if let Some(t) = self.parse_track_item(item) {
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
            base_url: cfg.plex_url.trim_end_matches('/').to_string(),
            token: cfg.plex_token.clone(),
            playback_mode: cfg.playback_mode.clone(),
            http: reqwest::Client::new(),
            path_mappings,
        }
    }

    pub fn update_config(&mut self, cfg: &AppConfig) {
        self.base_url = cfg.plex_url.trim_end_matches('/').to_string();
        self.token = cfg.plex_token.clone();
        self.playback_mode = cfg.playback_mode.clone();
        self.path_mappings.clear();
        if self.playback_mode == "local" && !cfg.remote_share_path.is_empty() && !cfg.local_mount_path.is_empty() {
            self.path_mappings.insert(
                cfg.remote_share_path.clone(),
                cfg.local_mount_path.clone(),
            );
        }
    }

    pub fn get_thumb_url(&self, thumb_path: &str) -> String {
        format!("{}{}?X-Plex-Token={}", self.base_url, thumb_path, self.token)
    }

    /// Helper reutilizável para converter itens brutos do JSON do Plex em PlexTrack
    fn parse_track_item(&self, item: &serde_json::Value) -> Option<PlexTrack> {
        let rating_key = item["ratingKey"].as_str()?.to_string();
        let title = item["title"].as_str().unwrap_or("Faixa").to_string();
        let album_title = item["parentTitle"].as_str().map(|s| s.to_string());
        let track_index = item["index"].as_u64().unwrap_or(1) as u32;
        let duration_ms = item["duration"].as_u64().unwrap_or(0);

        let thumb = item["thumb"]
            .as_str()
            .or_else(|| item["parentThumb"].as_str())
            .map(|t| self.get_thumb_url(t));

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
                format!("{}{}?X-Plex-Token={}", self.base_url, part_key, self.token)
            })
        } else {
            format!("{}{}?X-Plex-Token={}", self.base_url, part_key, self.token)
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
        let url = format!("{}/library/sections?X-Plex-Token={}", self.base_url, self.token);
        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut libraries = Vec::new();

        if let Some(dirs) = json["MediaContainer"]["Directory"].as_array() {
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

        let url = format!(
            "{}/library/sections/{}/all?type=9&sort={}&X-Plex-Token={}",
            self.base_url, section_key, sort_param, self.token
        );

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut albums = Vec::new();

        if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                let artist = item["parentTitle"].as_str().unwrap_or("Vários Artistas").to_string();
                let artist_rating_key = item["parentRatingKey"].as_str().map(|s| s.to_string());
                let year = item["year"].as_u64().map(|y| y as u32);
                let thumb = item["thumb"].as_str().map(|t| self.get_thumb_url(t));

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
        let url = format!(
            "{}/library/sections/{}/collections?X-Plex-Token={}",
            self.base_url, section_key, self.token
        );

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut collections = Vec::new();

        if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("").to_string();
                let child_count = item["childCount"].as_u64().unwrap_or(0) as u32;
                let thumb = item["thumb"].as_str().map(|t| self.get_thumb_url(t));

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
        let url = format!(
            "{}/library/collections/{}/children?X-Plex-Token={}",
            self.base_url, collection_rating_key, self.token
        );

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut albums = Vec::new();

        if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                let artist = item["parentTitle"].as_str().unwrap_or("Vários Artistas").to_string();
                let artist_rating_key = item["parentRatingKey"].as_str().map(|s| s.to_string());
                let year = item["year"].as_u64().map(|y| y as u32);
                let thumb = item["thumb"].as_str().map(|t| self.get_thumb_url(t));

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
        let url = format!(
            "{}/library/metadata/{}/children?X-Plex-Token={}",
            self.base_url, artist_rating_key, self.token
        );

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut albums = Vec::new();

        if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
            for item in meta {
                let rating_key = item["ratingKey"].as_str().unwrap_or("").to_string();
                let title = item["title"].as_str().unwrap_or("Desconhecido").to_string();
                let artist = item["parentTitle"].as_str().unwrap_or("").to_string();
                let year = item["year"].as_u64().map(|y| y as u32);
                let thumb = item["thumb"].as_str().map(|t| self.get_thumb_url(t));

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
        let url_top = format!(
            "{}/library/metadata/{}/topTracks?X-Plex-Token={}",
            self.base_url, artist_rating_key, self.token
        );
        if let Ok(resp) = self.http.get(&url_top).header("Accept", "application/json").send().await {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
                        let tracks: Vec<PlexTrack> = meta.iter().filter_map(|t| self.parse_track_item(t)).collect();
                        if !tracks.is_empty() {
                            return Ok(tracks);
                        }
                    }
                }
            }
        }

        // Tentativa 2: Hubs do artista (usado pelo Plex Web para renderizar o card Populares)
        let url_hub = format!(
            "{}/hubs/metadata/{}?X-Plex-Token={}",
            self.base_url, artist_rating_key, self.token
        );
        if let Ok(resp) = self.http.get(&url_hub).header("Accept", "application/json").send().await {
            if resp.status().is_success() {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(hubs) = json["MediaContainer"]["Hub"].as_array() {
                        for hub in hubs {
                            let hub_id = hub["hubIdentifier"].as_str().unwrap_or("");
                            let hub_type = hub["type"].as_str().unwrap_or("");
                            if hub_id == "artist.topTracks" || hub_type == "track" {
                                if let Some(meta) = hub["Metadata"].as_array() {
                                    let tracks: Vec<PlexTrack> = meta.iter().filter_map(|t| self.parse_track_item(t)).collect();
                                    if !tracks.is_empty() {
                                        return Ok(tracks);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Tentativa 3: allLeaves com ordenação manual em memória por popularidade
        let url_all = format!(
            "{}/library/metadata/{}/allLeaves?X-Plex-Token={}",
            self.base_url, artist_rating_key, self.token
        );
        let resp = self
            .http
            .get(&url_all)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut scored_tracks = Vec::new();

        if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
            for item in meta {
                let rating_count = item["ratingCount"].as_u64().unwrap_or(0);
                let view_count = item["viewCount"].as_u64().unwrap_or(0);
                if let Some(track) = self.parse_track_item(item) {
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
        let url = format!(
            "{}/library/metadata/{}/children?X-Plex-Token={}",
            self.base_url, album_rating_key, self.token
        );

        let resp = self
            .http
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut tracks = Vec::new();

        if let Some(meta) = json["MediaContainer"]["Metadata"].as_array() {
            for track in meta {
                if let Some(t) = self.parse_track_item(track) {
                    tracks.push(t);
                }
            }
        }

        Ok(tracks)
    }
}
