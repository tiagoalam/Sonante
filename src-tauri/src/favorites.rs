use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FavoriteAlbum {
    pub id: String,
    pub source: String,
    pub title: String,
    pub artist: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string")]
    pub year: Option<String>,
    pub thumb: Option<String>,
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

    pub fn load_all() -> Vec<FavoriteAlbum> {
        let path = Self::favorites_file_path();
        if let Err(error) =
            crate::persistence::prepare_private_file_for_load(&path, "favorites.json")
        {
            eprintln!("[Persistência] {}", error);
        }
        if !path.exists() {
            return Vec::new();
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
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

        list
    }

    pub fn save_all(list: &[FavoriteAlbum]) -> Result<(), String> {
        let path = Self::favorites_file_path();
        let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
        crate::persistence::atomic_write_private(&path, json.as_bytes(), "favorites.json")
    }

    pub fn toggle(album: FavoriteAlbum) -> Result<bool, String> {
        let clean_id = album.id.trim().to_string();
        if clean_id.is_empty() {
            return Err("ID do álbum inválido".to_string());
        }

        let mut list = Self::load_all();
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
