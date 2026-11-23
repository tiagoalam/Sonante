//! Strict, local-only import of the Equalizer APO ParametricEQ text format.
//! Imported files are user supplied; this module does not fetch profile data.
use crate::equalizer::{EqBand, EqFilterType, EqPreset, MAX_BANDS};
use std::collections::BTreeMap;

pub(crate) const MAX_BYTES: usize = 32_768;

#[derive(Debug)]
pub(crate) struct Imported {
    pub preset: EqPreset,
    pub source_filter_count: usize,
}

pub(crate) fn parse(document: &str) -> Result<Imported, &'static str> {
    if document.len() > MAX_BYTES || document.trim().is_empty() {
        return Err("EQ_AUTOEQ_IMPORT_INVALID");
    }
    let mut preamp = None;
    let mut filters = BTreeMap::<usize, EqBand>::new();
    for raw in document.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        if fields
            .first()
            .is_some_and(|v| v.eq_ignore_ascii_case("Preamp:"))
        {
            if preamp.is_some() || fields.len() != 3 || !fields[2].eq_ignore_ascii_case("dB") {
                return Err("EQ_AUTOEQ_IMPORT_INVALID");
            }
            preamp = Some(number(fields[1])?);
            continue;
        }
        if fields
            .first()
            .is_some_and(|v| v.eq_ignore_ascii_case("Filter"))
        {
            if fields.len() != 12 {
                return Err("EQ_AUTOEQ_IMPORT_INVALID");
            }
            let index = fields[1]
                .strip_suffix(':')
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|v| *v > 0 && *v <= MAX_BANDS)
                .ok_or("EQ_AUTOEQ_IMPORT_INVALID")?;
            let enabled = if fields[2].eq_ignore_ascii_case("ON") {
                true
            } else if fields[2].eq_ignore_ascii_case("OFF") {
                false
            } else {
                return Err("EQ_AUTOEQ_IMPORT_INVALID");
            };
            let filter_type = if fields[3].eq_ignore_ascii_case("PK") {
                EqFilterType::Peak
            } else if fields[3].eq_ignore_ascii_case("LSC") {
                EqFilterType::LowShelf
            } else if fields[3].eq_ignore_ascii_case("HSC") {
                EqFilterType::HighShelf
            } else {
                return Err("EQ_AUTOEQ_IMPORT_INVALID");
            };
            if !fields[4].eq_ignore_ascii_case("Fc")
                || !fields[6].eq_ignore_ascii_case("Hz")
                || !fields[7].eq_ignore_ascii_case("Gain")
                || !fields[9].eq_ignore_ascii_case("dB")
                || !fields[10].eq_ignore_ascii_case("Q")
            {
                return Err("EQ_AUTOEQ_IMPORT_INVALID");
            }
            let band = EqBand {
                id: format!("autoeq_{index:02}"),
                enabled,
                filter_type,
                frequency_hz: number(fields[5])?,
                gain_db: number(fields[8])?,
                q: number(fields[11])?,
            };
            if filters.insert(index, band).is_some() {
                return Err("EQ_AUTOEQ_IMPORT_INVALID");
            }
            continue;
        }
        return Err("EQ_AUTOEQ_IMPORT_INVALID");
    }
    if filters.is_empty() || filters.keys().copied().ne(1..=filters.len()) {
        return Err("EQ_AUTOEQ_IMPORT_INVALID");
    }

    // The current editor/store uses ten slots. Preserve every source filter in
    // order, then fill only the remaining slots with disabled neutral controls.
    let source_filter_count = filters.len();
    let mut preset = crate::equalizer::flat_preset();
    preset.id = "autoeq_preview".into();
    preset.name = "AutoEQ import".into();
    preset.preamp_db = preamp.unwrap_or(0.0);
    for band in &mut preset.bands {
        band.enabled = false;
        band.gain_db = 0.0;
    }
    for (slot, (_, band)) in preset.bands.iter_mut().zip(filters) {
        *slot = band;
    }
    crate::equalizer::validate_v1(&preset).map_err(|_| "EQ_AUTOEQ_IMPORT_INVALID")?;
    crate::shared_dsp::safety::assess(&preset, 48_000).map_err(|_| "EQ_AUTOEQ_UNSAFE_HEADROOM")?;
    Ok(Imported {
        preset,
        source_filter_count,
    })
}

