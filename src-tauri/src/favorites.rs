use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::plex::{contains_plex_token, legacy_plex_image_ref, PlexImageRef};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FavoriteAlbum {
    pub id: String,
    pub source: String,
    pub title: String,
    pub artist: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string")]
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plex_image: Option<PlexImageRef>,
    pub path_or_key: String,
    #[serde(default = "default_exists")]
    pub exists: bool,
}

fn default_exists() -> bool {
    true
}

fn deserialize_flexible_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<serde_json::Value>::deserialize(deserializer)?;
    match opt {
        Some(serde_json::Value::String(s)) => {
            if s.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(s))
            }
        }
        Some(serde_json::Value::Number(n)) => Ok(Some(n.to_string())),
        _ => Ok(None),
    }
}

fn check_local_path_exists(path_str: &str) -> bool {
    let p = Path::new(path_str);
    let full_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        crate::supervisor::MpdSupervisor::library_dir().join(p)
    };
    full_path.is_dir()
}

impl FavoriteAlbum {
    fn favorites_file_path() -> PathBuf {
        crate::persistence::sonante_config_dir().join("favorites.json")
    }

    fn sanitize_artwork(&mut self, selected_server_id: Option<&str>) -> bool {
        if self.source != "plex" {
            let mut changed = self.plex_image.take().is_some();
            if self.thumb.as_deref().is_some_and(contains_plex_token) {
                self.thumb = None;
                changed = true;
            }
            return changed;
        }

        let mut changed = false;
        if self.plex_image.as_ref().is_some_and(|image| {
            !image.is_valid()
                || selected_server_id.is_some_and(|server_id| image.server_id != server_id)
        }) {
            self.plex_image = None;
            changed = true;
        }
        if let Some(legacy_thumb) = self.thumb.take() {
            if self.plex_image.is_none() && contains_plex_token(&legacy_thumb) {
                self.plex_image = legacy_plex_image_ref(&legacy_thumb, selected_server_id);
            }
            changed = true;
        }
        changed
    }

    pub fn load_all(config: Option<&AppConfig>) -> Result<Vec<FavoriteAlbum>, String> {
        Self::load_all_from_path(&Self::favorites_file_path(), config)
    }

    fn load_all_from_path(path: &Path, config: Option<&AppConfig>) -> Result<Vec<FavoriteAlbum>, String> {
        crate::persistence::prepare_private_file_for_load(path, "favorites.json")?;
        let content = match fs::read(path) {
            Ok(content) => content,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("Falha ao ler favorites.json: {error}")),
        };
        let mut list: Vec<FavoriteAlbum> = serde_json::from_slice(&content)
            .map_err(|error| format!("favorites.json contém dados inválidos e foi preservado: {error}"))?;
        if list.iter().any(|fav| fav.id.trim().is_empty() || fav.source.trim().is_empty()) {
            return Err("favorites.json contém favoritos sem identidade válida e foi preservado.".to_string());
        }

        for fav in &mut list {
            if fav.source == "local" {
                fav.exists = check_local_path_exists(&fav.path_or_key);
            } else {
                fav.exists = true;
            }
        }

        // Sem configuração confiável, proteger a resposta em memória sem gravar migrações.
        let changed = list.iter_mut().fold(false, |changed, fav| {
            fav.sanitize_artwork(config.and_then(|config| config.plex_server_id.as_deref())) || changed
        });
        if config.is_some() && changed {
            Self::save_all_to_path(path, &list)?;
        }

