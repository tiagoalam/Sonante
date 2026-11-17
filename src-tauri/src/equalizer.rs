//! Versioned PEQ product data. Loading it has no effect on audio playback.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;
use std::sync::{atomic::{AtomicU64, Ordering}, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_NAME: &str = "equalizer-shared-v1.json";
const SCHEMA_VERSION: u32 = 1;
const FLAT_ID: &str = "flat";
const MAX_USER_PRESETS: usize = 64;
pub(crate) const MAX_BANDS: usize = 10; // Shared V1: bounded ten-band product UI.
const MAX_NAME_CHARS: usize = 80;
const MAX_ID_CHARS: usize = 96;
const MAX_FILE_BYTES: u64 = 1_048_576;
pub(crate) const MIN_FREQUENCY_HZ: f64 = 20.0;
pub(crate) const MAX_FREQUENCY_HZ: f64 = 20_000.0;
pub(crate) const MIN_GAIN_DB: f64 = -15.0;
pub(crate) const MAX_GAIN_DB: f64 = 15.0;
pub(crate) const MIN_PREAMP_DB: f64 = -24.0;
pub(crate) const MAX_PREAMP_DB: f64 = 12.0;
// CamillaDSP 4.1.3 requires Q > 0 and declares no maximum. This is a product bound.
pub(crate) const MIN_Q: f64 = 0.1;
pub(crate) const MAX_Q: f64 = 30.0;
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
    InvalidEquipment,
    PresetInUse,
    FileTooLarge,
    IdGenerationFailed,
    ReadFailed(String),
    WriteFailed(String),
    UnsafeOverwrite(Box<EqError>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct EqBand {
    pub id: String,
    pub enabled: bool,
    pub filter_type: EqFilterType,
    pub frequency_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqPreset {
    pub id: String,
    pub name: String,
    pub preamp_db: f64,
    pub bands: Vec<EqBand>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EquipmentProfile {
    pub id: String,
    pub name: String,
    pub preset: PresetRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EqualizerConfig {
    pub schema_version: u32,
    pub enabled: bool,
    pub active_preset: PresetRef,
    pub user_presets: Vec<EqPreset>,
    #[serde(default)]
    pub equipment_profiles: Vec<EquipmentProfile>,
}

impl Default for EqualizerConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            enabled: false,
            active_preset: PresetRef::flat(),
            user_presets: Vec::new(),
            equipment_profiles: Vec::new(),
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

/// Generic tonal preferences, never measured hardware correction.
pub(crate) fn built_in_presets() -> Vec<EqPreset> {
    let mut flat = flat_preset();
    for b in &mut flat.bands {
        b.enabled = false;
    }
    let mut presets = vec![flat];
    for (id, name, preamp, bands) in [
        // Broad listening preferences; band count follows tonal purpose, not a quota.
        (
            "gentle_bass_v2",
            "Gentle bass",
            0.0,
            vec![
                (EqFilterType::LowShelf, 72.0, 2.5, 0.707),
                (EqFilterType::Peak, 155.0, 1.1, 0.85),
                (EqFilterType::Peak, 320.0, -0.8, 0.85),
                (EqFilterType::Peak, 1000.0, -0.3, 0.8),
            ],
        ),
        (
            "voices_v2",
            "Voices",
            0.0,
            vec![
                (EqFilterType::LowShelf, 120.0, -1.2, 0.707),
                (EqFilterType::Peak, 320.0, -0.8, 0.8),
                (EqFilterType::Peak, 1200.0, 0.8, 0.8),
                (EqFilterType::Peak, 2300.0, 1.8, 0.9),
                (EqFilterType::Peak, 4200.0, -0.5, 0.9),
                (EqFilterType::HighShelf, 10000.0, 0.3, 0.707),
            ],
        ),
        (
            "smooth_v2",
            "Smooth",
            0.0,
            vec![
                (EqFilterType::LowShelf, 110.0, 0.4, 0.707),
                (EqFilterType::Peak, 250.0, -0.4, 0.85),
                (EqFilterType::Peak, 2800.0, -0.8, 0.85),
                (EqFilterType::Peak, 4800.0, -0.6, 0.9),
                (EqFilterType::HighShelf, 9000.0, -0.4, 0.707),
            ],
        ),
        (
            "light_loudness_v2",
            "Light loudness",
            0.0,
            vec![
                (EqFilterType::LowShelf, 80.0, 2.0, 0.707),
                (EqFilterType::Peak, 160.0, 0.8, 0.85),
                (EqFilterType::Peak, 400.0, -0.7, 0.8),
                (EqFilterType::Peak, 2000.0, 0.4, 0.8),
                (EqFilterType::HighShelf, 8500.0, 1.2, 0.707),
            ],
        ),
        (
            "style_rock_v2",
            "Rock",
            0.0,
            vec![
                (EqFilterType::LowShelf, 55.0, 2.5, 0.707),
                (EqFilterType::Peak, 110.0, 1.5, 0.9),
                (EqFilterType::Peak, 280.0, -1.5, 0.8),
                (EqFilterType::Peak, 1800.0, 0.8, 0.85),
                (EqFilterType::Peak, 3800.0, 0.8, 0.9),
                (EqFilterType::HighShelf, 9000.0, 0.7, 0.707),
            ],
        ),
        (
            "style_reggae_v2",
            "Reggae",
            0.0,
            vec![
                (EqFilterType::LowShelf, 60.0, 2.4, 0.707),
                (EqFilterType::Peak, 160.0, 0.8, 0.85),
                (EqFilterType::Peak, 350.0, -1.1, 0.8),
                (EqFilterType::Peak, 1800.0, 0.4, 0.85),
                (EqFilterType::Peak, 3800.0, 0.8, 0.9),
                (EqFilterType::HighShelf, 8000.0, 0.6, 0.707),
            ],
        ),
        (
            "style_jazz_v2",
            "Jazz",
            0.0,
            vec![
                (EqFilterType::LowShelf, 65.0, 0.8, 0.707),
                (EqFilterType::Peak, 220.0, -0.4, 0.85),
                (EqFilterType::Peak, 700.0, 0.3, 0.85),
                (EqFilterType::Peak, 1800.0, 0.7, 0.85),
                (EqFilterType::Peak, 4500.0, -0.4, 0.9),
                (EqFilterType::HighShelf, 10000.0, 0.5, 0.707),
            ],
        ),
        (
            "style_electronic_v2",
            "Electronic",
            0.0,
            vec![
                (EqFilterType::LowShelf, 40.0, 3.0, 0.707),
                (EqFilterType::Peak, 110.0, 1.2, 0.9),
                (EqFilterType::Peak, 280.0, -1.0, 0.8),
                (EqFilterType::Peak, 1100.0, -0.4, 0.85),
                (EqFilterType::Peak, 3500.0, 0.8, 0.9),
                (EqFilterType::HighShelf, 10000.0, 1.2, 0.707),
            ],
        ),
        (
            "style_pop_v2",
            "Pop",
            0.0,
            vec![
                (EqFilterType::LowShelf, 70.0, 1.5, 0.707),
                (EqFilterType::Peak, 180.0, -0.7, 0.85),
                (EqFilterType::Peak, 700.0, -0.5, 0.85),
                (EqFilterType::Peak, 2500.0, 1.4, 0.9),
                (EqFilterType::Peak, 4500.0, 0.3, 0.9),
                (EqFilterType::HighShelf, 9500.0, 0.9, 0.707),
            ],
        ),
        (
            "style_acoustic_v2",
            "Acoustic",
            0.0,
            vec![
                (EqFilterType::LowShelf, 100.0, 0.5, 0.707),
                (EqFilterType::Peak, 300.0, -0.8, 0.85),
                (EqFilterType::Peak, 900.0, 0.5, 0.85),
                (EqFilterType::Peak, 2600.0, 1.0, 0.9),
                (EqFilterType::Peak, 5500.0, -0.4, 0.9),
                (EqFilterType::HighShelf, 10000.0, 0.6, 0.707),
            ],
        ),
        (
            "mood_balanced_v2",
            "Balanced",
            0.0,
            vec![
                (EqFilterType::LowShelf, 70.0, 0.6, 0.707),
                (EqFilterType::Peak, 220.0, -0.3, 0.85),
                (EqFilterType::Peak, 1500.0, 0.5, 0.85),
                (EqFilterType::Peak, 5000.0, 0.4, 0.9),
                (EqFilterType::HighShelf, 11000.0, 0.4, 0.707),
            ],
        ),
        (
            "mood_relaxing_v2",
            "Relaxing",
            0.0,
            vec![
                (EqFilterType::LowShelf, 100.0, 0.5, 0.707),
                (EqFilterType::Peak, 300.0, 0.3, 0.85),
                (EqFilterType::Peak, 2700.0, -1.0, 0.85),
                (EqFilterType::Peak, 5000.0, -0.8, 0.9),
                (EqFilterType::HighShelf, 10000.0, -0.4, 0.707),
            ],
        ),
        (
            "mood_energetic_v2",
            "Energetic",
            0.0,
            vec![
                (EqFilterType::LowShelf, 70.0, 2.0, 0.707),
                (EqFilterType::Peak, 180.0, -0.7, 0.85),
                (EqFilterType::Peak, 800.0, 0.3, 0.85),
                (EqFilterType::Peak, 2500.0, 1.3, 0.9),
                (EqFilterType::Peak, 5200.0, 0.5, 0.9),
                (EqFilterType::HighShelf, 10000.0, 0.7, 0.707),
            ],
        ),
        (
            "mood_cozy_v2",
            "Cozy",
            0.0,
            vec![
                (EqFilterType::LowShelf, 120.0, 1.5, 0.707),
                (EqFilterType::Peak, 280.0, 0.5, 0.85),
                (EqFilterType::Peak, 800.0, -0.5, 0.85),
                (EqFilterType::Peak, 2800.0, -0.8, 0.9),
                (EqFilterType::Peak, 5200.0, -0.6, 0.9),
                (EqFilterType::HighShelf, 8000.0, -0.7, 0.707),
            ],
        ),
    ] {
        let mut p = flat_preset();
        p.id = id.into();
        p.name = name.into();
        p.preamp_db = preamp;
        for b in &mut p.bands {
            b.enabled = false
        }
        for (slot, (kind, freq, gain, q)) in p.bands.iter_mut().zip(bands) {
            slot.enabled = true;
            slot.filter_type = kind;
            slot.frequency_hz = freq;
            slot.gain_db = gain;
            slot.q = q;
        }
        presets.push(p);
    }
    // Explicit ten-band tonal curves. The preamp is lower than the sum of
    // positive band gains by 1 dB, a conservative bound against stacked boosts.
    const TONE_FREQUENCIES: [f64; 10] = [31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
    let tone_curves: [(&str, &str, [f64; 10]); 4] = [
        ("tone_classic_v3", "Classic", [0.8, 0.6, 0.3, 0.0, -0.1, 0.0, 0.1, 0.3, 0.4, 0.3]),
        ("tone_dance_v3", "Dance", [1.2, 1.0, 0.5, 0.0, -0.2, -0.1, 0.1, 0.3, 0.5, 0.3]),
        ("tone_hip_hop_v3", "Hip-Hop", [1.0, 0.7, 0.3, -0.3, -0.4, -0.1, 0.2, 0.5, 0.4, 0.1]),
        ("tone_rnb_v3", "R&B", [0.7, 0.6, 0.4, 0.0, -0.1, 0.0, 0.2, 0.3, 0.2, 0.0]),
    ];
    for (id, name, gains) in tone_curves {
        let mut preset = flat_preset();
        preset.id = id.into();
        preset.name = name.into();
        let positive_gain_cents = gains.iter().filter(|gain| **gain > 0.0)
            .map(|gain| (gain * 100.0).round() as i64).sum::<i64>();
        preset.preamp_db = -((positive_gain_cents + 100) as f64 / 100.0);
        for (index, (band, (frequency, gain))) in preset.bands.iter_mut().zip(TONE_FREQUENCIES.into_iter().zip(gains)).enumerate() {
            band.id = format!("{id}_{:02}", index + 1);
            band.enabled = true;
            band.filter_type = EqFilterType::Peak;
            band.frequency_hz = frequency;
            band.gain_db = gain;
            band.q = 1.0;
        }
        presets.push(preset);
    }
    presets
}

/// Presentation metadata only; never persisted or interpreted as stacked DSP.
#[derive(serde::Serialize)]
pub(crate) struct BuiltinGroup {
    id: &'static str,
    preset_ids: Vec<String>,
}
fn legacy_builtin_presets() -> &'static [EqPreset] {
    static PRESETS: OnceLock<Vec<EqPreset>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        serde_json::from_str(include_str!("equalizer_builtin_legacy_v1.json"))
            .expect("embedded legacy equalizer catalog is valid")
    })
}
// Keep old saved selections resolvable with their original coefficients.
// They are only surfaced in the existing legacy group when already selected.
fn legacy_tone_preset(id: &str) -> Option<EqPreset> {
    let (name, bands): (&str, &[(EqFilterType, f64, f64, f64)]) = match id {
        "k17_classic_ref_v1" => ("Classic", &[
            (EqFilterType::LowShelf, 88.0, 2.0, 0.707), (EqFilterType::Peak, 170.0, 0.7, 0.9),
            (EqFilterType::Peak, 380.0, -0.6, 0.85), (EqFilterType::Peak, 1500.0, 0.4, 0.9),
            (EqFilterType::Peak, 3600.0, 0.8, 0.95), (EqFilterType::HighShelf, 9200.0, 0.9, 0.707),
        ]),
        "k17_dance_ref_v1" => ("Dance", &[
            (EqFilterType::LowShelf, 54.0, 2.8, 0.707), (EqFilterType::Peak, 105.0, 1.8, 0.9),
            (EqFilterType::Peak, 235.0, 0.8, 0.9), (EqFilterType::Peak, 520.0, -0.5, 0.8),
            (EqFilterType::Peak, 1450.0, 0.3, 0.85), (EqFilterType::Peak, 4100.0, 0.8, 0.9),
            (EqFilterType::HighShelf, 10000.0, 1.1, 0.707),
        ]),
        "k17_pop_ref_v1" => ("Pop", &[
            (EqFilterType::LowShelf, 58.0, 1.0, 0.707), (EqFilterType::Peak, 130.0, 0.4, 0.9),
            (EqFilterType::Peak, 310.0, -0.7, 0.85), (EqFilterType::Peak, 780.0, 0.5, 0.85),
            (EqFilterType::Peak, 2500.0, 1.3, 0.9), (EqFilterType::Peak, 4500.0, 0.5, 0.9),
            (EqFilterType::HighShelf, 9800.0, 0.7, 0.707),
        ]),
        "k17_hip_hop_ref_v1" => ("Hip-Hop", &[
            (EqFilterType::LowShelf, 44.0, 2.7, 0.707), (EqFilterType::Peak, 88.0, 1.2, 0.9),
            (EqFilterType::Peak, 245.0, -0.7, 0.85), (EqFilterType::Peak, 680.0, -0.3, 0.85),
            (EqFilterType::Peak, 1800.0, 0.2, 0.9), (EqFilterType::Peak, 4200.0, 0.6, 0.9),
            (EqFilterType::HighShelf, 9200.0, 0.4, 0.707),
        ]),
        "k17_rnb_ref_v1" => ("R&B", &[
            (EqFilterType::LowShelf, 68.0, 2.4, 0.707), (EqFilterType::Peak, 142.0, 1.4, 0.9),
            (EqFilterType::Peak, 320.0, 0.7, 0.85), (EqFilterType::Peak, 720.0, -0.3, 0.85),
            (EqFilterType::Peak, 1700.0, 0.4, 0.9), (EqFilterType::Peak, 3900.0, 0.4, 0.9),
            (EqFilterType::HighShelf, 9000.0, 0.5, 0.707),
        ]),
        "k17_rock_ref_v1" => ("Rock", &[
            (EqFilterType::LowShelf, 72.0, 1.7, 0.707), (EqFilterType::Peak, 155.0, 0.8, 0.9),
            (EqFilterType::Peak, 430.0, -1.0, 0.85), (EqFilterType::Peak, 1300.0, -0.5, 0.9),
            (EqFilterType::Peak, 2800.0, 1.0, 0.9), (EqFilterType::Peak, 4700.0, 0.8, 0.95),
            (EqFilterType::HighShelf, 8500.0, 0.5, 0.707),
        ]),
        "k17_jazz_ref_v1" => ("Jazz", &[
            (EqFilterType::LowShelf, 68.0, 0.6, 0.707), (EqFilterType::Peak, 220.0, 0.3, 0.85),
            (EqFilterType::Peak, 520.0, -0.3, 0.85), (EqFilterType::Peak, 1450.0, 0.4, 0.85),
            (EqFilterType::Peak, 3200.0, 0.5, 0.9), (EqFilterType::Peak, 6200.0, -0.3, 0.9),
            (EqFilterType::HighShelf, 10000.0, -0.2, 0.707),
        ]),
        _ => return None,
    };
    let mut preset = flat_preset();
    preset.id = id.into(); preset.name = name.into(); preset.preamp_db = 0.0;
    for band in &mut preset.bands { band.enabled = false; }
    for (band, (kind, frequency, gain, q)) in preset.bands.iter_mut().zip(bands.iter().copied()) {
        band.enabled = true; band.filter_type = kind; band.frequency_hz = frequency; band.gain_db = gain; band.q = q;
    }
    Some(preset)
}
fn retired_generic_tone_preset(id: &str) -> Option<EqPreset> {
    const FREQUENCIES: [f64; 10] = [31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
    let gains: [f64; 10] = match id {
        "tone_pop_v3" => [0.5, 0.4, 0.1, -0.1, -0.1, 0.0, 0.4, 0.7, 0.4, 0.2],
        "tone_rock_v3" => [0.8, 0.5, 0.1, -0.3, -0.4, 0.1, 0.6, 0.8, 0.5, 0.2],
        "tone_jazz_v3" => [0.3, 0.2, 0.0, -0.1, 0.0, 0.1, 0.3, 0.3, 0.1, -0.1],
        _ => return None,
    };
    let mut preset = flat_preset();
    preset.id = id.to_owned();
    preset.name = "Previously saved tone".into();
    let positive_gain_cents = gains
        .iter()
        .filter(|gain| **gain > 0.0)
        .map(|gain| (gain * 100.0).round() as i64)
        .sum::<i64>();
    preset.preamp_db = -((positive_gain_cents + 100) as f64 / 100.0);
    for (index, (band, (frequency, gain))) in preset
        .bands
        .iter_mut()
        .zip(FREQUENCIES.into_iter().zip(gains))
        .enumerate()
    {
        band.id = format!("{id}_{:02}", index + 1);
        band.enabled = true;
        band.filter_type = EqFilterType::Peak;
        band.frequency_hz = frequency;
        band.gain_db = gain;
        band.q = 1.0;
    }
    Some(preset)
}
fn builtin_preset(id: &str) -> Option<EqPreset> {
    let presets = built_in_presets();
    if let Some(preset) = presets.iter().find(|preset| preset.id == id) {
        return Some(preset.clone());
    }
    if let Some(preset) = retired_generic_tone_preset(id) {
        return Some(preset);
    }
    if let Some(preset) = legacy_tone_preset(id) {
        return Some(preset);
    }
    legacy_builtin_presets().iter().find(|preset| preset.id == id).cloned()
}
pub(crate) fn catalog_built_ins(config: &EqualizerConfig) -> Vec<EqPreset> {
    let mut presets = built_in_presets();
    let mut referenced = Vec::new();
    if let PresetRef::BuiltIn { id } = &config.active_preset {
        referenced.push(id.as_str());
    }
    referenced.extend(config.equipment_profiles.iter().filter_map(|profile| match &profile.preset {
        PresetRef::BuiltIn { id } => Some(id.as_str()),
        PresetRef::User { .. } => None,
    }));
    for id in referenced {
        if presets.iter().any(|preset| preset.id == id) {
            continue;
        }
        if let Some(preset) = builtin_preset(id) {
            presets.push(preset);
        }
    }
    presets
}
pub(crate) fn builtin_groups(config: &EqualizerConfig) -> Vec<BuiltinGroup> {
    [
        ("reference", vec!["flat"]),
        (
            "styles",
            vec![
                "style_rock_v2",
                "style_reggae_v2",
                "style_jazz_v2",
                "style_electronic_v2",
                "style_pop_v2",
                "style_acoustic_v2",
                "tone_classic_v3",
                "tone_dance_v3",
                "tone_hip_hop_v3",
                "tone_rnb_v3",
            ],
        ),
        (
            "moods",
            vec![
                "mood_balanced_v2",
                "mood_relaxing_v2",
                "mood_energetic_v2",
                "mood_cozy_v2",
                "voices_v2",
            ],
        ),
        ("tonal", vec!["gentle_bass_v2", "smooth_v2", "light_loudness_v2"]),
    ]
    .into_iter()
    .map(|(id, preset_ids)| BuiltinGroup {
        id,
        preset_ids: preset_ids.into_iter().map(str::to_owned).collect(),
    })
    .chain(std::iter::once_with(|| {
        let mut preset_ids = Vec::new();
        if let PresetRef::BuiltIn { id } = &config.active_preset {
            if id != FLAT_ID && (legacy_builtin_presets().iter().any(|preset| preset.id == *id) || legacy_tone_preset(id).is_some() || retired_generic_tone_preset(id).is_some()) {
                preset_ids.push(id.clone());
            }
        }
        for profile in &config.equipment_profiles {
            if let PresetRef::BuiltIn { id } = &profile.preset {
                if id != FLAT_ID
                    && (legacy_builtin_presets().iter().any(|preset| preset.id == *id) || legacy_tone_preset(id).is_some() || retired_generic_tone_preset(id).is_some())
                    && !preset_ids.contains(id)
                {
                    preset_ids.push(id.clone());
                }
            }
        }
        BuiltinGroup { id: "legacy", preset_ids }
    }))
    .filter(|group| group.id != "legacy" || !group.preset_ids.is_empty())
    .collect()
}

#[cfg(test)]
mod collection_tests {
    use super::*;

    fn response_db(preset: &EqPreset, rate: u32, frequency: f64) -> f64 {
        let omega = 2. * std::f64::consts::PI * frequency / rate as f64;
        let (sin1, cos1) = omega.sin_cos();
        let (sin2, cos2) = (2. * omega).sin_cos();
        let mut magnitude = 1.;
        for band in preset.bands.iter().filter(|band| band.enabled) {
            let [b0, b1, b2, a1, a2] = crate::shared_dsp::safety::coefficients(band, rate);
            let numerator_re = b0 + b1 * cos1 + b2 * cos2;
            let numerator_im = -b1 * sin1 - b2 * sin2;
            let denominator_re = 1. + a1 * cos1 + a2 * cos2;
            let denominator_im = -a1 * sin1 - a2 * sin2;
            magnitude *= numerator_re.hypot(numerator_im) / denominator_re.hypot(denominator_im);
        }
        20. * magnitude.log10()
    }

    #[test]
    fn versioned_catalog_keeps_existing_builtin_references_and_personal_copies_unchanged() {
        let current = built_in_presets();
        let legacy = legacy_builtin_presets();
        assert_eq!(current.len(), 19);
        assert_eq!(legacy.len(), 15);
        assert_eq!(current[0], legacy[0]);
        assert!(current.iter().skip(1).all(|p| p.id.ends_with("_v2") || p.id.ends_with("_v3")));

        for old in legacy.iter().filter(|p| p.id != FLAT_ID) {
            let raw = serde_json::json!({
                "schema_version":1,"enabled":false,
                "active_preset":{"kind":"built_in","id":old.id},
                "user_presets":[]
            });
            let mut config: EqualizerConfig = serde_json::from_value(raw).unwrap();
            config.validate().unwrap();
            let reference = config.active_preset.clone();
            assert_eq!(config.preset(&reference).unwrap(), *old);
            let catalog = catalog_built_ins(&config);
            assert_eq!(catalog.iter().find(|preset| preset.id == old.id), Some(old));
            let groups = builtin_groups(&config);
            assert!(groups.iter().any(|group| group.id == "legacy" && group.preset_ids.contains(&old.id)));

            config.link_equipment(None, "Existing headphones", reference.clone()).unwrap();
            let copy = config.create_user_preset("Personal copy", Some(&reference)).unwrap();
            assert_eq!(config.preset(&PresetRef::User { id: copy.id }).unwrap().bands, old.bands);
        }
    }

    #[test]
    fn presentation_groups_cover_every_builtin_once_and_imports_remain_portable() {
        let presets = built_in_presets();
        let config = EqualizerConfig::default();
        let grouped: Vec<_> = builtin_groups(&config).into_iter().flat_map(|g| g.preset_ids).collect();
        let unique: std::collections::HashSet<_> = grouped.iter().cloned().collect();
        assert_eq!(grouped.len(), presets.len());
        assert_eq!(unique.len(), presets.len());
        for p in presets {
            assert!(unique.contains(&p.id));
            let text = crate::equalizer_portable::export(&p).unwrap();
            let imported = crate::equalizer_portable::inspect(&text).unwrap();
            assert_eq!(imported.preamp_db, p.preamp_db);
            for (a,b) in imported.bands.iter().zip(&p.bands) {
                assert_eq!((a.enabled,a.filter_type,a.frequency_hz,a.gain_db,a.q),(b.enabled,b.filter_type,b.frequency_hz,b.gain_db,b.q));
            }
            let mut config = EqualizerConfig::default();
            let user = config.create_user_preset(&p.name, Some(&PresetRef::BuiltIn { id:p.id.clone() })).unwrap();
            config.link_equipment(None, "Test DAC", PresetRef::User { id:user.id.clone() }).unwrap();
            config.set_active_preset(PresetRef::User { id:user.id }).unwrap();
            assert!(!config.enabled);
            assert_eq!(built_in_presets().iter().find(|v|v.id==p.id),Some(&p));
        }
    }

    #[test]
    fn new_rhythm_presets_are_unique_styles_and_keep_safe_headroom() {
        let config = EqualizerConfig::default();
        assert!(!config.enabled, "catalog changes must not activate EQ");
        let catalog = catalog_built_ins(&config);
        let new_styles = ["tone_classic_v3", "tone_dance_v3", "tone_hip_hop_v3", "tone_rnb_v3"];
        let styles = builtin_groups(&config).into_iter().find(|group| group.id == "styles").unwrap();
        for id in new_styles {
            assert!(styles.preset_ids.contains(&id.to_owned()), "{id} is part of the normal styles category");
            let preset = catalog.iter().find(|preset| preset.id == id).unwrap();
            assert!(preset.bands.iter().all(|band| band.enabled));
            assert_eq!(preset.bands.len(), 10);
            let positive = preset.bands.iter().map(|band| band.gain_db.max(0.0)).sum::<f64>();
            assert!((preset.preamp_db + positive + 1.0).abs() < 1e-9);
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let protection = crate::shared_dsp::safety::assess(preset, rate).unwrap();
                let peak = (0..=2048).map(|index| {
                    let frequency = 20.0_f64 * (20_000.0_f64 / 20.0_f64).powf(index as f64 / 2048.0);
                    response_db(preset, rate, frequency)
                }).fold(f64::NEG_INFINITY, f64::max);
                assert!(protection.effective_preamp_db + peak + protection.resampler_bound_db <= protection.ceiling_db + 1e-9);
            }
        }
        assert!(!builtin_groups(&config).iter().any(|group| group.id == "tones"));
        let names: Vec<_> = catalog.iter().map(|preset| preset.name.to_lowercase()).collect();
        assert_eq!(names.iter().filter(|name| name.as_str() == "rock").count(), 1);
        assert_eq!(names.iter().filter(|name| name.as_str() == "pop").count(), 1);
        assert_eq!(names.iter().filter(|name| name.as_str() == "jazz").count(), 1);
    }

    #[test]
    fn retired_duplicate_tones_keep_saved_coefficients_without_becoming_duplicate_choices() {
        let normal = built_in_presets();
        for (id, original) in [("tone_pop_v3", "style_pop_v2"), ("tone_rock_v3", "style_rock_v2"), ("tone_jazz_v3", "style_jazz_v2")] {
            assert!(!normal.iter().any(|preset| preset.id == id));
            let mut config = EqualizerConfig::default();
            config.active_preset = PresetRef::BuiltIn { id: id.into() };
            let catalog = catalog_built_ins(&config);
            let saved = catalog.iter().find(|preset| preset.id == id).expect("saved selection remains resolvable");
            assert_eq!(saved.name, "Previously saved tone");
            let expected_gains: [f64; 10] = match id {
                "tone_pop_v3" => [0.5, 0.4, 0.1, -0.1, -0.1, 0.0, 0.4, 0.7, 0.4, 0.2],
                "tone_rock_v3" => [0.8, 0.5, 0.1, -0.3, -0.4, 0.1, 0.6, 0.8, 0.5, 0.2],
                "tone_jazz_v3" => [0.3, 0.2, 0.0, -0.1, 0.0, 0.1, 0.3, 0.3, 0.1, -0.1],
                _ => unreachable!(),
            };
            assert_eq!(saved.bands.len(), expected_gains.len());
            assert_eq!(saved.bands.iter().map(|band| band.gain_db).collect::<Vec<_>>(), expected_gains);
            assert!(saved.bands.iter().all(|band| band.filter_type == EqFilterType::Peak && band.q == 1.0));
            assert_eq!(saved.bands.iter().map(|band| band.frequency_hz).collect::<Vec<_>>(), [31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0]);
            assert!(catalog.iter().any(|preset| preset.id == original));
            assert!(builtin_groups(&config).iter().any(|group| group.id == "legacy" && group.preset_ids.contains(&id.to_owned())));
        }
    }

    #[test]
    fn old_factory_tone_references_resolve_to_generic_curves_without_changing_saved_selections() {
        let aliases = [
            ("k17_classic_ref_v1", "tone_classic_v3"), ("k17_dance_ref_v1", "tone_dance_v3"),
            ("k17_pop_ref_v1", "tone_pop_v3"), ("k17_hip_hop_ref_v1", "tone_hip_hop_v3"),
            ("k17_rnb_ref_v1", "tone_rnb_v3"), ("k17_rock_ref_v1", "tone_rock_v3"),
            ("k17_jazz_ref_v1", "tone_jazz_v3"),
        ];
        for (old_id, new_id) in aliases {
            let mut config = EqualizerConfig::default();
            config.active_preset = PresetRef::BuiltIn { id: old_id.into() };
            let catalog = catalog_built_ins(&config);
            let old = catalog.iter().find(|preset| preset.id == old_id).unwrap();
            assert_eq!(old.name, legacy_tone_preset(old_id).unwrap().name);
            assert_eq!(old.preamp_db, 0.0);
            assert!((6..=7).contains(&old.bands.iter().filter(|band| band.enabled).count()));
            if let Some(current) = built_in_presets().into_iter().find(|preset| preset.id == new_id) {
                assert_ne!(old.bands, current.bands, "legacy selections keep their original coefficients");
            }
            assert!(builtin_groups(&config).iter().any(|group| group.id == "legacy" && group.preset_ids.contains(&old_id.into())));
        }
    }

    #[test]
    fn reformulated_defaults_have_finite_stable_curves_and_headroom_at_supported_analysis_rates() {
        let presets = built_in_presets();
        let flat_curve: Vec<_> = (0..=2048)
            .map(|index| {
                let frequency = 20.0_f64 * (20_000.0_f64 / 20.0_f64).powf(index as f64 / 2048.0);
                response_db(&presets[0], 48_000, frequency)
            })
            .collect();
        let mut sampled_curves = Vec::new();
        for preset in presets.iter().filter(|preset| preset.id != FLAT_ID) {
            validate_v1(preset).unwrap();
            let active: Vec<_> = preset.bands.iter().filter(|band| band.enabled).collect();
            assert!((4..=10).contains(&active.len()), "{} filter count", preset.id);
            if !preset.id.starts_with("tone_") { assert_eq!(preset.preamp_db, 0.0, "{} default preamp", preset.id); }

            let mut curve_48k = Vec::new();
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let protection = crate::shared_dsp::safety::assess(preset, rate)
                    .unwrap_or_else(|_| panic!("{} rejected at {rate} Hz", preset.id));
                assert!(protection.filter_bound_db.is_finite());
                assert!(protection.effective_preamp_db.is_finite());
                assert!(protection.effective_preamp_db + protection.filter_bound_db
                    + protection.resampler_bound_db <= protection.ceiling_db + 1e-9);

                for band in &active {
                    assert!((MIN_FREQUENCY_HZ..=MAX_FREQUENCY_HZ).contains(&band.frequency_hz));
                    assert!((MIN_GAIN_DB..=MAX_GAIN_DB).contains(&band.gain_db));
                    assert!((MIN_Q..=MAX_Q).contains(&band.q));
                    let [b0, b1, b2, a1, a2] = crate::shared_dsp::safety::coefficients(band, rate);
                    assert!([b0, b1, b2, a1, a2].iter().all(|value| value.is_finite()));
                    let discriminant = a1 * a1 - 4. * a2;
                    let radius = if discriminant < 0. {
                        a2.sqrt()
                    } else {
                        let root = discriminant.sqrt();
                        ((-a1 + root) / 2.).abs().max(((-a1 - root) / 2.).abs())
                    };
                    assert!(radius < 1.0, "{} has unstable pole at {rate} Hz", preset.id);
                }

                for index in 0..=2048 {
                    let frequency = 20.0_f64 * (20_000.0_f64 / 20.0_f64).powf(index as f64 / 2048.0);
                    let response = response_db(preset, rate, frequency);
                    assert!(response.is_finite(), "{} non-finite response at {frequency} Hz / {rate}", preset.id);
                    if rate == 48_000 { curve_48k.push(response); }
                }
            }
            let peak = curve_48k.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let minimum = curve_48k.iter().copied().fold(f64::INFINITY, f64::min);
            eprintln!("{}: sampled response {minimum:.2}..{peak:.2} dB; 48 kHz safety bound {:.2} dB; effective preamp {:.2} dB", preset.id,
                crate::shared_dsp::safety::assess(preset, 48_000).unwrap().filter_bound_db,
                crate::shared_dsp::safety::assess(preset, 48_000).unwrap().effective_preamp_db);
            sampled_curves.push(curve_48k);
        }

        // Each named profile must remain discernible from Flat and from every other profile.
        for left in 0..sampled_curves.len() {
            let flat_distance = sampled_curves[left].iter().zip(&flat_curve)
                .map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
            assert!(flat_distance >= 0.25, "profile {left} is indistinguishable from Flat");
            for right in 0..left {
                let distance = sampled_curves[left].iter().zip(&sampled_curves[right])
                    .map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
                assert!(distance >= 0.25, "profiles {left} and {right} are indistinguishable");
            }
        }
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
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

pub(crate) fn validate_preset(preset: &EqPreset) -> Result<(), EqError> {
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

pub(crate) fn validate_v1(preset: &EqPreset) -> Result<(), EqError> {
    validate_preset(preset)?;
    if preset.bands.len() != 10 {
        return Err(EqError::InvalidBandCount);
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
        if self.equipment_profiles.len() > 32 { return Err(EqError::InvalidEquipment); }
        let (mut equipment_ids, mut equipment_names) = (HashSet::new(), HashSet::new());
        for e in &self.equipment_profiles {
            if !valid_id(&e.id) || normalized_name(&e.name)? != e.name ||
                !equipment_ids.insert(&e.id) || !equipment_names.insert(e.name.to_lowercase()) {
                return Err(EqError::InvalidEquipment);
            }
            self.preset(&e.preset)?;
        }
        match &self.active_preset {
            PresetRef::BuiltIn { id } if builtin_preset(id).is_some() => Ok(()),
            PresetRef::BuiltIn { .. } => Err(EqError::UnknownBuiltinPreset),
            PresetRef::User { id } if !valid_id(id) || !ids.contains(id) => {
                Err(EqError::MissingActivePreset)
            }
            PresetRef::User { .. } => Ok(()),
        }
    }

    #[cfg(test)]
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
            PresetRef::BuiltIn { id } => builtin_preset(id).ok_or(EqError::UnknownBuiltinPreset),
            PresetRef::User { id } => self
                .user_presets
                .iter()
                .find(|preset| &preset.id == id)
                .cloned()
                .ok_or(EqError::UserPresetMissing),
        }
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub fn duplicate_preset(
        &mut self,
        source: &PresetRef,
        name: &str,
    ) -> Result<EqPreset, EqError> {
        self.create_user_preset(name, Some(source))
    }

    /// Explicit saved selection only. Equipment is a user label, not inferred hardware identity.
    pub fn link_equipment(&mut self, id: Option<String>, name: &str, preset: PresetRef) -> Result<(), EqError> {
        let name = normalized_name(name)?;
        let supplied = id.is_some();
        let id = match id { Some(id)=>id, None=>new_preset_id()? };
        self.edit(|config| {
            config.preset(&preset)?;
            let e = EquipmentProfile { id: id.clone(), name, preset };
            if supplied {
                let old = config.equipment_profiles.iter_mut().find(|e|e.id == id).ok_or(EqError::InvalidEquipment)?;
                *old = e;
            } else { config.equipment_profiles.push(e); }
            Ok(())
        })
    }
    pub fn unlink_equipment(&mut self, id: &str) -> Result<(), EqError> {
        self.edit(|config| {
            let index = config.equipment_profiles.iter().position(|e|e.id == id).ok_or(EqError::InvalidEquipment)?;
            config.equipment_profiles.remove(index); Ok(())
        })
    }

    pub fn delete_preset(&mut self, reference: &PresetRef) -> Result<(), EqError> {
        let id = match reference {
            PresetRef::BuiltIn { .. } => return Err(EqError::BuiltinReadOnly),
            PresetRef::User { id } => id,
        };
        self.edit(|config| {
            if config.equipment_profiles.iter().any(|e| &e.preset == reference) { return Err(EqError::PresetInUse); }
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
        for p in &config.user_presets {
            validate_v1(p)?;
        }
        Ok(config)
    }

    pub fn save(&self, config: &EqualizerConfig) -> Result<(), EqError> {
        config.validate()?;
        for p in &config.user_presets {
            validate_v1(p)?;
        }
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
        assert!(flat.bands.iter().all(|band| !band.enabled
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
    fn all_builtins_are_readonly_generic_copies_and_never_enable_dsp() {
        let presets = built_in_presets();
        assert_eq!(presets.len(), 19);
        let mut config = EqualizerConfig::default();
        for p in presets {
            validate_v1(&p).unwrap();
            assert!(crate::shared_dsp::safety::assess(&p, 48000).is_ok());
            let reference = PresetRef::BuiltIn { id: p.id.clone() };
            config.set_active_preset(reference.clone()).unwrap();
            assert_eq!(config.preset(&reference).unwrap(), p);
            assert!(!config.enabled);
            assert_eq!(
                config.delete_preset(&reference),
                Err(EqError::BuiltinReadOnly)
            );
            let copy = config
                .create_user_preset(&format!("Copy {}", p.id), Some(&reference))
                .unwrap();
            assert_eq!(copy.bands, p.bands);
            assert_eq!(copy.preamp_db, p.preamp_db);
            assert_ne!(copy.id, p.id);
        }
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
        for value in [-16.0, -15.01, 15.01, 16.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
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
    fn unknown_fields_are_rejected_and_size_is_bounded() {
        let fixture = TestStore::new();
        let mut json = serde_json::to_value(EqualizerConfig::default()).unwrap();
        json["future_optional"] = serde_json::json!(true);
        fs::write(&fixture.store.path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(fixture.store.load(), Err(EqError::InvalidJson));
        json.as_object_mut().unwrap().remove("future_optional");
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
