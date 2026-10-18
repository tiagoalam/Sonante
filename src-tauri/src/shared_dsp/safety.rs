//! Preventive gain budget for the pinned PCM48k / AsyncSinc Balanced pipeline.
//! Bounds impulse absolute sums (including tails), NOT a sampled magnitude curve.
//! No clipping/limiting, no makeup gain. Settings beyond the budget are refused.
use crate::equalizer::{EqBand, EqFilterType, EqPreset};
use serde::Serialize;
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Protection {
    pub requested_preamp_db: f64,
    pub effective_preamp_db: f64,
    pub attenuation_db: f64,
    pub filter_bound_db: f64,
    pub resampler_bound_db: f64,
    pub ceiling_db: f64,
}
// All 512 pinned sinc phases are bounded below 3.2 L1; quadratic interpolation
// has weight L1 <=1.25. 4 covers the complete interpolator; 1dB further reserve.
// See resampler-bound.json + pinned Rubato source audit in the private report.
pub(crate) const RESAMPLER_BOUND: f64 = 4.;
pub(crate) const CEILING_DB: f64 = -1.;
pub(crate) const MAX_ATTENUATION_DB: f64 = 48.;
pub(crate) fn coefficients(b: &EqBand, rate: u32) -> [f64; 5] {
    let a = 10f64.powf(b.gain_db / 40.);
    let w = 2. * std::f64::consts::PI * b.frequency_hz / rate as f64;
    let c = w.cos();
    let alpha = w.sin() / (2. * b.q);
    let z = 2. * a.sqrt() * alpha;
    let v = match b.filter_type {
        EqFilterType::Peak => [
            1. + alpha * a,
            -2. * c,
            1. - alpha * a,
            1. + alpha / a,
            -2. * c,
            1. - alpha / a,
        ],
        EqFilterType::LowShelf => [
            a * ((a + 1.) - (a - 1.) * c + z),
            2. * a * ((a - 1.) - (a + 1.) * c),
            a * ((a + 1.) - (a - 1.) * c - z),
            (a + 1.) + (a - 1.) * c + z,
            -2. * ((a - 1.) + (a + 1.) * c),
            (a + 1.) + (a - 1.) * c - z,
        ],
        EqFilterType::HighShelf => [
            a * ((a + 1.) + (a - 1.) * c + z),
            -2. * a * ((a - 1.) + (a + 1.) * c),
            a * ((a + 1.) + (a - 1.) * c - z),
            (a + 1.) - (a - 1.) * c + z,
            2. * ((a - 1.) - (a + 1.) * c),
            (a + 1.) - (a - 1.) * c - z,
        ],
    };
    [
        v[0] / v[3],
        v[1] / v[3],
        v[2] / v[3],
        v[4] / v[3],
        v[5] / v[3],
    ]
}
fn impulse_bound(b: &EqBand, rate: u32) -> Result<f64, ()> {
    if !b.enabled || b.gain_db == 0. {
        return Ok(1.);
    }
    let [b0, b1, b2, a1, a2] = coefficients(b, rate);
    if ![b0, b1, b2, a1, a2].iter().all(|v| v.is_finite()) {
        return Err(());
    }
    let disc = a1 * a1 - 4. * a2;
    let (r, gap) = if disc < 0. {
        (a2.sqrt(), (-disc).sqrt())
    } else {
        let d = disc.sqrt();
        (((-a1 + d) / 2.).abs().max(((-a1 - d) / 2.).abs()), d)
    };
    if !r.is_finite() || r >= 1. || gap < 1e-10 {
        return Err(());
    }
    let mut prev = b0;
    let mut curr = b1 - a1 * b0;
    let mut sum = b0.abs() + curr.abs();
    let mut next = b2 - a1 * curr - a2 * prev;
    prev = curr;
    curr = next;
    sum += curr.abs();
    // Exact finite prefix; analytic geometric envelope for ALL remaining samples.
    for _ in 3..4096 {
        next = -a1 * curr - a2 * prev;
        prev = curr;
        curr = next;
        sum += curr.abs();
    }
    let tail1 = -a1 * curr - a2 * prev;
    let tail2 = -a1 * tail1 - a2 * curr;
    // Exact modal amplitudes for distinct real or conjugate poles.
    let tail = if disc < 0. {
        let im = gap / 2.;
        let re = -a1 / 2.;
        (tail1 * tail1 + ((tail2 - re * tail1) / im).powi(2)).sqrt() / (1. - r)
    } else {
        let p1 = (-a1 + gap) / 2.;
        let p2 = (-a1 - gap) / 2.;
        ((tail2 - p2 * tail1) / gap).abs() / (1. - p1.abs())
            + ((p1 * tail1 - tail2) / gap).abs() / (1. - p2.abs())
    };
    let bound = (sum + tail1.abs() + tail) * (1. + 1e-8);
    if !bound.is_finite() || bound < 0. {
        Err(())
    } else {
        Ok(bound.max(1.))
    }
}
pub(crate) fn assess(p: &EqPreset, rate: u32) -> Result<Protection, ()> {
    crate::equalizer::validate_preset(p).map_err(|_| ())?;
    let mut filter_bound_db = 0.;
    for b in &p.bands {
        filter_bound_db += 20. * impulse_bound(b, rate)?.log10();
    }
    let resampler_bound_db = 20. * RESAMPLER_BOUND.log10();
    let allowed = CEILING_DB - filter_bound_db - resampler_bound_db;
    let effective = p.preamp_db.min(0.) + allowed;
    let attenuation = p.preamp_db - effective;
    if !attenuation.is_finite() || attenuation > MAX_ATTENUATION_DB {
        return Err(());
    }
    Ok(Protection {
        requested_preamp_db: p.preamp_db,
        effective_preamp_db: effective,
        attenuation_db: attenuation,
        filter_bound_db,
        resampler_bound_db,
        ceiling_db: CEILING_DB,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stacked_boosts_positive_preamp_and_extremes_are_bounded_or_refused() {
        let mut p = crate::equalizer::flat_preset();
        p.preamp_db = 12.;
        for b in &mut p.bands {
            b.gain_db = 12.;
            b.frequency_hz = 1000.;
        }
        assert!(assess(&p, 48000).is_err());
        for freq in [20., 31., 997., 20000.] {
            for q in [0.1, 0.707, 1., 30.] {
                for gain in [-12., 12.] {
                    for kind in [
                        EqFilterType::Peak,
                        EqFilterType::LowShelf,
                        EqFilterType::HighShelf,
                    ] {
                        let mut p = crate::equalizer::flat_preset();
                        for b in &mut p.bands {
                            b.enabled = false
                        }
                        let b = &mut p.bands[0];
                        b.enabled = true;
                        b.frequency_hz = freq;
                        b.q = q;
                        b.gain_db = gain;
                        b.filter_type = kind;
                        if let Ok(s) = assess(&p, 48000) {
                            assert!(
                                s.effective_preamp_db + s.filter_bound_db + s.resampler_bound_db
                                    <= CEILING_DB + 1e-9
                            );
                            assert!(s.effective_preamp_db <= p.preamp_db);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn bound_dominates_long_impulses_and_transients() {
        for q in [0.1, 0.707, 1., 30.] {
            for f in [20., 997., 20000.] {
                let mut p = crate::equalizer::flat_preset();
                p.bands[0].q = q;
                p.bands[0].gain_db = 12.;
                p.bands[0].frequency_hz = f;
                let b = &p.bands[0];
                let bound = impulse_bound(b, 48000).unwrap();
                let [b0, b1, b2, a1, a2] = coefficients(b, 48000);
                let (mut x1, mut x2, mut y1, mut y2) = (0., 0., 0., 0.);
                let mut sum = 0.;
                for n in 0..500000 {
                    let x = if n == 0 { 1. } else { 0. };
                    let y = b0 * x + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
                    x2 = x1;
                    x1 = x;
                    y2 = y1;
                    y1 = y;
                    assert!(y.is_finite());
                    sum += y.abs();
                }
                assert!(sum <= bound * (1. + 1e-8), "{q} {f} {sum} {bound}");
            }
        }
    }
}
