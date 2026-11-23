//! Read-only OPRA-to-Sonante compatibility assessment.
//!
//! This adapter never edits the catalog, persists a preset, contacts a service,
//! or changes EQ state. A preset candidate is returned only when the response
//! can be proven unchanged from the documented input semantics.

use crate::equalizer::{
    self, EqBand, EqFilterType, EqPreset, MAX_BANDS, MAX_FREQUENCY_HZ, MAX_GAIN_DB, MAX_PREAMP_DB,
    MAX_Q, MIN_FREQUENCY_HZ, MIN_GAIN_DB, MIN_PREAMP_DB, MIN_Q,
};
use crate::opra_catalog::{Catalog, EqRecord, OpraFilterType};

pub const OPRA_DECLARED_DATASET_LICENSE: &str = "CC-BY-SA-4.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Compatible,
    Incompatible,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumericField {
    Frequency,
    BandGain,
    BandQ,
    OverallGain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    UnsupportedFilter {
        band_index: usize,
    },
    TooManyBands {
        count: usize,
        maximum: usize,
    },
    ParameterOutOfRange {
        field: NumericField,
        band_index: Option<usize>,
    },
    MissingRequiredParameter {
        field: NumericField,
        band_index: usize,
    },
    MissingRequiredSlope {
        band_index: usize,
    },
    UnmappedSlope {
        band_index: usize,
    },
    QConventionNotSpecified {
        band_index: usize,
    },
    ShelfGainReferenceNotSpecified {
        band_index: usize,
    },
    OverallGainRoleNotSpecified,
    CandidateValidationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub opra_record_id: String,
    pub product_id: Option<String>,
    pub product_name: Option<String>,
    pub vendor_id: Option<String>,
    pub vendor_name: Option<String>,
    pub author: String,
    pub details: Option<String>,
    /// Retained as untrusted metadata; the adapter never opens this URL.
    pub source_url: Option<String>,
    /// This is the aggregate dataset declaration, not an individual
    /// measurement license or a rights determination for the source record.
    pub declared_dataset_license: &'static str,
    pub individual_source_license: Option<String>,
    /// The parser has no snapshot/version input yet, so this remains unknown.
    pub catalog_snapshot: Option<String>,
    pub transformation: Transformation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transformation {
    NoPresetProduced,
    ExactResponsePreservingMapping,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub verdict: Verdict,
    pub reasons: Vec<Reason>,
    pub provenance: Provenance,
    /// Present only for a mathematically response-neutral profile. This is an
    /// in-memory preview value, not a saved or activated preset.
    pub preview_preset: Option<EqPreset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssessmentError {
    ProfileNotFound,
}

/// Assess one parsed profile without changing it or its catalog.
pub fn assess_profile(catalog: &Catalog, record_id: &str) -> Result<Assessment, AssessmentError> {
    let record = catalog
        .equalizers
        .get(record_id)
        .ok_or(AssessmentError::ProfileNotFound)?;

    let mut reasons = Vec::new();
    inspect_profile(record, &mut reasons);
    let (preview_preset, transformation) = if reasons.is_empty() {
        match make_neutral_preview(record) {
            Some(preset) => (Some(preset), Transformation::ExactResponsePreservingMapping),
            None => {
                reasons.push(Reason::CandidateValidationFailed);
                (None, Transformation::NoPresetProduced)
            }
        }
    } else {
        (None, Transformation::NoPresetProduced)
    };
    let verdict = if reasons.iter().any(is_hard_incompatibility) {
        Verdict::Incompatible
    } else if !reasons.is_empty() {
        Verdict::Unverified
    } else {
        Verdict::Compatible
    };
    let provenance = provenance(catalog, record, transformation);
    Ok(Assessment {
        verdict,
        reasons,
        provenance,
        preview_preset,
    })
}

fn inspect_profile(record: &EqRecord, reasons: &mut Vec<Reason>) {
    let params = &record.data.parameters;
    if params.bands.len() > MAX_BANDS {
        reasons.push(Reason::TooManyBands {
            count: params.bands.len(),
            maximum: MAX_BANDS,
        });
    }
    let overall_gain_in_range =
        params.gain_db.is_finite() && (MIN_PREAMP_DB..=MAX_PREAMP_DB).contains(&params.gain_db);
    if !overall_gain_in_range {
        reasons.push(Reason::ParameterOutOfRange {
            field: NumericField::OverallGain,
            band_index: None,
        });
    }

    if overall_gain_in_range && params.gain_db != 0.0 {
        reasons.push(Reason::OverallGainRoleNotSpecified);
    }

    for (band_index, band) in params.bands.iter().enumerate() {
        let sonante_type = match band.filter_type {
            OpraFilterType::PeakDip => Some(EqFilterType::Peak),
            OpraFilterType::LowShelf => Some(EqFilterType::LowShelf),
            OpraFilterType::HighShelf => Some(EqFilterType::HighShelf),
            OpraFilterType::LowPass
            | OpraFilterType::HighPass
            | OpraFilterType::BandPass
            | OpraFilterType::BandStop => {
                reasons.push(Reason::UnsupportedFilter { band_index });
                None
            }
        };

        if !band.frequency.is_finite()
            || !(MIN_FREQUENCY_HZ..=MAX_FREQUENCY_HZ).contains(&band.frequency)
        {
            reasons.push(Reason::ParameterOutOfRange {
                field: NumericField::Frequency,
                band_index: Some(band_index),
            });
        }
        let gain = band.gain_db.unwrap_or(0.0);
        if !gain.is_finite() || !(MIN_GAIN_DB..=MAX_GAIN_DB).contains(&gain) {
            reasons.push(Reason::ParameterOutOfRange {
                field: NumericField::BandGain,
                band_index: Some(band_index),
            });
        }
        let q_required = !matches!(
            band.filter_type,
            OpraFilterType::LowPass | OpraFilterType::HighPass
        );
        match band.q {
            None if q_required => reasons.push(Reason::MissingRequiredParameter {
                field: NumericField::BandQ,
                band_index,
            }),
            None => {}
            Some(q) if !q.is_finite() || !(MIN_Q..=MAX_Q).contains(&q) => {
                reasons.push(Reason::ParameterOutOfRange {
                    field: NumericField::BandQ,
                    band_index: Some(band_index),
                });
            }
            Some(_) => {}
        }

        if band.slope.is_some() {
            // Sonante's preset model has no slope field. Even a shelf's
            // optional OPRA slope is not silently thrown away.
            reasons.push(Reason::UnmappedSlope { band_index });
        }
        if matches!(
            band.filter_type,
            OpraFilterType::LowPass | OpraFilterType::HighPass
        ) && band.slope.is_none()
        {
            reasons.push(Reason::MissingRequiredSlope { band_index });
        }
        if gain != 0.0 {
            if band.q.is_some() {
                reasons.push(Reason::QConventionNotSpecified { band_index });
            }
            if matches!(
                sonante_type,
                Some(EqFilterType::LowShelf | EqFilterType::HighShelf)
            ) {
                reasons.push(Reason::ShelfGainReferenceNotSpecified { band_index });
            }
        }
    }
}

fn is_hard_incompatibility(reason: &Reason) -> bool {
    matches!(
        reason,
        Reason::UnsupportedFilter { .. }
            | Reason::TooManyBands { .. }
            | Reason::ParameterOutOfRange { .. }
            | Reason::MissingRequiredParameter { .. }
            | Reason::MissingRequiredSlope { .. }
            | Reason::UnmappedSlope { .. }
            | Reason::CandidateValidationFailed
    )
}

fn make_neutral_preview(record: &EqRecord) -> Option<EqPreset> {
    let mut preset = equalizer::flat_preset();
    preset.id = "opra_preview".into();
    preset.name = "OPRA response-equivalent preview".into();
    preset.preamp_db = 0.0;

    for (index, source) in record.data.parameters.bands.iter().enumerate() {
        let filter_type = match source.filter_type {
            OpraFilterType::PeakDip => EqFilterType::Peak,
            OpraFilterType::LowShelf => EqFilterType::LowShelf,
            OpraFilterType::HighShelf => EqFilterType::HighShelf,
            _ => return None,
        };
        let q = source.q?;
        preset.bands[index] = EqBand {
            id: format!("opra_{index:02}"),
            enabled: true,
            filter_type,
            frequency_hz: source.frequency,
            gain_db: source.gain_db.unwrap_or(0.0),
            q,
        };
    }
    equalizer::validate_v1(&preset).ok()?;
    crate::shared_dsp::safety::assess(&preset, 48_000).ok()?;
    Some(preset)
}

fn provenance(catalog: &Catalog, record: &EqRecord, transformation: Transformation) -> Provenance {
    let product = record
        .data
        .product_id
        .as_ref()
        .and_then(|id| catalog.products.get(id));
    let vendor = product
        .and_then(|p| p.data.vendor_id.as_ref())
        .and_then(|id| catalog.vendors.get(id));
    Provenance {
        opra_record_id: record.id.clone(),
        product_id: record.data.product_id.clone(),
        product_name: product.map(|p| p.data.name.clone()),
        vendor_id: product.and_then(|p| p.data.vendor_id.clone()),
        vendor_name: vendor.map(|v| v.data.name.clone()),
        author: record.data.author.clone(),
        details: record.data.details.clone(),
        source_url: record.data.link.clone(),
        declared_dataset_license: OPRA_DECLARED_DATASET_LICENSE,
        individual_source_license: None,
        catalog_snapshot: None,
        transformation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opra_catalog::{
        EqInfo, EqParameters, EqType, ProductInfo, ProductSubtype, ProductType, VendorInfo,
    };
    use std::collections::BTreeMap;

    fn band(
        kind: OpraFilterType,
        frequency: f64,
        gain: Option<f64>,
        q: Option<f64>,
    ) -> crate::opra_catalog::OpraBand {
        crate::opra_catalog::OpraBand {
            filter_type: kind,
            frequency,
            gain_db: gain,
            q,
            slope: None,
        }
    }

    fn test_catalog(bands: Vec<crate::opra_catalog::OpraBand>, overall_gain: f64) -> Catalog {
        let eq = EqRecord {
            id: "eq_synthetic".into(),
            line: 1,
            data: EqInfo {
                product_id: Some("product_synthetic".into()),
                author: "Synthetic Author".into(),
                details: Some("Synthetic target • test".into()),
                link: Some("https://example.invalid/source".into()),
                eq_type: EqType::ParametricEq,
                parameters: EqParameters {
                    gain_db: overall_gain,
                    bands,
                },
            },
        };
        let product = crate::opra_catalog::ProductRecord {
            id: "product_synthetic".into(),
            line: 2,
            data: ProductInfo {
                vendor_id: Some("vendor_synthetic".into()),
                name: "Synthetic headphone".into(),
                blurb: None,
                line_art_svg: None,
                line_art_96x64_png: None,
                product_type: ProductType::Headphones,
                subtype: ProductSubtype::OverTheEar,
            },
        };
        let vendor = crate::opra_catalog::VendorRecord {
            id: "vendor_synthetic".into(),
            line: 3,
            data: VendorInfo {
                name: "Synthetic vendor".into(),
                official_name: None,
                blurb: None,
                logo: None,
            },
        };
        Catalog {
            vendors: BTreeMap::from([(vendor.id.clone(), vendor)]),
            products: BTreeMap::from([(product.id.clone(), product)]),
            equalizers: BTreeMap::from([(eq.id.clone(), eq)]),
        }
    }

    fn rbj_response_db(band: &EqBand, frequency: f64, sample_rate: f64) -> f64 {
        let omega = 2.0 * std::f64::consts::PI * band.frequency_hz / sample_rate;
        let cosine = omega.cos();
        let sine = omega.sin();
        let a = 10.0_f64.powf(band.gain_db / 40.0);
        let alpha = sine / (2.0 * band.q);
        let (b0, b1, b2, a0, a1, a2) = match band.filter_type {
            EqFilterType::Peak => (
                1.0 + alpha * a,
                -2.0 * cosine,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cosine,
                1.0 - alpha / a,
            ),
            EqFilterType::LowShelf => {
                let beta = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cosine + beta),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cosine),
                    a * ((a + 1.0) - (a - 1.0) * cosine - beta),
                    (a + 1.0) + (a - 1.0) * cosine + beta,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cosine),
                    (a + 1.0) + (a - 1.0) * cosine - beta,
                )
            }
            EqFilterType::HighShelf => {
                let beta = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cosine + beta),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cosine),
                    a * ((a + 1.0) + (a - 1.0) * cosine - beta),
                    (a + 1.0) - (a - 1.0) * cosine + beta,
                    2.0 * ((a - 1.0) - (a + 1.0) * cosine),
                    (a + 1.0) - (a - 1.0) * cosine - beta,
                )
            }
        };
        let w = 2.0 * std::f64::consts::PI * frequency / sample_rate;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -b1 * s1 - b2 * s2;
        let dr = a0 + a1 * c1 + a2 * c2;
        let di = -a1 * s1 - a2 * s2;
        20.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt().log10()
    }

    #[test]
    fn zero_gain_supported_filters_are_proven_response_equivalent_and_order_is_kept() {
        let original = test_catalog(
            vec![
                band(OpraFilterType::PeakDip, 1000.0, Some(0.0), Some(0.7)),
                band(OpraFilterType::LowShelf, 120.0, Some(0.0), Some(0.8)),
                band(OpraFilterType::HighShelf, 8000.0, Some(0.0), Some(0.9)),
            ],
            0.0,
        );
        let before = original.clone();
        let assessment = assess_profile(&original, "eq_synthetic").unwrap();
        assert_eq!(assessment.verdict, Verdict::Compatible);
        assert!(assessment.reasons.is_empty());
        let preset = assessment.preview_preset.as_ref().unwrap();
        assert_eq!(preset.preamp_db, 0.0);
        assert_eq!(
            preset.bands[..3]
                .iter()
                .map(|b| b.filter_type)
                .collect::<Vec<_>>(),
            vec![
                EqFilterType::Peak,
                EqFilterType::LowShelf,
                EqFilterType::HighShelf
            ]
        );
        assert_eq!(
            preset.bands[..3]
                .iter()
                .map(|b| b.frequency_hz)
                .collect::<Vec<_>>(),
            vec![1000.0, 120.0, 8000.0]
        );
        for band in &preset.bands {
            for frequency in [20.0, 40.0, 120.0, 1000.0, 8000.0, 19_999.0] {
                assert!(rbj_response_db(band, frequency, 48_000.0).abs() <= 1e-10);
            }
        }
        assert_eq!(original, before);
        assert_eq!(
            assessment.provenance.transformation,
            Transformation::ExactResponsePreservingMapping
        );
    }

    #[test]
    fn nonzero_peak_is_unverified_because_opra_does_not_define_q_equations() {
        let catalog = test_catalog(
            vec![band(OpraFilterType::PeakDip, 1000.0, Some(4.0), Some(1.2))],
            0.0,
        );
        let assessment = assess_profile(&catalog, "eq_synthetic").unwrap();
        assert_eq!(assessment.verdict, Verdict::Unverified);
        assert!(assessment
            .reasons
            .contains(&Reason::QConventionNotSpecified { band_index: 0 }));
        assert!(assessment.preview_preset.is_none());
    }

    #[test]
    fn nonzero_shelf_is_unverified_for_q_and_gain_reference() {
        for kind in [OpraFilterType::LowShelf, OpraFilterType::HighShelf] {
            let catalog = test_catalog(vec![band(kind, 500.0, Some(-3.0), Some(0.8))], 0.0);
            let assessment = assess_profile(&catalog, "eq_synthetic").unwrap();
            assert_eq!(assessment.verdict, Verdict::Unverified);
            assert!(assessment
                .reasons
                .contains(&Reason::QConventionNotSpecified { band_index: 0 }));
            assert!(assessment
                .reasons
                .contains(&Reason::ShelfGainReferenceNotSpecified { band_index: 0 }));
        }
    }

    #[test]
    fn incompatible_profiles_are_not_truncated_or_converted() {
        let too_many = test_catalog(
            (0..11)
                .map(|i| {
                    band(
                        OpraFilterType::PeakDip,
                        100.0 + f64::from(i),
                        Some(0.0),
                        Some(1.0),
                    )
                })
                .collect(),
            0.0,
        );
        let result = assess_profile(&too_many, "eq_synthetic").unwrap();
        assert_eq!(result.verdict, Verdict::Incompatible);
        assert!(result
            .reasons
            .iter()
            .any(|r| matches!(r, Reason::TooManyBands { count: 11, .. })));
        assert!(result.preview_preset.is_none());

        let unsupported =
            test_catalog(vec![band(OpraFilterType::LowPass, 1000.0, None, None)], 0.0);
        let result = assess_profile(&unsupported, "eq_synthetic").unwrap();
        assert_eq!(result.verdict, Verdict::Incompatible);
        assert!(result
            .reasons
            .contains(&Reason::UnsupportedFilter { band_index: 0 }));

        let other_unsupported = [
            band(OpraFilterType::HighPass, 1000.0, None, None),
            band(OpraFilterType::BandPass, 1000.0, None, Some(1.0)),
            band(OpraFilterType::BandStop, 1000.0, None, Some(1.0)),
        ];
        for mut unsupported_band in other_unsupported {
            if matches!(
                unsupported_band.filter_type,
                OpraFilterType::LowPass | OpraFilterType::HighPass
            ) {
                unsupported_band.slope = Some(12);
            }
            let result =
                assess_profile(&test_catalog(vec![unsupported_band], 0.0), "eq_synthetic").unwrap();
            assert_eq!(result.verdict, Verdict::Incompatible);
            assert!(result
                .reasons
                .contains(&Reason::UnsupportedFilter { band_index: 0 }));
            assert!(result.preview_preset.is_none());
        }
    }

    #[test]
    fn nonzero_overall_gain_is_unverified_and_never_previewed() {
        let catalog = test_catalog(
            vec![band(OpraFilterType::PeakDip, 1000.0, Some(0.0), Some(1.0))],
            -3.0,
        );
        let assessment = assess_profile(&catalog, "eq_synthetic").unwrap();
        assert_eq!(assessment.verdict, Verdict::Unverified);
        assert!(assessment
            .reasons
            .contains(&Reason::OverallGainRoleNotSpecified));
        assert!(assessment.preview_preset.is_none());
    }

    #[test]
    fn slopes_are_not_dropped_even_when_filter_kind_is_recognized() {
        let mut shelf = band(OpraFilterType::LowShelf, 120.0, Some(0.0), Some(0.7));
        shelf.slope = Some(12);
        let assessment = assess_profile(&test_catalog(vec![shelf], 0.0), "eq_synthetic").unwrap();
        assert_eq!(assessment.verdict, Verdict::Incompatible);
        assert!(assessment
            .reasons
            .contains(&Reason::UnmappedSlope { band_index: 0 }));
        assert!(assessment.preview_preset.is_none());
    }

    #[test]
    fn low_pass_contract_requires_slope_and_does_not_misclassify_it_as_q() {
        let assessment = assess_profile(
            &test_catalog(vec![band(OpraFilterType::LowPass, 1000.0, None, None)], 0.0),
            "eq_synthetic",
        )
        .unwrap();
        assert_eq!(assessment.verdict, Verdict::Incompatible);
        assert!(assessment
            .reasons
            .contains(&Reason::MissingRequiredSlope { band_index: 0 }));
        assert!(!assessment
            .reasons
            .contains(&Reason::MissingRequiredParameter {
                field: NumericField::BandQ,
                band_index: 0,
            }));
    }

    #[test]
    fn range_errors_include_preamp_frequency_gain_and_q() {
        let cases = [
            (vec![], 12.1, NumericField::OverallGain),
            (
                vec![band(
                    OpraFilterType::PeakDip,
                    20_001.0,
                    Some(0.0),
                    Some(1.0),
                )],
                0.0,
                NumericField::Frequency,
            ),
            (
                vec![band(OpraFilterType::PeakDip, 1000.0, Some(15.1), Some(1.0))],
                0.0,
                NumericField::BandGain,
            ),
            (
                vec![band(OpraFilterType::PeakDip, 1000.0, Some(0.0), Some(30.1))],
                0.0,
                NumericField::BandQ,
            ),
        ];
        for (bands, global_gain, field) in cases {
            let catalog = test_catalog(bands, global_gain);
            let assessment = assess_profile(&catalog, "eq_synthetic").unwrap();
            assert_eq!(assessment.verdict, Verdict::Incompatible);
            assert!(assessment.reasons.iter().any(
                |reason| matches!(reason, Reason::ParameterOutOfRange { field: found, .. } if *found == field)
            ));
        }

        let invalid_q = test_catalog(
            vec![band(OpraFilterType::PeakDip, 1000.0, Some(0.0), Some(0.05))],
            0.0,
        );
        assert_eq!(
            assess_profile(&invalid_q, "eq_synthetic").unwrap().verdict,
            Verdict::Incompatible
        );
    }

    #[test]
    fn provenance_is_preserved_without_inventing_measurement_license_or_snapshot() {
        let catalog = test_catalog(
            vec![band(OpraFilterType::PeakDip, 1000.0, Some(0.0), Some(1.0))],
            0.0,
        );
        let assessment = assess_profile(&catalog, "eq_synthetic").unwrap();
        assert_eq!(assessment.provenance.opra_record_id, "eq_synthetic");
        assert_eq!(
            assessment.provenance.product_id.as_deref(),
            Some("product_synthetic")
        );
        assert_eq!(
            assessment.provenance.product_name.as_deref(),
            Some("Synthetic headphone")
        );
        assert_eq!(
            assessment.provenance.vendor_id.as_deref(),
            Some("vendor_synthetic")
        );
        assert_eq!(
            assessment.provenance.vendor_name.as_deref(),
            Some("Synthetic vendor")
        );
        assert_eq!(assessment.provenance.author, "Synthetic Author");
        assert_eq!(
            assessment.provenance.details.as_deref(),
            Some("Synthetic target • test")
        );
        assert_eq!(
            assessment.provenance.source_url.as_deref(),
            Some("https://example.invalid/source")
        );
        assert_eq!(
            assessment.provenance.declared_dataset_license,
            OPRA_DECLARED_DATASET_LICENSE
        );
        assert_eq!(assessment.provenance.individual_source_license, None);
        assert_eq!(assessment.provenance.catalog_snapshot, None);
    }

    #[test]
    fn missing_record_returns_typed_error() {
        assert_eq!(
            assess_profile(&test_catalog(vec![], 0.0), "missing"),
            Err(AssessmentError::ProfileNotFound)
        );
    }

    #[test]
    fn assessment_consumes_the_existing_parser_catalog_contract() {
        let jsonl = concat!(
            r#"{"type":"vendor","id":"v","data":{"name":"Synthetic vendor"}}"#,
            "\n",
            r#"{"type":"product","id":"p","data":{"vendor_id":"v","name":"Synthetic headphone","type":"headphones","subtype":"over_the_ear"}}"#,
            "\n",
            r#"{"type":"eq","id":"e","data":{"product_id":"p","author":"Synthetic author","type":"parametric_eq","parameters":{"gain_db":0,"bands":[{"type":"peak_dip","frequency":1000,"gain_db":0,"q":1}]}}}"#,
        );
        let catalog = crate::opra_catalog::parse_catalog(jsonl.as_bytes()).unwrap();
        let assessment = assess_profile(&catalog, "e").unwrap();
        assert_eq!(assessment.verdict, Verdict::Compatible);
        assert_eq!(assessment.provenance.product_id.as_deref(), Some("p"));
        assert_eq!(assessment.provenance.vendor_id.as_deref(), Some("v"));
        assert!(assessment.preview_preset.is_some());
    }
}
