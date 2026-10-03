use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

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
            !image.is_valid() || Some(image.server_id.as_str()) != selected_server_id
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

    pub fn load_all(selected_server_id: Option<&str>) -> Result<Vec<FavoriteAlbum>, String> {
        let path = Self::favorites_file_path();
        if let Err(error) =
            crate::persistence::prepare_private_file_for_load(&path, "favorites.json")
        {
            eprintln!("[Persistência] {}", error);
        }
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };

        let mut list: Vec<FavoriteAlbum> = serde_json::from_str(&content).unwrap_or_default();

        // Remove entradas fantasmas ou corrompidas com ID em branco
        list.retain(|fav| !fav.id.trim().is_empty() && !fav.source.trim().is_empty());

        for fav in &mut list {
            if fav.source == "local" {
                fav.exists = check_local_path_exists(&fav.path_or_key);
            } else {
                fav.exists = true;
            }
        }

        let changed = list
            .iter_mut()
            .fold(false, |changed, fav| fav.sanitize_artwork(selected_server_id) || changed);
        if changed {
            Self::save_all(&list)?;
        }

        Ok(list)
    }

    pub fn save_all(list: &[FavoriteAlbum]) -> Result<(), String> {
        let path = Self::favorites_file_path();
        let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
        crate::persistence::atomic_write_private(&path, json.as_bytes(), "favorites.json")
    }

    pub fn toggle(
        mut album: FavoriteAlbum,
        selected_server_id: Option<&str>,
    ) -> Result<bool, String> {
        let clean_id = album.id.trim().to_string();
        if clean_id.is_empty() {
            return Err("ID do álbum inválido".to_string());
        }

        album.sanitize_artwork(selected_server_id);
        let mut list = Self::load_all(selected_server_id)?;
        let exists_index = list.iter().position(|item| item.id == clean_id && item.source == album.source);

        if let Some(idx) = exists_index {
            list.remove(idx);
            Self::save_all(&list)?;
            Ok(false)
        } else {
            let mut new_fav = album;
            new_fav.id = clean_id;
            if new_fav.source == "local" {
                new_fav.exists = check_local_path_exists(&new_fav.path_or_key);
            }
            list.push(new_fav);
            Self::save_all(&list)?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
