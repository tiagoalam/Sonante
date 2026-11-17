//! Original PMS album inventory/termination policy with an explicit Windows client.
use super::*;
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexLibrary {
    pub key: String,
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlexAlbum {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    pub rating_key: String,
    pub title: String,
    pub artist: String,
    pub artist_rating_key: Option<String>,
    pub year: Option<u32>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub thumb: Option<PlexImageRef>,
}

const ALBUM_PAGE_SIZE: usize = 200;
const MAX_ALBUM_PAGES: usize = 1_000;
const ALBUM_CATALOG_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlbumCatalogCompleteness {
    Complete,
    Partial,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct PlexAlbumCatalog {
    pub server_id: String,
    pub library_id: String,
    pub completeness: AlbumCatalogCompleteness,
    pub total: usize,
    pub albums: Vec<PlexAlbum>,
}

// Incomplete inventories are errors, never consumable album lists.
#[derive(Debug, Serialize)]
pub struct PlexAlbumCatalogError {
    pub completeness: AlbumCatalogCompleteness,
    pub received: usize,
    pub message: String,
}

impl PlexAlbumCatalogError {
    fn new(received: usize, message: String) -> Self {
        Self {
            completeness: if received == 0 {
                AlbumCatalogCompleteness::Unknown
            } else {
                AlbumCatalogCompleteness::Partial
            },
            received,
            message,
        }
    }
}

#[derive(Default)]
struct AlbumCatalogPages {
    albums: Vec<PlexAlbum>,
    ids: HashSet<String>,
    total: Option<usize>,
    bytes: usize,
}

impl AlbumCatalogPages {
    fn accept(
        &mut self,
        json: &serde_json::Value,
        connection: &ResolvedPlexConnection,
    ) -> Result<bool, String> {
        let invalid =
            || PlexError::invalid_response("validar a paginação dos álbuns").public_message();
        let container =
            media_container(json, "consultar os álbuns").map_err(|e| e.public_message())?;
        let number = |key: &str| -> Result<Option<usize>, String> {
            container
                .get(key)
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .ok_or_else(&invalid)
                })
                .transpose()
        };
        let offset = number("offset")?.ok_or_else(|| {
            "Completude Plex desconhecida: resposta sem offset de paginação.".to_string()
        })?;
        let size = number("size")?.ok_or_else(|| {
            "Completude Plex desconhecida: resposta sem size de paginação.".to_string()
        })?;
        let reported_total = number("totalSize")?;
        if offset != self.albums.len() || size > ALBUM_PAGE_SIZE {
            return Err(invalid());
        }
        if self
            .total
            .zip(reported_total)
            .is_some_and(|(old, new)| old != new)
        {
            return Err(invalid());
        }
        let total = reported_total.or(self.total);
        if total.is_some_and(|total| total > MAX_ALBUM_PAGES * ALBUM_PAGE_SIZE) {
            return Err(invalid());
        }
        let end = offset.checked_add(size).ok_or_else(&invalid)?;
        if total.is_some_and(|total| end > total || (size == 0 && end != total)) {
            return Err(invalid());
        }
        let metadata = match container.get("Metadata") {
            Some(value) => value.as_array().ok_or_else(&invalid)?.as_slice(),
            None if size == 0 => &[],
            None => return Err(invalid()),
        };
        if metadata.len() != size {
            return Err(invalid());
        }
        let mut page_ids = HashSet::new();
        let mut page = Vec::with_capacity(size);
        for item in metadata {
            let id = item["ratingKey"]
                .as_str()
                .filter(|id| navigation_rating_key(id))
                .ok_or_else(&invalid)?;
            if self.ids.contains(id) || !page_ids.insert(id.to_string()) {
                return Err(invalid());
            }
            if item.get("Genre").and_then(serde_json::Value::as_array).is_some_and(|v|v.len()>128){return Err(invalid());}
            page.push(PlexAlbum {
                added_at: item["addedAt"].as_u64().filter(|v| *v > 0 && *v <= 253_402_300_799),
                server_id: Some(connection.server_id.clone()),
                library_id: source_info::source_library(item),
                rating_key: id.to_string(),
                title: item["title"].as_str().unwrap_or("Desconhecido").to_string(),
                artist: item["parentTitle"]
                    .as_str()
                    .unwrap_or("Vários Artistas")
                    .to_string(),
                artist_rating_key: item["parentRatingKey"].as_str().map(str::to_string),
                year: item["year"]
                    .as_u64()
                    .and_then(|year| u32::try_from(year).ok()),
                genres: plex_album_genres(item, connection),
                thumb: item["thumb"]
                    .as_str()
                    .and_then(|path| PlexImageRef::from_connection(connection, path)),
            });
        }
        let bytes = page.iter().map(|a| a.rating_key.len()+a.title.len()+a.artist.len()+a.artist_rating_key.as_ref().map_or(0,String::len)+a.genres.iter().map(String::len).sum::<usize>()+a.thumb.as_ref().map_or(0,|i|i.server_id.len()+i.path.len())+128).sum::<usize>();
        let total_bytes=self.bytes.checked_add(bytes).ok_or_else(&invalid)?;
        if total_bytes>32*1024*1024{return Err(invalid());}
        self.bytes=total_bytes;
        self.total = total;
        self.ids.extend(page_ids);
        self.albums.extend(page);
        // A short page alone proves nothing. Without totalSize, require an explicit
        // empty page at the requested offset after traversing all preceding pages.
        Ok(total == Some(end) || size == 0)
    }
}

