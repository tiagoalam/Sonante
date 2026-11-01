use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigSaveError {
    BeforeRename(String),
    AfterRenameUnconfirmed(String),
}

impl ConfigSaveError {
    pub(crate) fn into_cause(self) -> String {
        match self {
            Self::BeforeRename(cause) | Self::AfterRenameUnconfirmed(cause) => cause,
        }
    }
}

impl From<String> for ConfigSaveError {
    fn from(cause: String) -> Self {
        Self::BeforeRename(cause)
    }
}

impl std::fmt::Display for ConfigSaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeRename(cause) => {
                write!(formatter, "config.json não foi substituído: {cause}")
            }
            Self::AfterRenameUnconfirmed(cause) => write!(
                formatter,
                "CONFIG_PERSISTENCE_UNCERTAIN: Estado incerto de config.json: a substituição pode ter ocorrido, mas a validação ou sincronização pós-rename falhou; commit e durabilidade não confirmados. {cause}"
            ),
        }
    }
}

impl std::error::Error for ConfigSaveError {}

// A declared volume policy for this configured route, never evidence of DAC
// identity, physical volume, or a new process's observed software gain.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DirectVolumeChoice {
    pub version: u8,
    pub endpoint: String,
    pub dop_enabled: bool,
}

fn is_false(value: &bool) -> bool { !*value }

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct EnabledSources { pub local: bool, pub plex: bool, pub navidrome: bool }
impl Default for EnabledSources {
    fn default() -> Self { Self { local: true, plex: true, navidrome: true } }
}
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct AppConfig {
    // Omission preserves the serialized shape of legacy recovery fingerprints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<EnabledSources>,
    #[serde(default = "default_first_run")]
    pub first_run: bool,
    pub alsa_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_volume_choice: Option<DirectVolumeChoice>,
    // Preserve the serialized shape used by legacy durable recovery fingerprints.
    #[serde(default, skip_serializing_if = "is_false")]
    pub direct_muted: bool,
    
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
            sources: None,
            alsa_device: "default".to_string(),
            direct_volume_choice: None, direct_muted:false,
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
    pub(crate) fn direct_context(&self) -> Option<DirectVolumeChoice> {
        (self.audio_output_type != "pipewire" && self.audio_output_type != "shared" && self.alsa_device != "default")
            .then(||DirectVolumeChoice {version:1,endpoint:self.alsa_device.clone(),dop_enabled:self.dop_enabled})
    }
    pub(crate) fn confirmed_direct_choice(&self) -> Option<DirectVolumeChoice> {
        self.direct_context().filter(|context| self.direct_volume_choice.as_ref()==Some(context))
    }
    pub(crate) fn validate_direct_choice(&self) -> Result<(),String> {
        if self.direct_volume_choice.is_some() && self.confirmed_direct_choice().is_none() {
            Err("DIRECT_VOLUME_CONTEXT_CHANGED".into())
        } else {Ok(())}
    }
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

    pub fn save(&self) -> Result<(), ConfigSaveError> {
        self.save_to_path(&Self::config_path())
    }

    fn save_to_path(&self, path: &Path) -> Result<(), ConfigSaveError> {
        // Nunca substitua um arquivo existente que não pôde ser carregado.
        Self::load_from_path(path)?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::persistence::atomic_write_private_synced(path, json.as_bytes(), "config.json")
    }

    pub(crate) fn apply_legacy_migrations(&mut self) {
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
    fn legacy_recovery_fingerprint_omits_absent_direct_metadata() {
        use sha2::{Digest, Sha256};
        let mut cfg: AppConfig=serde_json::from_str(LEGACY_CONFIG).unwrap();
        cfg.apply_legacy_migrations();
        let serialized=serde_json::to_vec(&cfg).unwrap();
        let text=std::str::from_utf8(&serialized).unwrap();
        assert!(!text.contains("direct_volume_choice"));
        assert!(!text.contains("direct_muted"));
        let expected: [u8;32]=Sha256::digest(&serialized).into();
        assert_eq!(crate::managed_save_intent::recovery_config_hash(LEGACY_CONFIG.as_bytes()).unwrap(),expected);
        let explicit_false=LEGACY_CONFIG.replacen('{',"{\"direct_muted\":false,",1);
        assert_eq!(crate::managed_save_intent::recovery_config_hash(explicit_false.as_bytes()).unwrap(),expected);
        cfg.direct_muted=true;
        let muted=serde_json::to_vec(&cfg).unwrap();
        assert_ne!(crate::managed_save_intent::recovery_config_hash(&muted).unwrap(),expected);
        assert!(serde_json::from_slice::<AppConfig>(&muted).unwrap().direct_muted);
    }

    #[test]
    fn direct_choice_migration_is_context_bound_and_mute_survives_durable_reopen() {
        let path=test_path();let mut cfg=AppConfig::default();cfg.first_run=false;cfg.audio_output_type="alsa".into();cfg.alsa_device="hw:CARD=Fixture,DEV=0".into();
        assert!(cfg.confirmed_direct_choice().is_none());
        cfg.direct_volume_choice=cfg.direct_context();cfg.direct_muted=true;cfg.validate_direct_choice().unwrap();cfg.save_to_path(&path).unwrap();
        let loaded=AppConfig::load_from_path(&path).unwrap();assert_eq!(loaded.confirmed_direct_choice(),cfg.direct_context());assert!(loaded.direct_muted);
        for changed in ["endpoint","dop","version","shared"] {
            let mut other=loaded.clone();match changed {"endpoint"=>other.alsa_device="hw:CARD=Other,DEV=0".into(),"dop"=>other.dop_enabled=!other.dop_enabled,"version"=>other.direct_volume_choice.as_mut().unwrap().version=2,_=>other.audio_output_type="pipewire".into()}
            assert!(other.confirmed_direct_choice().is_none());assert!(other.validate_direct_choice().is_err());
        }
        assert_eq!(AppConfig::load_from_path(&path).unwrap(),loaded);remove_test_path(&path);
    }

    #[test]
    fn absent_file_loads_defaults() {
        let path = test_path();
        assert_eq!(AppConfig::load_from_path(&path).unwrap(), AppConfig::default());
        remove_test_path(&path);
    }

    #[test]
    fn save_to_path_creates_and_replaces_config() {
        let path = test_path();
        let mut config = AppConfig::default();
        config.save_to_path(&path).unwrap();
        assert_eq!(AppConfig::load_from_path(&path).unwrap(), config);

        config.first_run = false;
        config.local_folders = vec!["/media/music".to_string()];
        config.save_to_path(&path).unwrap();
        assert_eq!(AppConfig::load_from_path(&path).unwrap(), config);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
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
        assert!(matches!(
            AppConfig::default().save_to_path(&path),
            Err(ConfigSaveError::BeforeRename(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        remove_test_path(&path);
    }

    #[test]
    fn existing_unreadable_config_is_an_error() {
        let path = test_path();
        fs::create_dir(&path).unwrap();
        assert!(AppConfig::load_from_path(&path).is_err());
        assert!(matches!(
            AppConfig::default().save_to_path(&path),
            Err(ConfigSaveError::BeforeRename(_))
        ));
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
