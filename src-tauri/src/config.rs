use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    #[serde(default = "default_first_run")]
    pub first_run: bool,
    pub alsa_device: String,
    
    // Múltiplas pastas locais
    #[serde(default)]
    pub local_folders: Vec<String>,

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
            alsa_device: "hw:CARD=R2R,DEV=0".to_string(),
            audio_output_type: "alsa".to_string(),
            local_folders: Vec::new(),
            plex_server_id: None,
            plex_server_name: None,
            plex_url: "http://192.168.1.100:32400".to_string(),
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
        let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("sonante");
        path.push("config.json");
        path
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&content) {
                    cfg.apply_legacy_migrations();
                    return cfg;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        
        // Grava primeiramente em arquivo temporário no mesmo diretório/sistema de arquivos
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, json).map_err(|e| e.to_string())?;

        // Operação atômica no nível do kernel (POSIX rename)
        fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;
        Ok(())
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