// Both the real client and fixture transport run this exact sequence.
async fn read_album_catalog<F, Fut, S>(
    section_key: &str,
    sort_by: &str,
    initial: PlexConnectionSnapshot,
    mut request: F,
    snapshot: S,
) -> Result<PlexAlbumCatalog, PlexAlbumCatalogError>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<
        Output = Result<(serde_json::Value, ResolvedPlexConnection), PlexError>,
    >,
    S: Fn() -> Result<PlexConnectionSnapshot, PlexError>,
{
    let mut pages = AlbumCatalogPages::default();
    let fail = |received, message| PlexAlbumCatalogError::new(received, message);
    if !navigation_rating_key(section_key) {
        return Err(fail(
            0,
            PlexError::invalid_response("consultar a biblioteca").public_message(),
        ));
    }
    let started = Instant::now();
    let mut server_id: Option<String> = None;
    let sort = match sort_by {
        "title" => "titleSort:asc",
        "year" => "originallyAvailableAt:desc",
        _ => "addedAt:desc",
    };
    for _ in 0..MAX_ALBUM_PAGES {
        let current = snapshot().map_err(|e| fail(pages.albums.len(), e.public_message()))?;
        if current.generation != initial.generation {
            return Err(fail(
                pages.albums.len(),
                PlexError {
                    operation: "consultar os álbuns",
                    kind: PlexErrorKind::ConfigurationChanged,
                }
                .public_message(),
            ));
        }
        if started.elapsed() >= ALBUM_CATALOG_TIMEOUT {
            break;
        }
        let path = format!("/library/sections/{section_key}/all?type=9&sort={sort}&X-Plex-Container-Start={}&X-Plex-Container-Size={ALBUM_PAGE_SIZE}", pages.albums.len());
        let (json, connection): (serde_json::Value, _) = request(path)
            .await
            .map_err(|e| fail(pages.albums.len(), e.public_message()))?;
        let current = snapshot().map_err(|e| fail(pages.albums.len(), e.public_message()))?;
        if connection.generation != initial.generation
            || current.generation != initial.generation
            || server_id
                .as_ref()
                .is_some_and(|id| id != &connection.server_id)
            || initial
                .identity
                .as_ref()
                .is_some_and(|id| id.machine_identifier != connection.server_id)
        {
            return Err(fail(
                pages.albums.len(),
                PlexError {
                    operation: "consultar os álbuns",
                    kind: PlexErrorKind::ConfigurationChanged,
                }
                .public_message(),
            ));
        }
        if started.elapsed() >= ALBUM_CATALOG_TIMEOUT {
            break;
        }
        server_id = Some(connection.server_id.clone());
        let complete = pages
            .accept(&json, &connection)
            .map_err(|message| fail(pages.albums.len(), message))?;
        if complete {
            for album in &mut pages.albums { album.library_id = Some(section_key.to_string()); }
            return Ok(PlexAlbumCatalog {
                server_id: connection.server_id,
                library_id: section_key.to_string(),
                completeness: AlbumCatalogCompleteness::Complete,
                total: pages.albums.len(),
                albums: pages.albums,
            });
        }
    }
    Err(fail(
        pages.albums.len(),
        "Catálogo Plex incompleto: limite de páginas ou tempo atingido.".to_string(),
    ))
}

