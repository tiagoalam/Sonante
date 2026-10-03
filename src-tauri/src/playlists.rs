use crate::audio::MediaLocator;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const PLAYLISTS_FILE_NAME: &str = "playlists.json";
const PLAYLISTS_SCHEMA_VERSION: u32 = 1;
const MAX_JAVASCRIPT_DATE_MILLIS: u64 = 8_640_000_000_000_000;
static NEXT_PLAYLIST_ID: AtomicU64 = AtomicU64::new(0);
static NEXT_PLAYLIST_ITEM_ID: AtomicU64 = AtomicU64::new(0);

pub const DUPLICATE_PLAYLIST_NAME_ERROR: &str = "playlist_name_duplicate";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaylistItemMetadata {
    /// Snapshot apenas para apresentação quando a origem estiver indisponível.
    /// Nunca substitui `media_locator` como identidade da faixa.
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaylistItem {
    /// Identidade da ocorrência dentro da playlist; permite duplicatas e remoção
    /// individual sem usar nome ou posição como chave.
    pub id: String,
    /// Referência estável e novamente resolvível. A resolução pode marcar o item
    /// como missing ou unavailable, mas nunca deve removê-lo automaticamente.
    pub media_locator: MediaLocator,
    pub metadata: PlaylistItemMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewPlaylistItem {
    pub media_locator: MediaLocator,
    pub metadata: PlaylistItemMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub items: Vec<PlaylistItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PlaylistFile {
    version: u32,
    playlists: Vec<Playlist>,
}

pub struct PlaylistStore {
    path: PathBuf,
}

pub struct PlaylistState(pub Mutex<PlaylistStore>);

impl Default for PlaylistStore {
    fn default() -> Self {
        Self::new(crate::persistence::sonante_config_dir().join(PLAYLISTS_FILE_NAME))
    }
}

impl PlaylistStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn list(&self) -> Result<Vec<Playlist>, String> {
        self.load()
    }

    pub fn create(&self, name: &str) -> Result<Playlist, String> {
        self.create_with_items(name, Vec::new())
    }

    pub fn create_with_items(
        &self,
        name: &str,
        items: Vec<NewPlaylistItem>,
    ) -> Result<Playlist, String> {
        let name = validate_name(name)?;
        let mut playlists = self.load()?;
        ensure_unique_name(&playlists, &name, None)?;
        let items = prepare_new_items(items)?;
        let timestamp = now_millis()?;
        let mut id = generate_id(timestamp);
        while playlists.iter().any(|playlist| playlist.id == id) {
            id = generate_id(timestamp);
        }
        let playlist = Playlist {
            id,
            name,
            created_at: timestamp,
            updated_at: timestamp,
            items,
        };
        playlists.push(playlist.clone());
        self.save(&playlists)?;
        Ok(playlist)
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<Playlist, String> {
        let name = validate_name(name)?;
        let mut playlists = self.load()?;
        ensure_unique_name(&playlists, &name, Some(id))?;
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == id)
            .ok_or_else(|| "Playlist não encontrada.".to_string())?;
        playlist.name = name;
        playlist.updated_at = now_millis()?.max(playlist.updated_at.saturating_add(1));
        let updated = playlist.clone();
        self.save(&playlists)?;
        Ok(updated)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let mut playlists = self.load()?;
        let original_len = playlists.len();
        playlists.retain(|playlist| playlist.id != id);
        if playlists.len() == original_len {
            return Err("Playlist não encontrada.".to_string());
        }
        self.save(&playlists)
    }

    pub fn get(&self, id: &str) -> Result<Playlist, String> {
        self.load()?
            .into_iter()
            .find(|playlist| playlist.id == id)
            .ok_or_else(|| "Playlist não encontrada.".to_string())
    }

    pub fn add_item(&self, playlist_id: &str, item: NewPlaylistItem) -> Result<Playlist, String> {
        self.add_items(playlist_id, vec![item])
    }

    pub fn add_items(
        &self,
        playlist_id: &str,
        items: Vec<NewPlaylistItem>,
    ) -> Result<Playlist, String> {
        if items.is_empty() {
            return Err("Informe ao menos uma faixa para adicionar.".to_string());
        }
        // O lote inteiro é validado e materializado antes de qualquer mutação ou escrita.
        let items = prepare_new_items(items)?;
        let mut playlists = self.load()?;
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Playlist não encontrada.".to_string())?;
        playlist.items.extend(items);
        touch(playlist)?;
        let updated = playlist.clone();
        self.save(&playlists)?;
        Ok(updated)
    }

    pub fn remove_item(&self, playlist_id: &str, item_id: &str) -> Result<Playlist, String> {
        let mut playlists = self.load()?;
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Playlist não encontrada.".to_string())?;
        let original_len = playlist.items.len();
        playlist.items.retain(|item| item.id != item_id);
        if playlist.items.len() == original_len {
            return Err("Item da playlist não encontrado.".to_string());
        }
        touch(playlist)?;
        let updated = playlist.clone();
        self.save(&playlists)?;
        Ok(updated)
    }

    pub fn reorder_items(
        &self,
        playlist_id: &str,
        ordered_item_ids: &[String],
    ) -> Result<Playlist, String> {
        let mut playlists = self.load()?;
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Playlist não encontrada.".to_string())?;
        if ordered_item_ids.len() != playlist.items.len() {
            return Err("A nova ordem não contém todos os itens da playlist.".to_string());
        }
        let requested: HashSet<&str> = ordered_item_ids.iter().map(String::as_str).collect();
        if requested.len() != ordered_item_ids.len()
            || playlist
                .items
                .iter()
                .any(|item| !requested.contains(item.id.as_str()))
        {
            return Err("A nova ordem contém itens inválidos ou duplicados.".to_string());
        }
        let mut previous = std::mem::take(&mut playlist.items)
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect::<std::collections::HashMap<_, _>>();
        let mut reordered = Vec::with_capacity(ordered_item_ids.len());
        for id in ordered_item_ids {
            let item = previous
                .remove(id)
                .ok_or_else(|| "A nova ordem contém item desconhecido.".to_string())?;
            reordered.push(item);
        }
        playlist.items = reordered;
        touch(playlist)?;
        let updated = playlist.clone();
        self.save(&playlists)?;
        Ok(updated)
    }

    fn load(&self) -> Result<Vec<Playlist>, String> {
        crate::persistence::prepare_private_file_for_load(&self.path, PLAYLISTS_FILE_NAME)?;
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|error| format!("Falha ao ler {}: {}", PLAYLISTS_FILE_NAME, error))?;
        let file: PlaylistFile = serde_json::from_str(&content).map_err(|error| {
            format!(
                "{} contém dados inválidos e foi preservado sem alterações: {}",
                PLAYLISTS_FILE_NAME, error
            )
        })?;
        if file.version != PLAYLISTS_SCHEMA_VERSION {
            return Err(format!(
                "Versão {} de {} não é suportada.",
                file.version, PLAYLISTS_FILE_NAME
            ));
        }
        validate_loaded_playlists(&file.playlists)?;
        Ok(file.playlists)
    }

    fn save(&self, playlists: &[Playlist]) -> Result<(), String> {
        let file = PlaylistFile {
            version: PLAYLISTS_SCHEMA_VERSION,
            playlists: playlists.to_vec(),
        };
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|error| format!("Falha ao serializar {}: {}", PLAYLISTS_FILE_NAME, error))?;
        crate::persistence::atomic_write_private(&self.path, &json, PLAYLISTS_FILE_NAME)
    }
}

fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("O nome da playlist não pode ficar vazio.".to_string());
    }
    Ok(name.to_string())
}

fn normalized_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn ensure_unique_name(
    playlists: &[Playlist],
    name: &str,
    current_playlist_id: Option<&str>,
) -> Result<(), String> {
    let normalized = normalized_name(name);
    if playlists.iter().any(|playlist| {
        Some(playlist.id.as_str()) != current_playlist_id
            && normalized_name(&playlist.name) == normalized
    }) {
        return Err(DUPLICATE_PLAYLIST_NAME_ERROR.to_string());
    }
    Ok(())
}

fn prepare_new_items(items: Vec<NewPlaylistItem>) -> Result<Vec<PlaylistItem>, String> {
    let timestamp = now_millis()?;
    items
        .into_iter()
        .map(|item| {
            validate_new_item(&item)?;
            Ok(PlaylistItem {
                id: generate_item_id(timestamp),
                media_locator: item.media_locator,
                metadata: item.metadata,
            })
        })
        .collect()
}

fn validate_new_item(item: &NewPlaylistItem) -> Result<(), String> {
    validate_media_locator(&item.media_locator)?;
    if item.metadata.title.trim().is_empty()
        || crate::plex::contains_plex_token(&item.metadata.title)
        || crate::plex::contains_plex_token(&item.metadata.artist)
        || crate::plex::contains_plex_token(&item.metadata.album)
        || item
            .metadata
            .duration
            .is_some_and(|duration| !duration.is_finite() || duration < 0.0)
    {
        return Err("Snapshot de metadata da playlist inválido.".to_string());
    }
    Ok(())
}

