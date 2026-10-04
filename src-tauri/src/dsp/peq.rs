//! Pure conversion of a product preset to CamillaDSP 4.1.3 filter/pipeline data.
use crate::equalizer::{
    self, EqError, EqFilterType, EqPreset, MAX_BANDS, MAX_FREQUENCY_HZ, MAX_GAIN_DB, MAX_PREAMP_DB,
    MAX_Q, MIN_FREQUENCY_HZ, MIN_GAIN_DB, MIN_PREAMP_DB, MIN_Q,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const PREAMP_NAME: &str = "peq_preamp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate_hz: u32,
    pub channels: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeqConversionError {
    UnsupportedChannels(u8),
    InvalidSampleRate(u32),
    InvalidPreset(EqError),
    InvalidBandIdentity {
        band_id: String,
    },
    DuplicateRuntimeFilterName {
        name: String,
    },
    InvalidFrequency {
        band_id: String,
        frequency_hz: f64,
    },
    FrequencyAboveNyquist {
        band_id: String,
        frequency_hz: f64,
        sample_rate_hz: u32,
        reason: &'static str,
    },
    InvalidGain {
        band_id: String,
        gain_db: f64,
    },
    InvalidQ {
        band_id: String,
        q: f64,
    },
    InvalidPreamp(f64),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CamillaPeqConfig {
    pub filters: BTreeMap<String, CamillaFilter>,
    pub pipeline: Vec<CamillaPipelineStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CamillaFilter {
    Gain { parameters: CamillaGain },
    Biquad { parameters: CamillaBiquad },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CamillaGain {
    pub gain: f64,
    pub scale: CamillaGainScale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CamillaGainScale {
    #[serde(rename = "dB")]
    Db,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CamillaBiquad {
    Peaking { freq: f64, gain: f64, q: f64 },
    Lowshelf { freq: f64, gain: f64, q: f64 },
    Highshelf { freq: f64, gain: f64, q: f64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CamillaPipelineStep {
    Filter {
        channels: Vec<usize>,
        names: Vec<String>,
    },
}

fn band_name(id: &str) -> Result<String, PeqConversionError> {
    if !equalizer::valid_id(id) {
        return Err(PeqConversionError::InvalidBandIdentity {
            band_id: id.to_owned(),
        });
    }
    // Hex encoding makes the persisted identity safe as a CamillaDSP map key.
    let mut name = String::from("peq_b_");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in id.bytes() {
        name.push(char::from(HEX[usize::from(byte >> 4)]));
        name.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(name)
}

fn push_step(
    config: &mut CamillaPeqConfig,
    name: String,
    filter: CamillaFilter,
) -> Result<(), PeqConversionError> {
    if config.filters.insert(name.clone(), filter).is_some() {
        return Err(PeqConversionError::DuplicateRuntimeFilterName { name });
    }
    config.pipeline.push(CamillaPipelineStep::Filter {
        channels: vec![0, 1],
        names: vec![name],
    });
    Ok(())
}

pub fn convert_preset(
    preset: &EqPreset,
    format: PcmFormat,
) -> Result<CamillaPeqConfig, PeqConversionError> {
    if format.channels != 2 {
        return Err(PeqConversionError::UnsupportedChannels(format.channels));
    }
    if format.sample_rate_hz == 0 {
        return Err(PeqConversionError::InvalidSampleRate(format.sample_rate_hz));
    }
    if !preset.preamp_db.is_finite() || !(MIN_PREAMP_DB..=MAX_PREAMP_DB).contains(&preset.preamp_db)
    {
        return Err(PeqConversionError::InvalidPreamp(preset.preamp_db));
    }
    if preset.bands.is_empty() || preset.bands.len() > MAX_BANDS {
        return Err(PeqConversionError::InvalidPreset(EqError::InvalidBandCount));
    }

    let mut names = HashSet::new();
    let mut converted = Vec::with_capacity(preset.bands.len());
    for band in &preset.bands {
        let name = band_name(&band.id)?;
        if !names.insert(name.clone()) {
            return Err(PeqConversionError::DuplicateRuntimeFilterName { name });
        }
        if !band.frequency_hz.is_finite()
            || !(MIN_FREQUENCY_HZ..=MAX_FREQUENCY_HZ).contains(&band.frequency_hz)
        {
            return Err(PeqConversionError::InvalidFrequency {
                band_id: band.id.clone(),
                frequency_hz: band.frequency_hz,
            });
        }
        if !band.gain_db.is_finite() || !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&band.gain_db) {
            return Err(PeqConversionError::InvalidGain {
                band_id: band.id.clone(),
                gain_db: band.gain_db,
            });
        }
        if !band.q.is_finite() || !(MIN_Q..=MAX_Q).contains(&band.q) {
            return Err(PeqConversionError::InvalidQ {
                band_id: band.id.clone(),
                q: band.q,
            });
        }
        if !band.enabled {
            continue;
        }
        // CamillaDSP can use f32 or f64; rounding must not reach Nyquist in either.
        if band.frequency_hz >= f64::from(format.sample_rate_hz) / 2.0
            || (band.frequency_hz as f32) >= (format.sample_rate_hz as f32) / 2.0
        {
            return Err(PeqConversionError::FrequencyAboveNyquist {
                band_id: band.id.clone(),
                frequency_hz: band.frequency_hz,
                sample_rate_hz: format.sample_rate_hz,
                reason: "CamillaDSP requires frequency < sample_rate / 2 at processing precision",
            });
        }
        let biquad = match band.filter_type {
            EqFilterType::Peak => CamillaBiquad::Peaking {
                freq: band.frequency_hz,
                gain: band.gain_db,
                q: band.q,
            },
            EqFilterType::LowShelf => CamillaBiquad::Lowshelf {
                freq: band.frequency_hz,
                gain: band.gain_db,
                q: band.q,
            },
            EqFilterType::HighShelf => CamillaBiquad::Highshelf {
                freq: band.frequency_hz,
                gain: band.gain_db,
                q: band.q,
            },
        };
        converted.push((name, CamillaFilter::Biquad { parameters: biquad }));
    }
    // Recheck product invariants even for structs constructed without the loader.
    equalizer::validate_preset(preset).map_err(PeqConversionError::InvalidPreset)?;

    let mut config = CamillaPeqConfig {
        filters: BTreeMap::new(),
        pipeline: Vec::new(),
    };
    // Explicit zero gain and zero-gain biquads keep stable names for future updates.
    push_step(
        &mut config,
        PREAMP_NAME.into(),
        CamillaFilter::Gain {
            parameters: CamillaGain {
                gain: preset.preamp_db,
                scale: CamillaGainScale::Db,
            },
        },
    )?;
    for (name, filter) in converted {
        push_step(&mut config, name, filter)?;
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::equalizer::flat_preset;
    use serde_json::{json, Value};

    fn pcm(rate: u32) -> PcmFormat {
        PcmFormat {
            sample_rate_hz: rate,
            channels: 2,
        }
    }
    fn names(config: &CamillaPeqConfig) -> Vec<String> {
        config
            .pipeline
            .iter()
            .map(|step| match step {
                CamillaPipelineStep::Filter { names, .. } => names[0].clone(),
            })
            .collect()
    }

    #[test]
    fn flat_converts_with_preamp_first_and_ten_stable_zero_gain_biquads() {
        let preset = flat_preset();
        let config = convert_preset(&preset, pcm(48_000)).unwrap();
        let order = names(&config);
        assert_eq!(order.len(), 11);
        assert_eq!(order[0], PREAMP_NAME);
        assert_eq!(
            config.filters[PREAMP_NAME],
            CamillaFilter::Gain {
                parameters: CamillaGain {
                    gain: 0.0,
                    scale: CamillaGainScale::Db
                }
            }
        );
        assert!(order[1..].iter().all(|name| matches!(
            &config.filters[name],
            CamillaFilter::Biquad {
                parameters: CamillaBiquad::Peaking { gain: 0.0, .. }
            }
        )));
    }

    #[test]
    fn peak_shelves_preamp_and_pipeline_serialize_to_camilla_shape() {
        let mut preset = flat_preset();
        preset.bands.truncate(3);
        preset.preamp_db = -6.0;
        preset.bands[0].filter_type = EqFilterType::Peak;
        preset.bands[1].filter_type = EqFilterType::LowShelf;
        preset.bands[2].filter_type = EqFilterType::HighShelf;
        preset.bands[0].gain_db = 2.0;
        preset.bands[1].gain_db = -3.0;
        preset.bands[2].gain_db = 4.0;
        let config = convert_preset(&preset, pcm(44_100)).unwrap();
        let value: Value = serde_json::to_value(&config).unwrap();
        let order = names(&config);
        assert_eq!(
            value["filters"][PREAMP_NAME],
            json!({"type":"Gain","parameters":{"gain":-6.0,"scale":"dB"}})
        );
        for (index, kind) in ["Peaking", "Lowshelf", "Highshelf"].iter().enumerate() {
            let band = &preset.bands[index];
            assert_eq!(
                value["filters"][&order[index + 1]],
                json!({"type":"Biquad","parameters":{"type":kind,"freq":band.frequency_hz,"gain":band.gain_db,"q":band.q}})
            );
            assert_eq!(
                value["pipeline"][index + 1],
                json!({"type":"Filter","channels":[0,1],"names":[order[index + 1]]})
            );
        }
        assert_eq!(
            value["pipeline"][0],
            json!({"type":"Filter","channels":[0,1],"names":[PREAMP_NAME]})
        );
    }

    #[test]
    fn disabled_band_is_omitted_but_invalid_disabled_values_still_fail() {
        let mut preset = flat_preset();
        preset.bands[0].enabled = false;
        let config = convert_preset(&preset, pcm(48_000)).unwrap();
        assert_eq!(config.pipeline.len(), 10);
        assert!(!config
            .filters
            .contains_key(&band_name(&preset.bands[0].id).unwrap()));
        preset.bands[0].q = f64::NAN;
        assert!(matches!(
            convert_preset(&preset, pcm(48_000)),
            Err(PeqConversionError::InvalidQ { .. })
        ));
    }

    #[test]
    fn order_can_change_without_changing_band_identity() {
        let mut preset = flat_preset();
        let before = names(&convert_preset(&preset, pcm(48_000)).unwrap());
        preset.bands.swap(0, 1);
        let after = names(&convert_preset(&preset, pcm(48_000)).unwrap());
        assert_eq!(before[1], after[2]);
        assert_eq!(before[2], after[1]);
        assert_ne!(before[1], before[2]);
        assert!(after.iter().all(|name| name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')));
    }

    #[test]
    fn frequency_is_strictly_below_nyquist_without_clamping() {
        let mut preset = flat_preset();
        preset.bands.truncate(1);
        preset.bands[0].frequency_hz = 20_000.0;
        for rate in [44_100, 48_000, 96_000, 192_000] {
            assert!(convert_preset(&preset, pcm(rate)).is_ok(), "rate {rate}");
        }
        for rate in [32_000, 40_000] {
            assert!(
                matches!(convert_preset(&preset, pcm(rate)), Err(PeqConversionError::FrequencyAboveNyquist { sample_rate_hz, band_id, frequency_hz: 20_000.0, .. }) if sample_rate_hz == rate && band_id == preset.bands[0].id)
            );
        }
        preset.bands[0].frequency_hz = 19_999.999;
        assert!(convert_preset(&preset, pcm(40_000)).is_ok());
        preset.bands[0].frequency_hz = 19_999.9999;
        assert!(matches!(
            convert_preset(&preset, pcm(40_000)),
            Err(PeqConversionError::FrequencyAboveNyquist { .. })
        ));
        preset.bands[0].frequency_hz = 20_000.001;
        assert!(matches!(
            convert_preset(&preset, pcm(48_000)),
            Err(PeqConversionError::InvalidFrequency { .. })
        ));
    }

    #[test]
    fn channels_and_sample_rate_must_be_explicitly_supported() {
        let preset = flat_preset();
        assert_eq!(
            convert_preset(&preset, pcm(0)),
            Err(PeqConversionError::InvalidSampleRate(0))
        );
        for channels in [0, 1, 3, 8] {
            assert_eq!(
                convert_preset(
                    &preset,
                    PcmFormat {
                        sample_rate_hz: 48_000,
                        channels
                    }
                ),
                Err(PeqConversionError::UnsupportedChannels(channels))
            );
        }
    }

    #[test]
    fn defensive_numeric_validation_rejects_invalid_and_nonfinite_values() {
        let mut preset = flat_preset();
        preset.bands.truncate(1);
        for value in [19.0, 20_001.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            preset.bands[0].frequency_hz = value;
            assert!(matches!(
                convert_preset(&preset, pcm(48_000)),
                Err(PeqConversionError::InvalidFrequency { .. })
            ));
        }
        preset.bands[0].frequency_hz = 1000.0;
        for value in [-12.01, 12.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            preset.bands[0].gain_db = value;
            assert!(matches!(
                convert_preset(&preset, pcm(48_000)),
                Err(PeqConversionError::InvalidGain { .. })
            ));
        }
        preset.bands[0].gain_db = 0.0;
        for value in [0.099, 30.001, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            preset.bands[0].q = value;
            assert!(matches!(
                convert_preset(&preset, pcm(48_000)),
                Err(PeqConversionError::InvalidQ { .. })
            ));
        }
        preset.bands[0].q = 1.0;
        for value in [-24.01, 12.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            preset.preamp_db = value;
            assert!(matches!(
                convert_preset(&preset, pcm(48_000)),
                Err(PeqConversionError::InvalidPreamp(_))
            ));
        }
    }

    #[test]
    fn numeric_product_bounds_are_inclusive() {
        let mut preset = flat_preset();
        preset.bands.truncate(1);
        for (frequency_hz, gain_db, q, preamp_db) in
            [(20.0, -12.0, 0.1, -24.0), (20_000.0, 12.0, 30.0, 12.0)]
        {
            preset.bands[0].frequency_hz = frequency_hz;
            preset.bands[0].gain_db = gain_db;
            preset.bands[0].q = q;
            preset.preamp_db = preamp_db;
            assert!(convert_preset(&preset, pcm(96_000)).is_ok());
        }
    }

    #[test]
    fn rejects_invalid_identity_collisions_and_directly_built_invalid_preset() {
        let mut preset = flat_preset();
        preset.bands[0].id = "unsafe\nname".into();
        assert!(matches!(
            convert_preset(&preset, pcm(48_000)),
            Err(PeqConversionError::InvalidBandIdentity { .. })
        ));
        preset.bands[0].id = preset.bands[1].id.clone();
        assert!(matches!(
            convert_preset(&preset, pcm(48_000)),
            Err(PeqConversionError::DuplicateRuntimeFilterName { .. })
        ));
        preset.bands[0].id = "unique".into();
        preset.name.clear();
        assert_eq!(
            convert_preset(&preset, pcm(48_000)),
            Err(PeqConversionError::InvalidPreset(
                EqError::InvalidPresetName
            ))
        );
    }

    #[test]
    fn accepts_sixty_four_bands_and_rejects_one_more() {
        let mut preset = flat_preset();
        let example = preset.bands[0].clone();
        while preset.bands.len() < MAX_BANDS {
            let mut band = example.clone();
            band.id = format!("extra_{}", preset.bands.len());
            preset.bands.push(band);
        }
        assert_eq!(
            convert_preset(&preset, pcm(48_000)).unwrap().pipeline.len(),
            MAX_BANDS + 1
        );
        let mut band = example;
        band.id = "extra_64".into();
        preset.bands.push(band);
        assert_eq!(
            convert_preset(&preset, pcm(48_000)),
            Err(PeqConversionError::InvalidPreset(EqError::InvalidBandCount))
        );
    }
}
