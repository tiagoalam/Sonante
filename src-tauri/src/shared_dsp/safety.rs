//! Preventive gain budget for the pinned consumer-clock PCM48k pipeline.
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
// At48k the qualified MPD producer follows FIFO backpressure; Camilla has no
// rate conversion. The transfer operator is identity (L1=1), not a relaxed
// bound on the old sinc. Runtime readback refuses an unexpected resampler.
// Other nominal rates retain the old conversion envelope; not runtime rates.
pub(crate) const RESAMPLER_BOUND: f64 = 1.;
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
// A finite convolution of prefixes of length K is zero beyond m*(K-1).
// Therefore the INFINITE cascade tail beyond N is bounded by the telescoping
// remainder sum_i tail_i * product_{j!=i} ||h_j||1. No frequency-grid maximum.
const CASCADE_PREFIX: usize = 16384;
fn cascade_bound(p:&EqPreset,rate:u32,product:f64)->f64 {
    if rate!=48000 {return product;}
    let bands:Vec<_>=p.bands.iter().filter(|b|b.enabled&&b.gain_db!=0.).collect();
    if bands.is_empty(){return 1.;}
    let k=CASCADE_PREFIX/bands.len();let mut h=vec![0.;CASCADE_PREFIX];h[0]=1.;
    let mut tails=0.;let mut numerical=0.;let mut numeric_input=1.;
    let gamma=64.*f64::EPSILON/(1.-64.*f64::EPSILON);
    for band in bands {
        let [b0,b1,b2,a1,a2]=coefficients(band,rate);
        let l=match impulse_bound(band,rate){Ok(v)=>v,Err(_)=>return product};
        let d=a1*a1-4.*a2;let gap=d.abs().sqrt();let r=if d<0.{a2.sqrt()}else{((-a1+gap)/2.).abs().max(((-a1-gap)/2.).abs())};
        // Near repeated/unit poles keep the established product bound.
        if !r.is_finite()||r>=1.-1e-6||gap<1e-3{return product;}
        let h0=b0;let h1=b1-a1*h0;let h2=b2-a1*h1-a2*h0;let h3=-a1*h2-a2*h1;
        let (g,tail)=if d<0. {
            let g=2.*r/(gap*(1.-r));
            let amplitude=(h2*h2+((h3+a1*h2/2.)/(gap/2.)).powi(2)).sqrt();
            (g,amplitude*r.powi((k-2)as i32)/(1.-r))
        }else{
            let p1=(-a1+gap)/2.;let p2=(-a1-gap)/2.;
            let g=(p1.abs()/(1.-p1.abs())+p2.abs()/(1.-p2.abs()))/gap;
            let c1=(h3-p2*h2)/gap;let c2=(p1*h2-h3)/gap;
            (g,c1.abs()*p1.abs().powi((k-2)as i32)/(1.-p1.abs())+c2.abs()*p2.abs().powi((k-2)as i32)/(1.-p2.abs()))
        };
        let g=g*(1.+1e-6);let bs=4.*(b0.abs()+b1.abs()+b2.abs()+1.);let aa=4.*(a1.abs()+a2.abs()+1.);
        // Initial modal arithmetic floor, independent of a potentially cancelled
        // h2/h3. All bounds are positive; no subtraction of near-equal products.
        let tail=(tail+64.*f64::EPSILON*(bs+aa+1.)*g)*(1.+1e-6);
        tails+=tail/l;
        // Gamma64 and enlarged B/A cover both DF1 prefix evaluation and
        // Camilla's transposed DF2 states: Y <= L*X+G*gamma*(B*X+A*Y).
        let denominator=1.-g*gamma*aa;
        if denominator<=0. {return product;}
        let ln=(l+g*gamma*bs)/denominator;
        let local=g*gamma*(bs+aa*ln)*numeric_input+1e-280;
        numerical=numerical*l+local;numeric_input*=ln;
        let(mut x1,mut x2,mut y1,mut y2)=(0.,0.,0.,0.);
        for x in &mut h {let input=*x;let y=b0*input+b1*x1+b2*x2-a1*y1-a2*y2;x2=x1;x1=input;y2=y1;y1=y;*x=y;}
    }
    // N*error bounds the computed prefix's absolute-sum error; one more error
    // term accounts for the runtime's bounded arithmetic on arbitrary input.
    let candidate=(h.iter().map(|v|v.abs()).sum::<f64>()+product*tails+(CASCADE_PREFIX as f64+1.)*numerical)*(1.+1e-6);
    if candidate.is_finite(){candidate.max(1.).min(product)}else{product}
}

