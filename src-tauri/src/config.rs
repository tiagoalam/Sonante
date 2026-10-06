use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AppConfig {
    #[serde(default = "default_first_run")]
    pub first_run: bool,
    pub alsa_device: String,
    
    // Múltiplas pastas locais
    #[serde(default)]
    pub local_folders: Vec<String>,
    #[serde(default = "default_online_artwork_enabled")]
    pub online_artwork_enabled: bool,

    // Plex
    #[serde(default)]
    pub plex_server_id: Option<String>,
    #[serde(default)]
    pub plex_server_name: Option<String>,
    // Rota legada mantida para compatibilidade; não representa a identidade do servidor.
    pub plex_url: String,
    pub plex_token: String,
    pub playback_mode: String,      // "http" ou "local"
    pub local_mount_path: String,   // Legado / montagem Plex
    pub remote_share_path: String,  // Caminho no servidor Plex
    
    // Audiófilo
    #[serde(default = "default_audio_output_type")]
    pub audio_output_type: String,
    #[serde(default = "default_dop")]
    pub dop_enabled: bool,
    #[serde(default = "default_buffer_size")]
    pub audio_buffer_size_kb: u32,
    #[serde(default = "default_replay_gain")]
    pub replay_gain: String,
}

fn default_first_run() -> bool {
    true
}

fn default_online_artwork_enabled() -> bool {
    true
}

fn default_audio_output_type() -> String {
    "alsa".to_string()
}

fn default_dop() -> bool {
    true
}

fn default_buffer_size() -> u32 {
    16384
}

fn default_replay_gain() -> String {
    "off".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            first_run: true,
            alsa_device: "default".to_string(),
            audio_output_type: "pipewire".to_string(),
            local_folders: Vec::new(),
            online_artwork_enabled: true,
            plex_server_id: None,
            plex_server_name: None,
            plex_url: "".to_string(),
            plex_token: "".to_string(),
            playback_mode: "http".to_string(),
            local_mount_path: "".to_string(),
            remote_share_path: "".to_string(),
            dop_enabled: true,
            audio_buffer_size_kb: 16384,
            replay_gain: "off".to_string(),
        }
    }
}

impl AppConfig {
    fn config_path() -> PathBuf {
        crate::persistence::sonante_config_dir().join("config.json")
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from_path(&Self::config_path())
    }

    pub(crate) fn load_from_path(path: &Path) -> Result<Self, String> {
        crate::persistence::prepare_private_file_for_load(path, "config.json")?;
        let content = match fs::read(path) {
            Ok(content) => content,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("Falha ao ler config.json: {error}")),
        };
        let mut cfg: Self = serde_json::from_slice(&content)
            .map_err(|error| format!("config.json contém dados inválidos e foi preservado: {error}"))?;
        cfg.apply_legacy_migrations();
        Ok(cfg)
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to_path(&Self::config_path())
    }

    fn save_to_path(&self, path: &Path) -> Result<(), String> {
        // Nunca substitua um arquivo existente que não pôde ser carregado.
        Self::load_from_path(path)?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::persistence::atomic_write_private(path, json.as_bytes(), "config.json")
    }

    fn apply_legacy_migrations(&mut self) {
        // Migra configurações antigas com local_mount_path único para local_folders.
        if self.local_folders.is_empty() && !self.local_mount_path.is_empty() {
            self.local_folders.push(self.local_mount_path.clone());
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
        let dir = std::env::temp_dir().join(format!("sonante-config-{}-{id}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        dir.join("config.json")
    }

    fn remove_test_path(path: &Path) {
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    const LEGACY_CONFIG: &str = r#"{
        "first_run": false,
        "alsa_device": "default",
        "local_folders": [],
        "plex_url": "https://legacy-route.example.invalid:32400",
        "plex_token": "TEST_ACCOUNT_TOKEN",
        "playback_mode": "http",
        "local_mount_path": "/media/plex",
        "remote_share_path": "/music",
        "audio_output_type": "pipewire",
        "dop_enabled": false,
        "audio_buffer_size_kb": 8192,
        "replay_gain": "off"
    }"#;

    #[test]
    fn absent_file_loads_defaults() {
        let path = test_path();
        assert_eq!(AppConfig::load_from_path(&path).unwrap(), AppConfig::default());
        remove_test_path(&path);
    }

    #[test]
    fn defaults_are_portable_for_first_run() {
        let config = AppConfig::default();

        assert!(config.first_run);
        assert_eq!(config.audio_output_type, "pipewire");
        assert_eq!(config.alsa_device, "default");
        assert!(config.plex_url.is_empty());
        assert!(config.online_artwork_enabled);
    }

    #[test]
    fn valid_040_config_preserves_roots_and_plex() {
        let path = test_path();
        fs::write(&path, LEGACY_CONFIG).unwrap();
        let config = AppConfig::load_from_path(&path).unwrap();
        assert_eq!(config.local_folders, vec!["/media/plex"]);
        assert_eq!(config.plex_url, "https://legacy-route.example.invalid:32400");
        assert_eq!(config.plex_token, "TEST_ACCOUNT_TOKEN");
        assert!(config.online_artwork_enabled);
        remove_test_path(&path);
    }

    #[test]
    fn invalid_json_is_an_error_and_cannot_be_overwritten() {
        let path = test_path();
        let original = b"{invalid config bytes";
        fs::write(&path, original).unwrap();
        assert!(AppConfig::load_from_path(&path).is_err());
        assert!(AppConfig::default().save_to_path(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn existing_unreadable_config_is_an_error() {
        let path = test_path();
        fs::create_dir(&path).unwrap();
        assert!(AppConfig::load_from_path(&path).is_err());
        assert!(AppConfig::default().save_to_path(&path).is_err());
        remove_test_path(&path);
    }

    #[test]
    fn legacy_config_without_server_identity_still_deserializes() {
        let config: AppConfig = serde_json::from_str(LEGACY_CONFIG).unwrap();

        assert_eq!(config.plex_server_id, None);
        assert_eq!(config.plex_server_name, None);
        assert_eq!(
            config.plex_url,
            "https://legacy-route.example.invalid:32400"
        );
        assert_eq!(config.plex_token, "TEST_ACCOUNT_TOKEN");
    }

    #[test]
    fn server_identity_fields_survive_config_roundtrip() {
        let mut config = AppConfig::default();
        config.plex_server_id = Some("fixture-machine-id".to_string());
        config.plex_server_name = Some("Fixture Music Server".to_string());

        let serialized = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(restored.plex_server_id, config.plex_server_id);
        assert_eq!(restored.plex_server_name, config.plex_server_name);
        assert_eq!(restored.plex_url, config.plex_url);
        assert_eq!(restored.plex_token, config.plex_token);
    }

    #[test]
    fn legacy_migration_does_not_change_plex_server_or_route() {
        let mut config: AppConfig = serde_json::from_str(LEGACY_CONFIG).unwrap();
        let original_url = config.plex_url.clone();
        let original_token = config.plex_token.clone();

        config.apply_legacy_migrations();

        assert_eq!(config.plex_server_id, None);
        assert_eq!(config.plex_server_name, None);
        assert_eq!(config.plex_url, original_url);
        assert_eq!(config.plex_token, original_token);
        assert_eq!(config.local_folders, vec!["/media/plex"]);
    }
}