fn number(value: &str) -> Result<f64, &'static str> {
    let value = value
        .parse::<f64>()
        .map_err(|_| "EQ_AUTOEQ_IMPORT_INVALID")?;
    if !value.is_finite() {
        return Err("EQ_AUTOEQ_IMPORT_INVALID");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(filters: &str, preamp: &str) -> String {
        format!("# user supplied AutoEq-style profile\n{preamp}{filters}")
    }

    #[test]
    fn imports_supported_types_preamp_off_filters_and_variable_counts_without_loss() {
        let text = file(
            "Filter 1: ON PK Fc 105 Hz Gain 5.8 dB Q 0.70\nFilter 2: OFF LSC Fc 80 Hz Gain -2.0 dB Q 0.8\nFilter 3: ON HSC Fc 10000 Hz Gain 1.2 dB Q 0.7\n",
            "Preamp: +2.5 dB\n",
        );
        let imported = parse(&text).unwrap();
        assert_eq!(imported.source_filter_count, 3);
        assert_eq!(imported.preset.preamp_db, 2.5);
        assert_eq!(imported.preset.bands[0].filter_type, EqFilterType::Peak);
        assert!(!imported.preset.bands[1].enabled);
        assert_eq!(imported.preset.bands[1].gain_db, -2.0);
        assert_eq!(
            imported.preset.bands[2].filter_type,
            EqFilterType::HighShelf
        );
        assert!(imported.preset.bands[3..]
            .iter()
            .all(|b| !b.enabled && b.gain_db == 0.0));

        let ten = (1..=MAX_BANDS)
            .map(|i| format!("Filter {i}: ON PK Fc {} Hz Gain 0 dB Q 1\n", 100 * i))
            .collect::<String>();
        assert_eq!(
            parse(&file(&ten, "Preamp: -3.25 dB\n"))
                .unwrap()
                .source_filter_count,
            MAX_BANDS
        );
        let one = parse("Filter 1: ON PK Fc 1000 Hz Gain 0 dB Q 1\n").unwrap();
        assert_eq!(one.source_filter_count, 1);
        assert_eq!(one.preset.preamp_db, 0.0);

        let formatted = parse("# profile\r\n\r\nPreamp:\t-1.25 dB\r\nFilter 1:\tON\tPK Fc\t1200 Hz Gain\t-0.5 dB Q\t1.2\r\n").unwrap();
        assert_eq!(formatted.preset.preamp_db, -1.25);
        assert_eq!(formatted.preset.bands[0].frequency_hz, 1200.0);
        assert_eq!(formatted.preset.bands[0].gain_db, -0.5);
    }

    #[test]
    fn malformed_unknown_non_finite_incomplete_and_out_of_range_inputs_are_rejected() {
        for text in [
            "",
            "# no filters",
            "Preamp: NaN dB\nFilter 1: ON PK Fc 100 Hz Gain 1 dB Q 1\n",
            "Preamp: 1 dB\nPreamp: 2 dB\nFilter 1: ON PK Fc 100 Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc inf Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain -inf dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain 1 dB Q NaN\n",
            "Filter 1: ON AP Fc 100 Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain 1 dB\n",
            "Filter 2: ON PK Fc 100 Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain 1 dB Q 1\nFilter 1: OFF PK Fc 200 Hz Gain 0 dB Q 1\n",
            "Filter 1: ON PK Fc 19 Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc 20001 Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain 16 dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain 1 dB Q 0.01\n",
            "Preamp: -24.1 dB\nFilter 1: ON PK Fc 100 Hz Gain 1 dB Q 1\n",
            "Preamp: 12.1 dB\nFilter 1: ON PK Fc 100 Hz Gain 1 dB Q 1\n",
            "Filter 1: ON PK Fc 100 Hz Gain 1 dB Q 1\nUnknown: ignored?\n",
        ] {
            assert_eq!(
                parse(text).unwrap_err(),
                "EQ_AUTOEQ_IMPORT_INVALID",
                "{text}"
            );
        }
        let eleven = (1..=11)
            .map(|i| format!("Filter {i}: ON PK Fc 100 Hz Gain 0 dB Q 1\n"))
            .collect::<String>();
        assert_eq!(parse(&eleven).unwrap_err(), "EQ_AUTOEQ_IMPORT_INVALID");
        assert_eq!(
            parse(&"x".repeat(MAX_BYTES + 1)).unwrap_err(),
            "EQ_AUTOEQ_IMPORT_INVALID"
        );
    }

    #[test]
    fn high_requested_preamp_is_preserved_and_existing_headroom_policy_remains_authoritative() {
        let imported = parse("Preamp: 6 dB\nFilter 1: ON PK Fc 1000 Hz Gain 2 dB Q 1\n").unwrap();
        assert_eq!(imported.preset.preamp_db, 6.0);
        let budget = crate::shared_dsp::safety::assess(&imported.preset, 48_000).unwrap();
        assert!(budget.effective_preamp_db <= -1.0 - budget.filter_bound_db);

        let filters = (1..=MAX_BANDS)
            .map(|i| format!("Filter {i}: ON PK Fc 1000 Hz Gain 12 dB Q 1\n"))
            .collect::<String>();
        let unsafe_profile = format!("Preamp: 12 dB\n{filters}");
        assert_eq!(
            parse(&unsafe_profile).unwrap_err(),
            "EQ_AUTOEQ_UNSAFE_HEADROOM"
        );
    }
}
