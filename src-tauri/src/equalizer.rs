//! Versioned PEQ product data. Loading it has no effect on audio playback.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_NAME: &str = "equalizer.json";
const SCHEMA_VERSION: u32 = 1;
const FLAT_ID: &str = "flat";
const MAX_USER_PRESETS: usize = 64;
const MAX_BANDS: usize = 64; // Allows imported/AutoEQ filters beyond the ten-band UI.
const MAX_NAME_CHARS: usize = 80;
const MAX_ID_CHARS: usize = 96;
const MAX_FILE_BYTES: u64 = 1_048_576;
const MIN_FREQUENCY_HZ: f64 = 20.0;
const MAX_FREQUENCY_HZ: f64 = 20_000.0;
const MIN_GAIN_DB: f64 = -12.0;
const MAX_GAIN_DB: f64 = 12.0;
const MIN_PREAMP_DB: f64 = -24.0;
const MAX_PREAMP_DB: f64 = 12.0;
// CamillaDSP 4.1.3 requires Q > 0 and declares no maximum. This is a product bound.
const MIN_Q: f64 = 0.1;
const MAX_Q: f64 = 30.0;
static NEXT_PRESET_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EqError {
    UnsupportedSchemaVersion(u32),
    InvalidJson,
    InvalidPresetName,
    DuplicatePresetName,
    InvalidPresetId,
    DuplicatePresetId,
    InvalidBandId,
    DuplicateBandId,
    UnknownBuiltinPreset,
    MissingActivePreset,
    UserPresetMissing,
    BuiltinReadOnly,
    InvalidFrequency,
    InvalidGain,
    InvalidQ,
    InvalidPreamp,
    InvalidBandCount,
    TooManyPresets,
    FileTooLarge,
    IdGenerationFailed,
    ReadFailed(String),
    WriteFailed(String),
    UnsafeOverwrite(Box<EqError>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PresetRef {
    BuiltIn { id: String },
    User { id: String },
}

impl PresetRef {
    fn flat() -> Self {
        Self::BuiltIn { id: FLAT_ID.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqFilterType {
    Peak,
    LowShelf,
    HighShelf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    pub id: String,
    pub enabled: bool,
    pub filter_type: EqFilterType,
    pub frequency_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqPreset {
    pub id: String,
    pub name: String,
    pub preamp_db: f64,
    pub bands: Vec<EqBand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqualizerConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub active_preset: PresetRef,
    pub user_presets: Vec<EqPreset>,
}

impl Default for EqualizerConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            enabled: false,
            active_preset: PresetRef::flat(),
            user_presets: Vec::new(),
        }
    }
}

pub fn flat_preset() -> EqPreset {
    const FREQUENCIES: [f64; 10] = [
        31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
    ];
    EqPreset {
        id: FLAT_ID.into(),
        name: "Flat".into(),
        preamp_db: 0.0,
        bands: FREQUENCIES
            .into_iter()
            .enumerate()
            .map(|(index, frequency_hz)| EqBand {
                id: format!("flat_{:02}", index + 1),
                enabled: true,
                filter_type: EqFilterType::Peak,
                frequency_hz,
                gain_db: 0.0,
                q: 1.0,
            })
            .collect(),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_CHARS
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn normalized_name(name: &str) -> Result<String, EqError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_NAME_CHARS
        || trimmed.chars().any(char::is_control)
    {
        return Err(EqError::InvalidPresetName);
    }
    Ok(trimmed.to_owned())
}

fn validate_preset(preset: &EqPreset) -> Result<(), EqError> {
    if !valid_id(&preset.id) {
        return Err(EqError::InvalidPresetId);
    }
    if normalized_name(&preset.name)? != preset.name {
        return Err(EqError::InvalidPresetName);
    }
    if !preset.preamp_db.is_finite() || !(MIN_PREAMP_DB..=MAX_PREAMP_DB).contains(&preset.preamp_db)
    {
        return Err(EqError::InvalidPreamp);
    }
    if preset.bands.is_empty() || preset.bands.len() > MAX_BANDS {
        return Err(EqError::InvalidBandCount);
    }
    let mut band_ids = HashSet::new();
    for band in &preset.bands {
        if !valid_id(&band.id) {
            return Err(EqError::InvalidBandId);
        }
        if !band_ids.insert(&band.id) {
            return Err(EqError::DuplicateBandId);
        }
        if !band.frequency_hz.is_finite()
            || !(MIN_FREQUENCY_HZ..=MAX_FREQUENCY_HZ).contains(&band.frequency_hz)
        {
            return Err(EqError::InvalidFrequency);
        }
        if !band.gain_db.is_finite() || !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&band.gain_db) {
            return Err(EqError::InvalidGain);
        }
        if !band.q.is_finite() || !(MIN_Q..=MAX_Q).contains(&band.q) {
            return Err(EqError::InvalidQ);
        }
    }
    Ok(())
}

fn new_preset_id() -> Result<String, EqError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| EqError::IdGenerationFailed)?
        .as_millis();
    let sequence = NEXT_PRESET_ID.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "eqp_{millis:x}_{:x}_{sequence:x}",
        std::process::id()
    ))
}