pub(crate) fn assess(p: &EqPreset, rate: u32) -> Result<Protection, ()> {
    crate::equalizer::validate_preset(p).map_err(|_| ())?;
    if rate==0{return Err(());}
    let mut product=1.;
    for b in &p.bands {product*=impulse_bound(b,rate)?;}
    let filter_bound_db=20.*cascade_bound(p,rate,product).log10();
    let resampler=if rate==48000{RESAMPLER_BOUND}else{4.};
    let resampler_bound_db=20.*resampler.log10();
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
    #[test]
    fn headroom_flat_keeps_reserve_manual_attenuation_and_rate_scope(){
        let mut p=crate::equalizer::flat_preset();let flat=assess(&p,48000).unwrap();
        assert!((flat.effective_preamp_db-(-1.)).abs()<1e-10);
        assert!((assess(&p,44100).unwrap().effective_preamp_db-(-1.-20.*4f64.log10())).abs()<1e-10);
        p.preamp_db=-6.;assert!((assess(&p,48000).unwrap().effective_preamp_db-(flat.effective_preamp_db-6.)).abs()<1e-10);
        p.preamp_db=6.;assert_eq!(assess(&p,48000).unwrap().effective_preamp_db,flat.effective_preamp_db);
        p.bands[0].gain_db=f64::NAN;assert!(assess(&p,48000).is_err());
    }
    #[test]
    fn headroom_cascade_cancellation_has_a_bounded_infinite_tail(){
        let mut p=crate::equalizer::flat_preset();
        for(i,gain)in[6.,-6.].into_iter().enumerate(){p.bands[i].enabled=true;p.bands[i].frequency_hz=1000.;p.bands[i].q=0.707;p.bands[i].gain_db=gain;}
        let old=p.bands.iter().map(|b|impulse_bound(b,48000).unwrap()).product::<f64>();let new=cascade_bound(&p,48000,old);
        assert!(new<1.01,"{new}");assert!(old>2.);assert!(new>=1.);
    }
    #[test]
    fn headroom_cascade_bound_covers_long_impulses_and_adversarial_transients(){
        for q in [0.1,0.707,30.] {
            let mut p=crate::equalizer::flat_preset();for(i,b)in p.bands.iter_mut().take(8).enumerate(){b.enabled=true;b.q=q;b.frequency_hz=[20.,50.,125.,500.,1000.,4000.,8000.,20000.][i];b.gain_db=if i%2==0{6.}else{-6.};}
            let product=p.bands.iter().map(|b|impulse_bound(b,48000).unwrap()).product::<f64>();let bound=cascade_bound(&p,48000,product);assert!(bound>=1.&&bound<=product);
            let apply=|mut data:Vec<f64>|{for b in p.bands.iter().filter(|b|b.enabled){let[b0,b1,b2,a1,a2]=coefficients(b,48000);let(mut x1,mut x2,mut y1,mut y2)=(0.,0.,0.,0.);for x in &mut data{let input=*x;let y=b0*input+b1*x1+b2*x2-a1*y1-a2*y2;x2=x1;x1=input;y2=y1;y1=y;*x=y;}}data};
            let mut input=vec![0.;500000];input[0]=1.;let impulse=apply(input);let l1=impulse.iter().map(|x|x.abs()).sum::<f64>();assert!(l1<=bound*(1.+1e-8),"q={q} measured={l1} bound={bound}");
            let adversarial=impulse[..16384].iter().rev().map(|x|if *x>=0.{0.999}else{-0.999}).collect();let signal=apply(adversarial);assert!(signal.iter().all(|x|x.is_finite()&&x.abs()<=bound));
        }
    }
    #[test]
    #[ignore="owned converter export for file/null proof only"]
    fn headroom_export_real_converter(){
        assert_eq!(std::env::var("SONANTE_DIRECT_GAIN_QA").unwrap(),"owned-file-null");
        let mut presets=crate::equalizer::built_in_presets();let mut p=crate::equalizer::flat_preset();p.id="cancel_pair".into();p.name="Synthetic inverse pair".into();for(i,gain)in[6.,-6.].into_iter().enumerate(){p.bands[i].enabled=true;p.bands[i].frequency_hz=1000.;p.bands[i].q=0.707;p.bands[i].gain_db=gain;}presets.push(p);
        let mut edge=crate::equalizer::flat_preset();edge.id="synthetic_extreme".into();edge.name="Synthetic supported limits".into();for(i,b)in edge.bands.iter_mut().enumerate(){b.enabled=true;b.frequency_hz=[20.,40.,80.,160.,320.,640.,1250.,2500.,10000.,20000.][i];b.gain_db=if i%2==0{6.}else{-6.};b.q=if i%3==0{30.}else{0.1};}presets.push(edge);
        let mut mix=crate::equalizer::flat_preset();mix.id="synthetic_cascade".into();mix.name="Synthetic eight-band cascade".into();for(i,b)in mix.bands.iter_mut().take(8).enumerate(){b.enabled=true;b.q=0.707;b.frequency_hz=[20.,50.,125.,500.,1000.,4000.,8000.,20000.][i];b.gain_db=if i%2==0{6.}else{-6.};}presets.push(mix);
        let mut narrow=crate::equalizer::flat_preset();narrow.id="synthetic_low_highq".into();narrow.name="Synthetic long tail".into();narrow.bands[0].enabled=true;narrow.bands[0].frequency_hz=20.;narrow.bands[0].gain_db=12.;narrow.bands[0].q=30.;presets.push(narrow);
        for gain in [-6.,6.]{let mut manual=crate::equalizer::flat_preset();manual.id=if gain<0.{"manual_negative"}else{"manual_positive"}.into();manual.name=manual.id.clone();manual.preamp_db=gain;presets.push(manual);}
        let mut cases=Vec::new();for p in presets {for rate in [44100,48000]{
            let old_filter_db=p.bands.iter().map(|b|20.*impulse_bound(b,rate).unwrap().log10()).sum::<f64>();
            let protection=assess(&p,rate);let mut item=serde_json::json!({"preset":p,"rate":rate,"old_filter_bound_db":old_filter_db,"old_effective_preamp_db":p.preamp_db.min(0.)-1.-20.*4f64.log10()-old_filter_db});
            if let Ok(value)=protection{item["protection"]=serde_json::to_value(value).unwrap();if rate==48000{let format=super::super::peq::PcmFormat{sample_rate_hz:rate,channels:2};item["config"]=serde_json::to_value(super::super::peq::convert_preset(&p,format).unwrap()).unwrap();item["reference_config"]=serde_json::to_value(super::super::peq::convert_reference(&p,format).unwrap()).unwrap();}}
            cases.push(item);
        }}
        for name in ["equalizer-coefficients.json","equalizer-collection.json"] {
            let path=format!("/build/app-source/tests/fixtures/{name}");let mut fixtures:Vec<serde_json::Value>=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            for item in &mut fixtures {
                if name=="equalizer-collection.json" {
                    let id=item["preset"]["id"].as_str().unwrap();let current=crate::equalizer::built_in_presets().into_iter().find(|p|p.id==id).unwrap();item["preset"]=serde_json::to_value(current).unwrap();
                }
                let p:EqPreset=serde_json::from_value(item["preset"].clone()).unwrap();let result=assess(&p,48000);let admitted=result.is_ok();
                item["protection"]=result.ok().map(|v|serde_json::to_value(v).unwrap()).unwrap_or(serde_json::Value::Null);
                item["coefficients"]=serde_json::to_value(p.bands.iter().map(|b|coefficients(b,48000)).collect::<Vec<_>>()).unwrap();
                if item.get("admitted").is_some(){item["admitted"]=serde_json::json!(admitted);}
                if item.get("config").is_some()&&admitted{item["config"]=serde_json::to_value(super::super::peq::convert_preset(&p,super::super::peq::PcmFormat{sample_rate_hz:48000,channels:2}).unwrap()).unwrap();}
            }
            std::fs::write(format!("/build/app-source/headroom-fixture-{name}"),serde_json::to_vec_pretty(&fixtures).unwrap()).unwrap();
        }
        std::fs::write("/build/app-source/headroom-export.json",serde_json::to_vec_pretty(&cases).unwrap()).unwrap();println!("[HeadroomQA] converter_export_cases={}",cases.len());
    }

}