        Ok(list)
    }

    fn save_all_to_path(path: &Path, list: &[FavoriteAlbum]) -> Result<(), String> {
        let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
        crate::persistence::atomic_write_private(path, json.as_bytes(), "favorites.json")
    }

    pub fn toggle(
        album: FavoriteAlbum,
        config: &AppConfig,
    ) -> Result<bool, String> {
        Self::toggle_at_path(&Self::favorites_file_path(), album, config)
    }

    fn toggle_at_path(
        path: &Path,
        mut album: FavoriteAlbum,
        config: &AppConfig,
    ) -> Result<bool, String> {
        let clean_id = album.id.trim().to_string();
        if clean_id.is_empty() {
            return Err("ID do álbum inválido".to_string());
        }

        album.sanitize_artwork(config.plex_server_id.as_deref());
        let mut list = Self::load_all_from_path(path, Some(config))?;
        let exists_index = list.iter().position(|item| item.id == clean_id && item.source == album.source);

        if let Some(idx) = exists_index {
            list.remove(idx);
            Self::save_all_to_path(path, &list)?;
            Ok(false)
        } else {
            let mut new_fav = album;
            new_fav.id = clean_id;
            if new_fav.source == "local" {
                new_fav.exists = check_local_path_exists(&new_fav.path_or_key);
            }
            list.push(new_fav);
            Self::save_all_to_path(path, &list)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_path() -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sonante-favorites-{}-{id}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        dir.join("favorites.json")
    }

    fn remove_test_path(path: &Path) {
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn plain_favorite() -> FavoriteAlbum {
        FavoriteAlbum {
            thumb: None,
            ..legacy_favorite()
        }
    }

    fn config_with_server() -> AppConfig {
        AppConfig {
            plex_server_id: Some("server-1".into()),
            ..AppConfig::default()
        }
    }

    fn favorite_with_artwork() -> FavoriteAlbum {
        FavoriteAlbum {
            thumb: None,
            plex_image: Some(PlexImageRef {
                server_id: "server-1".into(),
                path: "/library/metadata/42/thumb/1".into(),
            }),
            ..plain_favorite()
        }
    }

    #[test]
    fn absent_file_is_an_empty_list() {
        let path = test_path();
        assert!(FavoriteAlbum::load_all_from_path(&path, None).unwrap().is_empty());
        remove_test_path(&path);
    }

    #[test]
    fn valid_json_loads_and_normal_toggle_preserves_other_favorites() {
        let path = test_path();
        let config = config_with_server();
        let first = plain_favorite();
        fs::write(&path, serde_json::to_vec(&vec![first.clone()]).unwrap()).unwrap();
        let loaded = FavoriteAlbum::load_all_from_path(&path, Some(&config)).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, first.id);

        let mut second = plain_favorite();
        second.id = "43".into();
        assert!(FavoriteAlbum::toggle_at_path(&path, second.clone(), &config).unwrap());
        assert_eq!(FavoriteAlbum::load_all_from_path(&path, Some(&config)).unwrap().len(), 2);
        assert!(!FavoriteAlbum::toggle_at_path(&path, second, &config).unwrap());
        assert_eq!(FavoriteAlbum::load_all_from_path(&path, Some(&config)).unwrap().len(), 1);
        remove_test_path(&path);
    }

    #[test]
    fn valid_config_keeps_matching_plex_artwork_without_rewriting_file() {
        let path = test_path();
        let original = serde_json::to_vec(&vec![favorite_with_artwork()]).unwrap();
        fs::write(&path, &original).unwrap();
        let list = FavoriteAlbum::load_all_from_path(&path, Some(&config_with_server())).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].plex_image.is_some());
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn valid_config_without_server_preserves_stable_artwork() {
        let path = test_path();
        let original = serde_json::to_vec(&vec![favorite_with_artwork()]).unwrap();
        fs::write(&path, &original).unwrap();
        let list = FavoriteAlbum::load_all_from_path(&path, Some(&AppConfig::default())).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].plex_image.is_some());
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn valid_config_with_different_server_removes_stale_artwork() {
        let path = test_path();
        fs::write(&path, serde_json::to_vec(&vec![favorite_with_artwork()]).unwrap()).unwrap();
        let config = AppConfig {
            plex_server_id: Some("server-2".into()),
            ..AppConfig::default()
        };
        let list = FavoriteAlbum::load_all_from_path(&path, Some(&config)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].plex_image.is_none());
        assert_eq!(fs::read(&path).unwrap(), serde_json::to_vec_pretty(&list).unwrap());
        remove_test_path(&path);
    }

    #[test]
    fn invalid_config_does_not_change_valid_favorites_or_artwork() {
        let path = test_path();
        let config_path = path.parent().unwrap().join("config.json");
        fs::write(&config_path, b"{invalid config").unwrap();
        let original = serde_json::to_vec(&vec![favorite_with_artwork()]).unwrap();
        fs::write(&path, &original).unwrap();

        let config = AppConfig::load_from_path(&config_path).ok();
        assert!(config.is_none());
        let list = FavoriteAlbum::load_all_from_path(&path, config.as_ref()).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].plex_image.is_some());
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn unavailable_config_keeps_legacy_file_but_omits_authenticated_thumb_from_response() {
        let path = test_path();
        let original = serde_json::to_vec(&vec![legacy_favorite()]).unwrap();
        fs::write(&path, &original).unwrap();
        let list = FavoriteAlbum::load_all_from_path(&path, None).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].thumb.is_none());
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn invalid_json_blocks_toggle_without_changing_original_bytes() {
        let path = test_path();
        let original = b"{invalid favorites bytes";
        fs::write(&path, original).unwrap();
        assert!(FavoriteAlbum::load_all_from_path(&path, None).is_err());
        assert!(FavoriteAlbum::toggle_at_path(&path, plain_favorite(), &AppConfig::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn existing_unreadable_favorites_is_an_error() {
        let path = test_path();
        fs::create_dir(&path).unwrap();
        assert!(FavoriteAlbum::load_all_from_path(&path, None).is_err());
        assert!(FavoriteAlbum::toggle_at_path(&path, plain_favorite(), &AppConfig::default()).is_err());
        remove_test_path(&path);
    }

    fn legacy_favorite() -> FavoriteAlbum {
        FavoriteAlbum {
            id: "42".to_string(),
            source: "plex".to_string(),
            title: "Album".to_string(),
            artist: "Artist".to_string(),
            year: None,
            thumb: Some(
                "https://old.invalid/library/metadata/42/thumb/1?X-Plex-Token=SECRET"
                    .to_string(),
            ),
            plex_image: None,
            path_or_key: "42".to_string(),
            exists: true,
        }
    }

    #[test]
    fn legacy_authenticated_thumb_becomes_stable_reference() {
        let mut favorite = legacy_favorite();
        assert!(favorite.sanitize_artwork(Some("server-1")));
        assert_eq!(favorite.thumb, None);
        assert_eq!(
            favorite.plex_image,
            Some(PlexImageRef {
                server_id: "server-1".to_string(),
                path: "/library/metadata/42/thumb/1".to_string(),
            })
        );
        assert!(!serde_json::to_string(&favorite)
            .unwrap()
            .contains("X-Plex-Token"));
    }

    #[test]
    fn legacy_favorite_without_server_identity_is_preserved_without_artwork() {
        let mut favorite = legacy_favorite();
        assert!(favorite.sanitize_artwork(None));
        assert_eq!(favorite.id, "42");
        assert_eq!(favorite.thumb, None);
        assert_eq!(favorite.plex_image, None);
    }
}