impl EqualizerConfig {
    pub fn validate(&self) -> Result<(), EqError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(EqError::UnsupportedSchemaVersion(self.schema_version));
        }
        if self.user_presets.len() > MAX_USER_PRESETS {
            return Err(EqError::TooManyPresets);
        }
        let (mut ids, mut names) = (HashSet::new(), HashSet::new());
        for preset in &self.user_presets {
            validate_preset(preset)?;
            if !ids.insert(&preset.id) {
                return Err(EqError::DuplicatePresetId);
            }
            if !names.insert(preset.name.to_lowercase()) {
                return Err(EqError::DuplicatePresetName);
            }
        }
        match &self.active_preset {
            PresetRef::BuiltIn { id } if id == FLAT_ID => Ok(()),
            PresetRef::BuiltIn { .. } => Err(EqError::UnknownBuiltinPreset),
            PresetRef::User { id } if !valid_id(id) || !ids.contains(id) => {
                Err(EqError::MissingActivePreset)
            }
            PresetRef::User { .. } => Ok(()),
        }
    }

    pub fn list_presets(&self) -> Vec<(PresetRef, EqPreset)> {
        std::iter::once((PresetRef::flat(), flat_preset()))
            .chain(self.user_presets.iter().cloned().map(|preset| {
                (
                    PresetRef::User {
                        id: preset.id.clone(),
                    },
                    preset,
                )
            }))
            .collect()
    }

    pub fn preset(&self, reference: &PresetRef) -> Result<EqPreset, EqError> {
        match reference {
            PresetRef::BuiltIn { id } if id == FLAT_ID => Ok(flat_preset()),
            PresetRef::BuiltIn { .. } => Err(EqError::UnknownBuiltinPreset),
            PresetRef::User { id } => self
                .user_presets
                .iter()
                .find(|preset| &preset.id == id)
                .cloned()
                .ok_or(EqError::UserPresetMissing),
        }
    }

    pub fn active(&self) -> Result<EqPreset, EqError> {
        self.preset(&self.active_preset)
    }

    fn edit<T>(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<T, EqError>,
    ) -> Result<T, EqError> {
        let mut candidate = self.clone();
        let result = change(&mut candidate)?;
        candidate.validate()?;
        *self = candidate;
        Ok(result)
    }

    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), EqError> {
        self.edit(|config| {
            config.enabled = enabled;
            Ok(())
        })
    }

    pub fn set_active_preset(&mut self, reference: PresetRef) -> Result<(), EqError> {
        self.edit(|config| {
            config.active_preset = reference;
            Ok(())
        })
    }

    pub fn create_user_preset(
        &mut self,
        name: &str,
        source: Option<&PresetRef>,
    ) -> Result<EqPreset, EqError> {
        let name = normalized_name(name)?;
        let source = self.preset(source.unwrap_or(&PresetRef::flat()))?;
        let mut preset = source;
        preset.id = new_preset_id()?;
        while self
            .user_presets
            .iter()
            .any(|existing| existing.id == preset.id)
        {
            preset.id = new_preset_id()?;
        }
        preset.name = name;
        self.edit(|config| {
            config.user_presets.push(preset.clone());
            Ok(preset)
        })
    }

    pub fn rename_user_preset(&mut self, id: &str, name: &str) -> Result<(), EqError> {
        let name = normalized_name(name)?;
        self.edit(|config| {
            let preset = config
                .user_presets
                .iter_mut()
                .find(|preset| preset.id == id)
                .ok_or(EqError::UserPresetMissing)?;
            preset.name = name;
            Ok(())
        })
    }

    pub fn update_user_preset(&mut self, id: &str, updated: EqPreset) -> Result<(), EqError> {
        if updated.id != id {
            return Err(EqError::InvalidPresetId);
        }
        self.edit(|config| {
            let preset = config
                .user_presets
                .iter_mut()
                .find(|preset| preset.id == id)
                .ok_or(EqError::UserPresetMissing)?;
            *preset = updated;
            Ok(())
        })
    }

    pub fn duplicate_preset(
        &mut self,
        source: &PresetRef,
        name: &str,
    ) -> Result<EqPreset, EqError> {
        self.create_user_preset(name, Some(source))
    }

    pub fn delete_preset(&mut self, reference: &PresetRef) -> Result<(), EqError> {
        let id = match reference {
            PresetRef::BuiltIn { .. } => return Err(EqError::BuiltinReadOnly),
            PresetRef::User { id } => id,
        };
        self.edit(|config| {
            let index = config
                .user_presets
                .iter()
                .position(|preset| &preset.id == id)
                .ok_or(EqError::UserPresetMissing)?;
            config.user_presets.remove(index);
            if config.active_preset == *reference {
                config.active_preset = PresetRef::flat();
            }
            Ok(())
        })
    }
}