fn plex_album_genres(item: &serde_json::Value, connection: &ResolvedPlexConnection) -> Vec<String> {
    let mut genres = Vec::new();
    if let Some(entries) = item["Genre"].as_array() {
        for entry in entries {
            if let Some(value) = entry["tag"].as_str() {
                if connection.token.is_empty() || !value.contains(&connection.token) {
                    catalog_genres::add_genre(&mut genres, value);
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


pub(super) mod catalog_genres {
/// Merge metadata without selecting a dominant genre or splitting arbitrary tag text.
pub(crate) fn add_genre(genres: &mut Vec<String>, value: &str) {
    let display = value.trim();
    if display.is_empty()
        || display.len() > 4096
        || value.chars().any(char::is_control)
        || display.contains("://")
        || display.to_lowercase().contains("x-plex-token")
    {
        return;
    }
    let key = display
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if !genres.iter().any(|genre| {
        genre
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            == key
    }) {
        genres.push(display.to_string());
    }
}


}
pub(super) mod source_info {
use serde_json::Value;
pub(crate) fn source_library(item: &Value) -> Option<String> {
    let value = item.get("librarySectionID")?;
    let id = value.as_str().map(str::to_owned).or_else(|| value.as_u64().map(|n| n.to_string()))?;
    super::navigation_rating_key(&id).then_some(id)
}

}
    pub(crate) async fn get_music_libraries(client: &super::super::Client) -> Result<Vec<PlexLibrary>, String> {
        let operation = "consultar as bibliotecas";
        let (json, _connection): (serde_json::Value, _) = client
            .manager
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

    pub(super) async fn get_albums(
        client: &super::super::Client,
        section_key: &str,
        sort_by: &str,
    ) -> Result<PlexAlbumCatalog, PlexAlbumCatalogError> {
        let initial = client
            .manager
            .snapshot()
            .map_err(|e| PlexAlbumCatalogError::new(0, e.public_message()))?;
        read_album_catalog(
            section_key,
            sort_by,
            initial,
            |path| async move {
                client.manager
                    .request_json(&path, "consultar os álbuns")
                    .await
            },
            || client.manager.snapshot(),
        )
        .await
    }


pub(crate) async fn albums(client:&super::super::Client,section:&str,sort:&str)->Result<PlexAlbumCatalog,PlexAlbumCatalogError>{
    let _slot=client.network.clone().try_acquire_owned().map_err(|_|PlexAlbumCatalogError::new(0,"PLEX_UNAVAILABLE".into()))?;
    let mut stop=client.stop.subscribe();if *stop.borrow(){return Err(PlexAlbumCatalogError::new(0,"PLEX_CONTEXT_CHANGED".into()));}
    tokio::select!{biased;_=stop.wait_for(|closed|*closed)=>Err(PlexAlbumCatalogError::new(0,"PLEX_CONTEXT_CHANGED".into())),result=get_albums(client,section,sort)=>result}
}

#[cfg(test)]
#[path="catalog_tests.rs"]
mod tests;