fn touch(playlist: &mut Playlist) -> Result<(), String> {
    playlist.updated_at = now_millis()?.max(playlist.updated_at.saturating_add(1));
    Ok(())
}

fn validate_loaded_playlists(playlists: &[Playlist]) -> Result<(), String> {
    let mut ids = HashSet::with_capacity(playlists.len());
    let mut names = HashSet::with_capacity(playlists.len());
    for playlist in playlists {
        if playlist.id.trim().is_empty() || playlist.name.trim().is_empty() {
            return Err(format!(
                "{} contém playlist sem identidade ou nome válido.",
                PLAYLISTS_FILE_NAME
            ));
        }
        if !ids.insert(&playlist.id) {
            return Err(format!(
                "{} contém IDs de playlist duplicados.",
                PLAYLISTS_FILE_NAME
            ));
        }
        if !names.insert(normalized_name(&playlist.name)) {
            return Err(format!(
                "{} contém nomes de playlist duplicados.",
                PLAYLISTS_FILE_NAME
            ));
        }
        if playlist.created_at == 0
            || playlist.updated_at < playlist.created_at
            || playlist.updated_at > MAX_JAVASCRIPT_DATE_MILLIS
        {
            return Err(format!(
                "{} contém timestamps inválidos.",
                PLAYLISTS_FILE_NAME
            ));
        }
        let mut item_ids = HashSet::with_capacity(playlist.items.len());
        for item in &playlist.items {
            if item.id.trim().is_empty() || !item_ids.insert(&item.id) {
                return Err(format!(
                    "{} contém item sem identidade válida ou duplicada.",
                    PLAYLISTS_FILE_NAME
                ));
            }
            validate_media_locator(&item.media_locator)?;
            if item.metadata.title.trim().is_empty()
                || crate::plex::contains_plex_token(&item.metadata.title)
                || crate::plex::contains_plex_token(&item.metadata.artist)
                || crate::plex::contains_plex_token(&item.metadata.album)
                || item
                    .metadata
                    .duration
                    .is_some_and(|duration| !duration.is_finite() || duration < 0.0)
            {
                return Err(format!(
                    "{} contém snapshot de metadata inválido.",
                    PLAYLISTS_FILE_NAME
                ));
            }
        }
    }
    Ok(())
}