pub struct EqualizerStore {
    path: PathBuf,
}

impl Default for EqualizerStore {
    fn default() -> Self {
        Self::new(crate::persistence::sonante_config_dir().join(FILE_NAME))
    }
}

impl EqualizerStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<EqualizerConfig, EqError> {
        crate::persistence::prepare_private_file_for_load(&self.path, FILE_NAME)
            .map_err(EqError::ReadFailed)?;
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(EqualizerConfig::default())
            }
            Err(error) => return Err(EqError::ReadFailed(error.to_string())),
        };
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| EqError::ReadFailed(error.to_string()))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(EqError::FileTooLarge);
        }
        let config: EqualizerConfig =
            serde_json::from_slice(&bytes).map_err(|_| EqError::InvalidJson)?;
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self, config: &EqualizerConfig) -> Result<(), EqError> {
        config.validate()?;
        // A present invalid file is evidence to preserve, never a default to replace.
        self.load()
            .map_err(|error| EqError::UnsafeOverwrite(Box::new(error)))?;
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|_| EqError::WriteFailed("encode equalizer.json".into()))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(EqError::FileTooLarge);
        }
        crate::persistence::atomic_write_private(&self.path, &bytes, FILE_NAME)
            .map_err(EqError::WriteFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestStore {
        dir: PathBuf,
        store: EqualizerStore,
    }

    impl TestStore {
        fn new() -> Self {
            let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("sonante-equalizer-{}-{id}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let store = EqualizerStore::new(dir.join(FILE_NAME));
            Self { dir, store }
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn user(config: &mut EqualizerConfig, name: &str) -> EqPreset {
        config.create_user_preset(name, None).unwrap()
    }

    #[test]
    fn absent_file_defaults_disabled_with_flat_builtin_ten_bands() {
        let fixture = TestStore::new();
        let config = fixture.store.load().unwrap();
        assert_eq!(config, EqualizerConfig::default());
        assert!(!config.enabled);
        assert_eq!(config.active_preset, PresetRef::flat());
        assert!(config.user_presets.is_empty());
        let flat = config.active().unwrap();
        assert_eq!(flat.name, "Flat");
        assert_eq!(flat.bands.len(), 10);
        assert_eq!(
            flat.bands
                .iter()
                .map(|band| band.frequency_hz)
                .collect::<Vec<_>>(),
            vec![31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0]
        );
        assert!(flat.bands.iter().all(|band| band.enabled
            && band.filter_type == EqFilterType::Peak
            && band.gain_db == 0.0
            && band.q == 1.0));
        assert_eq!(flat.preamp_db, 0.0);
        assert!(!fixture.store.path.exists());
    }

    #[test]
    fn json_schema_is_explicit_and_builtin_is_not_copied() {
        let fixture = TestStore::new();
        fixture.store.save(&EqualizerConfig::default()).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&fixture.store.path).unwrap()).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(
            json["active_preset"],
            serde_json::json!({"kind":"built_in","id":"flat"})
        );
        assert_eq!(json["user_presets"], serde_json::json!([]));
        let mut config = EqualizerConfig::default();
        let mut preset = user(&mut config, "Shelf");
        preset.bands[0].filter_type = EqFilterType::LowShelf;
        preset.bands[1].filter_type = EqFilterType::HighShelf;
        let id = preset.id.clone();
        config.update_user_preset(&id, preset).unwrap();
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(
            json["user_presets"][0]["bands"][0]["filter_type"],
            "low_shelf"
        );
        assert_eq!(
            json["user_presets"][0]["bands"][1]["filter_type"],
            "high_shelf"
        );
    }

    #[test]
    fn user_presets_roundtrip_and_band_order_is_preserved() {
        let fixture = TestStore::new();
        let mut config = EqualizerConfig::default();
        let first = user(&mut config, "Personal");
        let second = user(&mut config, "Headphones");
        let mut updated = first.clone();
        updated.bands.swap(0, 9);
        updated.bands.truncate(3);
        config
            .update_user_preset(&first.id, updated.clone())
            .unwrap();
        config
            .set_active_preset(PresetRef::User {
                id: second.id.clone(),
            })
            .unwrap();
        config.set_enabled(true).unwrap();
        fixture.store.save(&config).unwrap();
        assert_eq!(fixture.store.load().unwrap(), config);
        assert_eq!(
            fixture.store.load().unwrap().user_presets[0].bands,
            updated.bands
        );
        assert_eq!(config.list_presets().len(), 3);
    }

    #[test]
    fn numeric_bounds_are_inclusive_and_non_finite_values_are_rejected() {
        let mut config = EqualizerConfig::default();
        let preset = user(&mut config, "Numbers");
        let index = config
            .user_presets
            .iter()
            .position(|item| item.id == preset.id)
            .unwrap();
        let band = &mut config.user_presets[index].bands[0];
        band.frequency_hz = MIN_FREQUENCY_HZ;
        band.gain_db = MIN_GAIN_DB;
        band.q = MIN_Q;
        config.user_presets[index].preamp_db = MIN_PREAMP_DB;
        assert_eq!(config.validate(), Ok(()));
        let band = &mut config.user_presets[index].bands[0];
        band.frequency_hz = MAX_FREQUENCY_HZ;
        band.gain_db = MAX_GAIN_DB;
        band.q = MAX_Q;
        config.user_presets[index].preamp_db = MAX_PREAMP_DB;
        assert_eq!(config.validate(), Ok(()));
        for value in [19.99, 20_000.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            config.user_presets[index].bands[0].frequency_hz = value;
            assert_eq!(config.validate(), Err(EqError::InvalidFrequency));
        }
        config.user_presets[index].bands[0].frequency_hz = 1000.0;
        for value in [-12.01, 12.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            config.user_presets[index].bands[0].gain_db = value;
            assert_eq!(config.validate(), Err(EqError::InvalidGain));
        }
        config.user_presets[index].bands[0].gain_db = 0.0;
        for value in [
            0.0,
            0.099,
            30.001,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            config.user_presets[index].bands[0].q = value;
            assert_eq!(config.validate(), Err(EqError::InvalidQ));
        }
        config.user_presets[index].bands[0].q = 1.0;
        for value in [-24.01, 12.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            config.user_presets[index].preamp_db = value;
            assert_eq!(config.validate(), Err(EqError::InvalidPreamp));
        }
    }

    #[test]
    fn variable_band_count_and_duplicate_ids_are_validated() {
        let mut config = EqualizerConfig::default();
        let preset = user(&mut config, "Bands");
        let bands = &mut config.user_presets[0].bands;
        bands.clear();
        assert_eq!(config.validate(), Err(EqError::InvalidBandCount));
        config.user_presets[0].bands = preset.bands.clone();
        config.user_presets[0].bands.truncate(1);
        assert_eq!(config.validate(), Ok(()));
        let one = config.user_presets[0].bands[0].clone();
        for index in 1..MAX_BANDS {
            let mut band = one.clone();
            band.id = format!("band_{index}");
            config.user_presets[0].bands.push(band);
        }
        assert_eq!(config.validate(), Ok(()));
        config.user_presets[0].bands.push(one.clone());
        assert_eq!(config.validate(), Err(EqError::InvalidBandCount));
        config.user_presets[0].bands.pop();
        config.user_presets[0].bands[1].id = one.id.clone();
        assert_eq!(config.validate(), Err(EqError::DuplicateBandId));
        config.user_presets[0].bands[1].id.clear();
        assert_eq!(config.validate(), Err(EqError::InvalidBandId));
        config.user_presets[0].bands[1].id = "band_1".into();
        config.user_presets.push(config.user_presets[0].clone());
        assert_eq!(config.validate(), Err(EqError::DuplicatePresetId));
    }

    #[test]
    fn names_ids_references_and_preset_quota_are_validated() {
        let mut config = EqualizerConfig::default();
        let first = user(&mut config, "Rock");
        assert_eq!(
            config.create_user_preset(" rock ", None),
            Err(EqError::DuplicatePresetName)
        );
        assert_eq!(
            config.create_user_preset("ROCK", None),
            Err(EqError::DuplicatePresetName)
        );
        assert_eq!(
            config.create_user_preset(" ", None),
            Err(EqError::InvalidPresetName)
        );
        assert_eq!(
            config.create_user_preset(&"x".repeat(MAX_NAME_CHARS + 1), None),
            Err(EqError::InvalidPresetName)
        );
        let flat_named = user(&mut config, " Flat ");
        assert_eq!(flat_named.name, "Flat");
        assert_ne!(flat_named.id, FLAT_ID);
        config.user_presets[0].id.clear();
        assert_eq!(config.validate(), Err(EqError::InvalidPresetId));
        config.user_presets[0].id = first.id;
        config.active_preset = PresetRef::BuiltIn {
            id: "unknown".into(),
        };
        assert_eq!(config.validate(), Err(EqError::UnknownBuiltinPreset));
        config.active_preset = PresetRef::User {
            id: "missing".into(),
        };
        assert_eq!(config.validate(), Err(EqError::MissingActivePreset));
        config.active_preset = PresetRef::flat();
        for index in config.user_presets.len()..MAX_USER_PRESETS {
            let mut copy = flat_preset();
            copy.id = format!("user_{index}");
            copy.name = format!("User {index}");
            config.user_presets.push(copy);
        }
        assert_eq!(config.validate(), Ok(()));
        let mut excess = flat_preset();
        excess.id = "excess".into();
        excess.name = "Excess".into();
        config.user_presets.push(excess);
        assert_eq!(config.validate(), Err(EqError::TooManyPresets));
    }

    #[test]
    fn crud_is_transactional_and_builtins_are_read_only() {
        let mut config = EqualizerConfig::default();
        let first = config.create_user_preset(" First ", None).unwrap();
        assert_eq!(first.name, "First");
        config.rename_user_preset(&first.id, " Renamed ").unwrap();
        assert_eq!(config.user_presets[0].name, "Renamed");
        let mut updated = config.user_presets[0].clone();
        updated.bands[0].gain_db = -3.0;
        config
            .update_user_preset(&first.id, updated.clone())
            .unwrap();
        assert_eq!(config.user_presets[0], updated);
        let copy_flat = config
            .duplicate_preset(&PresetRef::flat(), "Flat copy")
            .unwrap();
        assert_ne!(copy_flat.id, FLAT_ID);
        let copy_user = config
            .duplicate_preset(
                &PresetRef::User {
                    id: first.id.clone(),
                },
                "User copy",
            )
            .unwrap();
        assert_ne!(copy_user.id, first.id);
        assert_eq!(copy_user.bands[0].gain_db, -3.0);
        assert_eq!(
            config.delete_preset(&PresetRef::flat()),
            Err(EqError::BuiltinReadOnly)
        );
        assert_eq!(
            config.rename_user_preset(FLAT_ID, "Changed"),
            Err(EqError::UserPresetMissing)
        );
        assert_eq!(
            config.update_user_preset(FLAT_ID, flat_preset()),
            Err(EqError::UserPresetMissing)
        );
        let before = config.clone();
        assert_eq!(
            config.set_active_preset(PresetRef::User {
                id: "missing".into()
            }),
            Err(EqError::MissingActivePreset)
        );
        assert_eq!(config, before);
        config
            .set_active_preset(PresetRef::User {
                id: first.id.clone(),
            })
            .unwrap();
        config.set_enabled(true).unwrap();
        config
            .delete_preset(&PresetRef::User { id: first.id })
            .unwrap();
        assert_eq!(config.active_preset, PresetRef::flat());
        assert!(config.enabled);
    }

    #[test]
    fn corrupt_and_future_schema_files_are_never_overwritten() {
        let fixture = TestStore::new();
        for (bytes, expected) in [
            (b"{bad json".to_vec(), EqError::InvalidJson),
            (br#"{"schema_version":999,"enabled":false,"active_preset":{"kind":"built_in","id":"flat"},"user_presets":[]}"#.to_vec(), EqError::UnsupportedSchemaVersion(999)),
        ] {
            fs::write(&fixture.store.path, &bytes).unwrap();
            assert_eq!(fixture.store.load(), Err(expected.clone()));
            assert_eq!(fixture.store.save(&EqualizerConfig::default()), Err(EqError::UnsafeOverwrite(Box::new(expected))));
            assert_eq!(fs::read(&fixture.store.path).unwrap(), bytes);
        }
    }

    #[test]
    fn dangling_reference_and_duplicate_names_on_disk_are_errors() {
        let fixture = TestStore::new();
        let mut config = EqualizerConfig::default();
        user(&mut config, "Rock");
        user(&mut config, "Jazz");
        config.user_presets[1].name = " rock ".into();
        fs::write(&fixture.store.path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(fixture.store.load(), Err(EqError::InvalidPresetName));
        config.user_presets[1].name = "ROCK".into();
        fs::write(&fixture.store.path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(fixture.store.load(), Err(EqError::DuplicatePresetName));
        config.user_presets[1].name = "Jazz".into();
        config.active_preset = PresetRef::User { id: "gone".into() };
        let invalid = serde_json::to_vec(&config).unwrap();
        fs::write(&fixture.store.path, &invalid).unwrap();
        assert_eq!(fixture.store.load(), Err(EqError::MissingActivePreset));
        assert_eq!(
            fixture.store.save(&EqualizerConfig::default()),
            Err(EqError::UnsafeOverwrite(Box::new(
                EqError::MissingActivePreset
            )))
        );
        assert_eq!(fs::read(&fixture.store.path).unwrap(), invalid);
    }

    #[test]
    fn true_absence_allows_atomic_private_initial_save_and_roundtrip() {
        let fixture = TestStore::new();
        let config = EqualizerConfig::default();
        fixture.store.save(&config).unwrap();
        assert_eq!(fixture.store.load().unwrap(), config);
        assert_eq!(fs::read_dir(&fixture.dir).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&fixture.store.path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&fixture.dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn unknown_fields_are_ignored_only_under_supported_schema_and_size_is_bounded() {
        let fixture = TestStore::new();
        let mut json = serde_json::to_value(EqualizerConfig::default()).unwrap();
        json["future_optional"] = serde_json::json!(true);
        fs::write(&fixture.store.path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(fixture.store.load().unwrap(), EqualizerConfig::default());
        json["schema_version"] = serde_json::json!(2);
        fs::write(&fixture.store.path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(
            fixture.store.load(),
            Err(EqError::UnsupportedSchemaVersion(2))
        );
        fs::write(&fixture.store.path, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert_eq!(fixture.store.load(), Err(EqError::FileTooLarge));
    }
}