fn validate_media_locator(locator: &MediaLocator) -> Result<(), String> {
    let valid = match locator {
        MediaLocator::Local { uri } => {
            !uri.trim().is_empty()
                && !uri.contains(['\0', '\r', '\n'])
                && !crate::plex::contains_plex_token(uri)
        }
        MediaLocator::Plex {
            server_id,
            part_key,
            rating_key,
            file_path,
        } => {
            !server_id.trim().is_empty()
                && server_id.trim() == server_id
                && server_id.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                })
                && !part_key.trim().is_empty()
                && part_key.starts_with('/')
                && !part_key.starts_with("//")
                && !part_key.contains(['\0', '\r', '\n'])
                && !part_key.contains("://")
                && !crate::plex::contains_plex_token(server_id)
                && !crate::plex::contains_plex_token(part_key)
                && !rating_key.as_deref().is_some_and(|key| {
                    key.trim().is_empty()
                        || key.trim() != key
                        || key.contains(['\0', '\r', '\n', '/', '?', '#'])
                        || crate::plex::contains_plex_token(key)
                })
                && !file_path
                    .as_deref()
                    .is_some_and(|path| {
                        path.contains(['\0', '\r', '\n'])
                            || crate::plex::contains_plex_token(path)
                    })
        }
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{} contém referência de mídia inválida ou autenticada.",
            PLAYLISTS_FILE_NAME
        ))
    }
}

fn now_millis() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("Relógio do sistema inválido para playlists: {}", error))?
        .as_millis();
    u64::try_from(millis).map_err(|_| "Data atual fora da faixa suportada.".to_string())
}

fn generate_id(timestamp: u64) -> String {
    let sequence = NEXT_PLAYLIST_ID.fetch_add(1, Ordering::Relaxed);
    format!("pl_{:x}_{:x}_{:x}", timestamp, std::process::id(), sequence)
}

fn generate_item_id(timestamp: u64) -> String {
    let sequence = NEXT_PLAYLIST_ITEM_ID.fetch_add(1, Ordering::Relaxed);
    format!("pli_{:x}_{:x}_{:x}", timestamp, std::process::id(), sequence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_store(name: &str) -> (PathBuf, PlaylistStore) {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("playlist-tests")
            .join(format!("{}-{}-{}", std::process::id(), name, id));
        fs::create_dir_all(&directory).unwrap();
        let store = PlaylistStore::new(directory.join(PLAYLISTS_FILE_NAME));
        (directory, store)
    }

    fn local_item(uri: &str, title: &str) -> NewPlaylistItem {
        NewPlaylistItem {
            media_locator: MediaLocator::Local { uri: uri.into() },
            metadata: PlaylistItemMetadata {
                title: title.into(),
                artist: "Artista".into(),
                album: "Álbum".into(),
                duration: Some(123.0),
            },
        }
    }

    fn plex_item(part_key: &str, title: &str) -> NewPlaylistItem {
        NewPlaylistItem {
            media_locator: MediaLocator::Plex {
                server_id: "server-id".into(),
                part_key: part_key.into(),
                rating_key: Some("track-42".into()),
                file_path: None,
            },
            metadata: PlaylistItemMetadata {
                title: title.into(),
                artist: "Artista Plex".into(),
                album: "Álbum Plex".into(),
                duration: None,
            },
        }
    }

    #[test]
    fn creates_and_lists_playlists_with_stable_distinct_ids() {
        let (directory, store) = test_store("create-list");
        let first = store.create(" Viagem ").unwrap();
        let second = store.create("Trabalho").unwrap();
        assert_eq!(first.name, "Viagem");
        assert_eq!(second.name, "Trabalho");
        assert_ne!(first.id, second.id);
        assert!(first.items.is_empty());
        assert_eq!(store.list().unwrap(), vec![first, second]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_duplicate_name_on_create_after_trim_and_case_folding() {
        let (directory, store) = test_store("duplicate-create");
        store.create("Rock").unwrap();
        assert_eq!(store.create(" rock ").unwrap_err(), DUPLICATE_PLAYLIST_NAME_ERROR);
        assert_eq!(store.create("ROCK").unwrap_err(), DUPLICATE_PLAYLIST_NAME_ERROR);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_duplicate_name_on_rename_but_allows_own_name() {
        let (directory, store) = test_store("duplicate-rename");
        let rock = store.create("Rock").unwrap();
        let jazz = store.create("Jazz").unwrap();
        assert_eq!(
            store.rename(&jazz.id, " ROCK ").unwrap_err(),
            DUPLICATE_PLAYLIST_NAME_ERROR
        );
        assert_eq!(store.rename(&rock.id, " rock ").unwrap().name, "rock");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn renames_playlist_and_updates_timestamp() {
        let (directory, store) = test_store("rename");
        let created = store.create("Original").unwrap();
        let renamed = store.rename(&created.id, " Novo nome ").unwrap();
        assert_eq!(renamed.id, created.id);
        assert_eq!(renamed.name, "Novo nome");
        assert!(renamed.updated_at > created.updated_at);
        assert_eq!(store.list().unwrap(), vec![renamed]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deletes_only_requested_playlist() {
        let (directory, store) = test_store("delete");
        let first = store.create("Primeira").unwrap();
        let second = store.create("Segunda").unwrap();
        store.delete(&first.id).unwrap();
        assert_eq!(store.list().unwrap(), vec![second]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_empty_names_for_create_and_rename() {
        let (directory, store) = test_store("empty-name");
        assert!(store.create("   ").is_err());
        let playlist = store.create("Válida").unwrap();
        assert!(store.rename(&playlist.id, "\n\t").is_err());
        assert_eq!(store.list().unwrap(), vec![playlist]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn persists_and_loads_from_a_new_store_instance() {
        let (directory, store) = test_store("reload");
        let created = store.create("Persistente").unwrap();
        let reloaded = PlaylistStore::new(store.path.clone());
        assert_eq!(reloaded.list().unwrap(), vec![created]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_file_is_reported_and_never_overwritten() {
        let (directory, store) = test_store("invalid");
        let invalid = b"{ definitely-not-json";
        fs::write(&store.path, invalid).unwrap();
        assert!(store.list().unwrap_err().contains("dados inválidos"));
        assert!(store.create("Não sobrescrever").is_err());
        assert_eq!(fs::read(&store.path).unwrap(), invalid);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stable_local_and_plex_references_survive_without_origin_checks_or_removal() {
        let (directory, store) = test_store("media-references");
        let mut playlist = store.create("Referências").unwrap();
        playlist.items = vec![
            PlaylistItem {
                id: "item-local".into(),
                media_locator: MediaLocator::Local {
                    uri: "disco-removido/album/faixa.flac".into(),
                },
                metadata: PlaylistItemMetadata {
                    title: "Faixa local".into(),
                    artist: "Artista".into(),
                    album: "Álbum".into(),
                    duration: Some(123.0),
                },
            },
            PlaylistItem {
                id: "item-plex".into(),
                media_locator: MediaLocator::Plex {
                    server_id: "server-id".into(),
                    part_key: "/library/parts/42/file.flac".into(),
                    rating_key: Some("track-42".into()),
                    file_path: None,
                },
                metadata: PlaylistItemMetadata {
                    title: "Faixa Plex removida".into(),
                    artist: "Artista".into(),
                    album: "Álbum".into(),
                    duration: None,
                },
            },
        ];
        store.save(&[playlist.clone()]).unwrap();
        assert_eq!(store.list().unwrap(), vec![playlist]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn authenticated_media_reference_is_rejected() {
        let locator = MediaLocator::Plex {
            server_id: "server-id".into(),
            part_key: "/library/parts/42?X-Plex-Token=SECRET".into(),
            rating_key: Some("track-42".into()),
            file_path: None,
        };
        assert!(validate_media_locator(&locator).is_err());
    }

    #[test]
    fn adds_local_and_plex_items_with_distinct_occurrence_ids() {
        let (directory, store) = test_store("add-local-plex");
        let playlist = store.create("Mista").unwrap();
        let with_local = store
            .add_item(&playlist.id, local_item("local/track.flac", "Local"))
            .unwrap();
        let with_plex = store
            .add_item(
                &playlist.id,
                plex_item("/library/parts/42/file.flac", "Plex"),
            )
            .unwrap();
        assert_eq!(with_local.items.len(), 1);
        assert_eq!(with_plex.items.len(), 2);
        assert_ne!(with_plex.items[0].id, with_plex.items[1].id);
        assert!(matches!(with_plex.items[0].media_locator, MediaLocator::Local { .. }));
        assert!(matches!(with_plex.items[1].media_locator, MediaLocator::Plex { .. }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_authenticated_plex_locator_before_writing() {
        let (directory, store) = test_store("reject-authenticated");
        let playlist = store.create("Segura").unwrap();
        let authenticated_part = format!("/library/parts/42?{}={}", "X-Plex-Token", "credential");
        let item = plex_item(&authenticated_part, "Segredo");
        assert!(store.add_item(&playlist.id, item).is_err());
        assert!(store.get(&playlist.id).unwrap().items.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn batch_add_preserves_input_order_and_is_atomic_on_validation_error() {
        let (directory, store) = test_store("batch-order");
        let playlist = store.create("Álbum").unwrap();
        let updated = store
            .add_items(
                &playlist.id,
                vec![
                    local_item("album/01.flac", "Um"),
                    local_item("album/02.flac", "Dois"),
                    local_item("album/03.flac", "Três"),
                ],
            )
            .unwrap();
        assert_eq!(
            updated
                .items
                .iter()
                .map(|item| item.metadata.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Um", "Dois", "Três"]
        );
        let invalid = NewPlaylistItem {
            metadata: PlaylistItemMetadata {
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                duration: None,
            },
            ..local_item("album/04.flac", "Quatro")
        };
        assert!(store
            .add_items(&playlist.id, vec![local_item("album/04.flac", "Quatro"), invalid])
            .is_err());
        assert_eq!(store.get(&playlist.id).unwrap(), updated);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn removes_one_occurrence_and_reorders_by_item_identity() {
        let (directory, store) = test_store("remove-reorder");
        let playlist = store.create("Ordem").unwrap();
        let added = store
            .add_items(
                &playlist.id,
                vec![
                    local_item("same.flac", "Um"),
                    local_item("same.flac", "Um novamente"),
                    local_item("other.flac", "Dois"),
                ],
            )
            .unwrap();
        let removed = store.remove_item(&playlist.id, &added.items[0].id).unwrap();
        assert_eq!(removed.items.len(), 2);
        assert_eq!(removed.items[0].id, added.items[1].id);
        let order = vec![removed.items[1].id.clone(), removed.items[0].id.clone()];
        let reordered = store.reorder_items(&playlist.id, &order).unwrap();
        assert_eq!(
            reordered.items.iter().map(|item| &item.id).collect::<Vec<_>>(),
            order.iter().collect::<Vec<_>>()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_item_snapshot_and_identity_survive_reload_in_order() {
        let (directory, store) = test_store("missing-reload");
        let playlist = store.create("Ausentes").unwrap();
        let added = store
            .add_items(
                &playlist.id,
                vec![
                    local_item("removed/track.flac", "Faixa removida"),
                    plex_item("/library/parts/404/file.flac", "Plex removida"),
                ],
            )
            .unwrap();
        let reloaded = PlaylistStore::new(store.path.clone()).get(&playlist.id).unwrap();
        assert_eq!(reloaded.items, added.items);
        assert_eq!(reloaded.items[0].metadata.title, "Faixa removida");
        assert_eq!(reloaded.items[1].metadata.title, "Plex removida");
        fs::remove_dir_all(directory).unwrap();
    }
}
