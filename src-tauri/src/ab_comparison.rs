//! Bounded, explicit A/B sample preparation. No output, queue or persisted data.
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Seek, SeekFrom},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    errors::Error as DecodeError,
    formats::{FormatOptions, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
    units::{Time, TimeBase},
};
pub(crate) const RATE: u32 = 48_000;
pub(crate) const MAX_SOURCE_BYTES: usize = 256 * 1024 * 1024;
const MAX_RATE: u32 = 192_000;
const MIN_FRAMES: usize = RATE as usize * 3;
const DEADLINE: Duration = Duration::from_secs(40);
// Only fixed identifiers and numeric media facts cross IPC. Decoder/parser
// messages may contain input data and are deliberately never serialized.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct DecodeFailure {
    code: String,
    codec: Option<&'static str>,
    sample_rate: Option<u32>,
    channels: Option<u32>,
}
impl From<&str> for DecodeFailure {
    fn from(code: &str) -> Self {
        let safe = match code {
            "AB_BUSY" | "AB_SHARED_ONLY" | "AB_SOURCE" | "AB_CHANGED" | "AB_SAME_TRACK"
            | "AB_TRACKS" | "AB_NETWORK" | "AB_LIMIT" | "AB_STORAGE" | "AB_FORMAT"
            | "AB_DECODE" | "AB_SEEK" | "AB_TOO_SHORT" | "AB_INVALID" | "AB_PAUSE"
            | "AB_CONTAINER" | "AB_CODEC" | "AB_SAMPLE_RATE" | "AB_CHANNELS" | "AB_FORMAT_INFO"
            | "AB_FORMAT_CHANGE" | "AB_RESAMPLE" => code,
            _ => "AB_UNAVAILABLE",
        };
        Self {
            code: safe.to_owned(),
            codec: None,
            sample_rate: None,
            channels: None,
        }
    }
}
impl From<String> for DecodeFailure {
    fn from(code: String) -> Self {
        Self::from(code.as_str())
    }
}
fn codec_label(codec: symphonia::core::codecs::CodecType) -> Option<&'static str> {
    use symphonia::core::codecs::*;
    Some(match codec {
        CODEC_TYPE_FLAC => "FLAC",
        CODEC_TYPE_ALAC => "ALAC",
        CODEC_TYPE_MP1 => "MP1",
        CODEC_TYPE_MP2 => "MP2",
        CODEC_TYPE_MP3 => "MP3",
        CODEC_TYPE_AAC => "AAC",
        CODEC_TYPE_VORBIS => "Vorbis",
        CODEC_TYPE_OPUS => "Opus",
        CODEC_TYPE_WAVPACK => "WavPack",
        CODEC_TYPE_MONKEYS_AUDIO => "APE",
        CODEC_TYPE_TTA => "TTA",
        CODEC_TYPE_WMA => "WMA",
        CODEC_TYPE_DCA => "DTS",
        CODEC_TYPE_EAC3 => "E-AC-3",
        CODEC_TYPE_AC4 => "AC-4",
        CODEC_TYPE_SPEEX => "Speex",
        CODEC_TYPE_MUSEPACK => "Musepack",
        _ if symphonia::default::get_codecs()
            .get_codec(codec)
            .is_some_and(|descriptor| descriptor.short_name.starts_with("pcm_")) =>
        {
            "PCM"
        }
        _ => return None,
    })
}
fn signature_codec(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"DSD ") {
        Some("DSD (DSF)")
    } else if bytes.starts_with(b"FRM8") && bytes.get(12..16) == Some(b"DSD ") {
        Some("DSD (DSDIFF)")
    } else if bytes.starts_with(b"wvpk") {
        Some("WavPack")
    } else if bytes.starts_with(b"MAC ") {
        Some("APE")
    } else {
        None
    }
}
fn media_failure(code: &str, params: &symphonia::core::codecs::CodecParameters) -> DecodeFailure {
    DecodeFailure {
        codec: codec_label(params.codec),
        sample_rate: params.sample_rate,
        channels: params.channels.map(|v| v.count() as u32),
        ..code.into()
    }
}
fn input_format(
    params: &symphonia::core::codecs::CodecParameters,
) -> Result<(u32, usize), DecodeFailure> {
    let rate = params
        .sample_rate
        .ok_or_else(|| media_failure("AB_FORMAT_INFO", params))?;
    if !(8_000..=MAX_RATE).contains(&rate) {
        return Err(media_failure("AB_SAMPLE_RATE", params));
    }
    let channels = params
        .channels
        .map(|v| v.count())
        .ok_or_else(|| media_failure("AB_FORMAT_INFO", params))?;
    if !matches!(channels, 1 | 2) {
        return Err(media_failure("AB_CHANNELS", params));
    }
    Ok((rate, channels))
}
pub(crate) type Check = Arc<dyn Fn() -> bool + Send + Sync>;
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Side {
    A,
    B,
}
impl Side {
    fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }
}
pub(crate) fn valid_part_path(path: &str) -> bool {
    if path.len() > 4096 || !path.starts_with("/library/parts/") {
        return false;
    }
    let Ok(decoded) = urlencoding::decode(path) else {
        return false;
    };
    if path.contains(['?', '#', '\\'])
        || path.chars().any(char::is_control)
        || decoded.contains(['?', '#', '\\'])
        || decoded.chars().any(char::is_control)
        || decoded.to_ascii_lowercase().contains("x-plex-token")
    {
        return false;
    }
    let parts = decoded.split('/').collect::<Vec<_>>();
    parts.len() >= 5
        && parts[3].bytes().all(|b| b.is_ascii_digit())
        && !parts[3].is_empty()
        && parts[1..]
            .iter()
            .all(|p| !p.is_empty() && *p != "." && *p != "..")
}
#[derive(Clone, Serialize)]
pub(crate) struct Track {
    pub key: String,
    pub title: String,
    pub duration: Option<f64>,
    pub number: usize,
}
pub(crate) fn track(
    key: &str,
    title: &str,
    duration: Option<f64>,
    number: usize,
) -> Result<Track, String> {
    use sha2::{Digest, Sha256};
    if title.is_empty() || title.len() > 4096 || title.chars().any(char::is_control) {
        return Err("AB_TRACKS".into());
    }
    Ok(Track {
        key: format!(
            "{:x}",
            Sha256::digest(format!("{key}\0{title}\0{duration:?}"))
        ),
        title: title.to_owned(),
        duration: duration.filter(|n| n.is_finite() && *n > 0.),
        number,
    })
}
pub(crate) struct Input {
    pub media: Box<dyn MediaSource>,
    pub alive: Check,
    pub validate: Check,
    pub identity: String,
}
#[derive(Default)]
struct Slot {
    input: Option<Input>,
    identity: Option<String>,
    alive: Option<Check>,
    ready: bool,
}
pub(crate) struct Job {
    pub token: String,
    cancel: AtomicBool,
    slots: Mutex<[Slot; 2]>,
}
impl Job {
    pub(crate) fn current(&self) -> bool {
        !self.cancel.load(Ordering::Acquire)
    }
    pub(crate) fn local_input(&self, side: Side) -> Result<bool, String> {
        // This identity was built by the native source resolver, never accepted
        // from IPC. It selects the retained Local admission fence below.
        self.slots
            .lock()
            .map_err(|_| "AB_UNAVAILABLE".to_string())?[side.index()]
        .identity
        .as_ref()
        .map(|identity| identity.starts_with("local:"))
        .ok_or_else(|| "AB_CHANGED".to_string())
    }
    pub(crate) fn valid(&self) -> bool {
        self.current()
            && self.slots.lock().is_ok_and(|s| {
                s.iter()
                    .all(|v| v.ready && v.alive.as_ref().is_some_and(|c| c()))
            })
    }
    pub(crate) fn install(&self, side: Side, input: Input) -> Result<(), String> {
        if !self.current() || !(input.validate)() || !(input.alive)() {
            return Err("AB_CHANGED".into());
        }
        let mut slots = self.slots.lock().map_err(|_| "AB_UNAVAILABLE")?;
        let other = 1 - side.index();
        if slots[other].identity.as_ref() == Some(&input.identity) {
            return Err("AB_SAME_TRACK".into());
        }
        slots[side.index()] = Slot {
            identity: Some(input.identity.clone()),
            alive: Some(input.alive.clone()),
            input: Some(input),
            ready: false,
        };
        Ok(())
    }
}
fn owner() -> &'static Mutex<Option<Arc<Job>>> {
    static OWNER: OnceLock<Mutex<Option<Arc<Job>>>> = OnceLock::new();
    OWNER.get_or_init(|| Mutex::new(None))
}
pub(crate) fn begin() -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut owner = owner().lock().map_err(|_| "AB_UNAVAILABLE")?;
    if let Some(old) = owner.take() {
        old.cancel.store(true, Ordering::Release);
    }
    let token = format!("ab_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    *owner = Some(Arc::new(Job {
        token: token.clone(),
        cancel: AtomicBool::new(false),
        slots: Mutex::new([Slot::default(), Slot::default()]),
    }));
    Ok(token)
}
pub(crate) fn job(token: &str) -> Result<Arc<Job>, String> {
    owner()
        .lock()
        .map_err(|_| "AB_UNAVAILABLE")?
        .as_ref()
        .filter(|j| j.token == token && j.current())
        .cloned()
        .ok_or("AB_CHANGED".into())
}
pub(crate) fn cancel(token: &str) {
    if let Ok(mut owner) = owner().lock() {
        if owner.as_ref().is_some_and(|j| j.token == token) {
            if let Some(old) = owner.take() {
                old.cancel.store(true, Ordering::Release);
            }
        }
    }
}
struct BudgetSource {
    inner: Box<dyn MediaSource>,
    check: Check,
    deadline: Instant,
    bytes: usize,
    detected: Arc<Mutex<Option<&'static str>>>,
}
impl BudgetSource {
    fn allowed(&self) -> std::io::Result<()> {
        if !(self.check)() || Instant::now() > self.deadline {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "AB_CANCELLED",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for BudgetSource {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.allowed()?;
        let n = self.inner.read(b)?;
        if self.bytes == 0 {
            if let Ok(mut detected) = self.detected.lock() {
                *detected = signature_codec(&b[..n]);
            }
        }
        self.bytes = self.bytes.saturating_add(n);
        if self.bytes > 512 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "AB_LIMIT",
            ));
        }
        Ok(n)
    }
}
impl Seek for BudgetSource {
    fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
        self.allowed()?;
        self.inner.seek(p)
    }
}
impl MediaSource for BudgetSource {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }
    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
}
fn failure(error: DecodeError) -> String {
    match error {
        DecodeError::Unsupported(_) => "AB_CODEC",
        _ => "AB_DECODE",
    }
    .into()
}
pub(crate) fn parameters(start: f64, seconds: u32) -> Result<(), String> {
    if !start.is_finite() || !(0.0..=36_000.0).contains(&start) || !matches!(seconds, 30 | 60 | 90)
    {
        Err("AB_INVALID".into())
    } else {
        Ok(())
    }
}
pub(crate) fn decode(
    job: Arc<Job>,
    side: Side,
    start: f64,
    seconds: u32,
) -> Result<Vec<u8>, DecodeFailure> {
    parameters(start, seconds)?;
    if !job.current() {
        return Err("AB_CHANGED".into());
    }
    static WORKER: Mutex<()> = Mutex::new(());
    let _worker = WORKER.try_lock().map_err(|_| "AB_BUSY")?;
    if !job.current() {
        return Err("AB_CHANGED".into());
    }
    let input = job.slots.lock().map_err(|_| "AB_UNAVAILABLE")?[side.index()]
        .input
        .take()
        .ok_or("AB_CHANGED")?;
    if !(input.validate)() || !(input.alive)() {
        return Err("AB_CHANGED".into());
    }
    let checked_job = job.clone();
    let policy = input.alive.clone();
    let check: Check = Arc::new(move || checked_job.current() && policy());
    let validate = input.validate;
    let detected = Arc::new(Mutex::new(None));
    let source = BudgetSource {
        inner: input.media,
        check: check.clone(),
        deadline: Instant::now() + DEADLINE,
        bytes: 0,
        detected: detected.clone(),
    };
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let mut format = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|error| DecodeFailure {
            codec: detected.lock().ok().and_then(|value| *value),
            ..DecodeFailure::from(if matches!(error, DecodeError::Unsupported(_)) {
                "AB_CONTAINER"
            } else {
                "AB_DECODE"
            })
        })?
        .format;
    let selected = format.default_track().ok_or("AB_FORMAT_INFO")?;
    let id = selected.id;
    let params = selected.codec_params.clone();
    let (rate, channels) = input_format(&params)?;
    let base = params.time_base.unwrap_or(TimeBase::new(1, rate));
    let tolerance = (2. / rate as f64).max(base.numer as f64 / base.denom as f64 * 1.1);
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|error| {
            media_failure(
                if matches!(error, DecodeError::Unsupported(_)) {
                    "AB_CODEC"
                } else {
                    "AB_DECODE"
                },
                &params,
            )
        })?;
    if start > 0. {
        format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(start),
                    track_id: Some(id),
                },
            )
            .map_err(|_| "AB_SEEK")?;
        decoder.reset();
    }
    let wanted = seconds as usize * rate as usize;
    let mut pcm = Vec::<f32>::with_capacity(wanted * 2);
    let mut previous_end: Option<f64> = None;
    while pcm.len() / 2 < wanted {
        if !check() {
            return Err("AB_CHANGED".into());
        }
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(DecodeError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(media_failure(&failure(e), &params)),
        };
        if packet.track_id() != id {
            continue;
        }
        let time = base.calc_time(packet.ts());
        let at = time.seconds as f64 + time.frac;
        let decoded = decoder
            .decode(&packet)
            .map_err(|error| media_failure(&failure(error), &params))?;
        if decoded.spec().rate != rate || decoded.spec().channels.count() != channels {
            return Err(media_failure("AB_FORMAT_CHANGE", &params));
        }
        if decoded.frames() > rate as usize * 4 {
            return Err(media_failure("AB_DECODE", &params));
        }
        let frames = decoded.frames();
        let end = at + frames as f64 / rate as f64;
        if end <= start {
            continue;
        }
        if pcm.is_empty() && at > start + tolerance {
            return Err("AB_SEEK".into());
        }
        if previous_end.is_some_and(|p| (at - p).abs() > tolerance) {
            return Err("AB_SEEK".into());
        }
        previous_end = Some(end);
        let skip = ((start - at).max(0.) * rate as f64).round() as usize;
        let take = frames.saturating_sub(skip).min(wanted - pcm.len() / 2);
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        samples.copy_interleaved_ref(decoded);
        let src = samples.samples();
        for i in skip..skip + take {
            let a = src[i * channels];
            let b = src[i * channels + channels - 1];
            if !a.is_finite() || !b.is_finite() || a.abs() > 8. || b.abs() > 8. {
                return Err("AB_DECODE".into());
            }
            pcm.push(a);
            pcm.push(b);
        }
    }
    if !check() || !validate() {
        return Err("AB_CHANGED".into());
    }
    let pcm = resample(pcm, rate).map_err(|code| media_failure(&code, &params))?;
    if pcm.len() / 2 < MIN_FRAMES {
        return Err("AB_TOO_SHORT".into());
    }
    if !check() || !validate() {
        return Err("AB_CHANGED".into());
    }
    let encoded = encode(&pcm, rate, channels as u32, start)?;
    job.slots.lock().map_err(|_| "AB_UNAVAILABLE")?[side.index()].ready = true;
    Ok(encoded)
}
fn resample(input: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    if rate == RATE {
        return Ok(input);
    }
    let frames = input.len() / 2;
    let mut converter = Fft::<f32>::new(rate as usize, RATE as usize, 1024, 2, 2, FixedSync::Input)
        .map_err(|_| "AB_RESAMPLE")?;
    let len = converter.process_all_needed_output_len(frames);
    let mut output = vec![0.; len * 2];
    let source = InterleavedSlice::new(&input, 2, frames).map_err(|_| "AB_DECODE")?;
    let mut target = InterleavedSlice::new_mut(&mut output, 2, len).map_err(|_| "AB_DECODE")?;
    let (_, produced) = converter
        .process_all_into_buffer(&source, &mut target, frames, None)
        .map_err(|_| "AB_DECODE")?;
    output.truncate(produced.min(frames.saturating_mul(RATE as usize) / rate as usize) * 2);
    Ok(output)
}
fn encode(samples: &[f32], rate: u32, channels: u32, start: f64) -> Result<Vec<u8>, String> {
    if samples.len() % 2 != 0 || samples.len() > RATE as usize * 90 * 2 {
        return Err("AB_LIMIT".into());
    }
    let mut peak = 0f32;
    let mut sum = 0f64;
    for &v in samples {
        if !v.is_finite() {
            return Err("AB_DECODE".into());
        }
        peak = peak.max(v.abs());
        sum += (v as f64).powi(2);
    }
    let rms = (sum / samples.len().max(1) as f64).sqrt() as f32;
    let mut bytes = Vec::with_capacity(40 + samples.len() * 4);
    bytes.extend_from_slice(b"SAB1");
    for n in [
        RATE,
        2,
        (samples.len() / 2) as u32,
        rate,
        channels,
        (start * 1000.).round() as u32,
    ] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    bytes.extend_from_slice(&peak.to_le_bytes());
    bytes.extend_from_slice(&rms.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    for &v in samples {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    static SERIAL: Mutex<()> = Mutex::new(());
    fn wav(seconds: usize, rate: u32, channels: u16) -> Vec<u8> {
        let frames = seconds * rate as usize;
        let size = frames * channels as usize * 2;
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + size as u32).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(channels.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * channels as u32 * 2).to_le_bytes());
        v.extend((channels * 2).to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend((size as u32).to_le_bytes());
        for i in 0..frames {
            let value =
                ((i as f64 * 440. * std::f64::consts::TAU / rate as f64).sin() * 8192.) as i16;
            for _ in 0..channels {
                v.extend(value.to_le_bytes());
            }
        }
        v
    }
    #[test]
    fn ab_format_failures_preserve_only_known_codec_and_numeric_facts() {
        use symphonia::core::{audio::Channels, codecs::*};
        let mut params = CodecParameters::new();
        params
            .for_codec(CODEC_TYPE_FLAC)
            .with_sample_rate(352_800)
            .with_channels(Channels::FRONT_LEFT | Channels::FRONT_RIGHT);
        let error = input_format(&params).unwrap_err();
        assert_eq!(error.code, "AB_SAMPLE_RATE");
        assert_eq!(error.codec, Some("FLAC"));
        assert_eq!(error.sample_rate, Some(352_800));
        assert_eq!(error.channels, Some(2));
        params
            .with_sample_rate(48_000)
            .with_channels(Channels::from_bits_truncate(0x3f));
        let error = input_format(&params).unwrap_err();
        assert_eq!(error.code, "AB_CHANNELS");
        assert_eq!(error.channels, Some(6));
        params.sample_rate = None;
        assert_eq!(input_format(&params).unwrap_err().code, "AB_FORMAT_INFO");
        assert_eq!(codec_label(CODEC_TYPE_PCM_S16LE), Some("PCM"));
        assert_eq!(codec_label(CODEC_TYPE_OPUS), Some("Opus"));
        assert_eq!(codec_label(CODEC_TYPE_NULL), None);
        let raw = DecodeError::Unsupported("private/path?token=private");
        let public = media_failure(&failure(raw), &params);
        let serialized = serde_json::to_string(&public).unwrap();
        assert!(serialized.contains("AB_CODEC"));
        assert!(!serialized.contains("private"));
        assert_eq!(DecodeFailure::from("private/path").code, "AB_UNAVAILABLE");
        assert_eq!(signature_codec(b"FRM812345678DSD "), Some("DSD (DSDIFF)"));
    }
    #[test]
    fn ab_decode_reports_rate_channels_and_unrecognized_input_without_guessing_a_codec() {
        let _serial = SERIAL.lock().unwrap();
        for (bytes, expected, codec, rate, channels) in [
            (
                wav(1, 384_000, 2),
                "AB_SAMPLE_RATE",
                Some("PCM"),
                Some(384_000),
                Some(2),
            ),
            (
                wav(1, 48_000, 6),
                "AB_CHANNELS",
                Some("PCM"),
                Some(48_000),
                Some(6),
            ),
            (
                [b"DSD ".as_slice(), &[0; 124]].concat(),
                "AB_CONTAINER",
                Some("DSD (DSF)"),
                None,
                None,
            ),
            (vec![0; 128], "AB_CONTAINER", None, None, None),
        ] {
            let job = Arc::new(Job {
                token: "format-fixture".into(),
                cancel: AtomicBool::new(false),
                slots: Mutex::new([Slot::default(), Slot::default()]),
            });
            job.install(
                Side::B,
                Input {
                    media: Box::new(Cursor::new(bytes)),
                    alive: Arc::new(|| true),
                    validate: Arc::new(|| true),
                    identity: "fixture".into(),
                },
            )
            .unwrap();
            let error = decode(job.clone(), Side::B, 0., 30).unwrap_err();
            assert_eq!(error.code, expected);
            assert_eq!(error.codec, codec);
            assert_eq!(error.sample_rate, rate);
            assert_eq!(error.channels, channels);
            assert!(!job.valid());
        }
    }
    #[test]
    fn limits_reject_invalid_offsets_and_durations() {
        for (start, len) in [(f64::NAN, 30), (-1., 30), (36_001., 30), (0., 29), (0., 91)] {
            assert!(parameters(start, len).is_err());
        }
        for len in [30, 60, 90] {
            assert!(parameters(0., len).is_ok());
        }
    }
    #[test]
    fn prepared_pcm_is_bounded_stereo_48k_with_a_complete_header() {
        let _serial = SERIAL.lock().unwrap();
        let input = wav(4, 44_100, 1);
        let job = Arc::new(Job {
            token: "fixture".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        job.install(
            Side::A,
            Input {
                media: Box::new(Cursor::new(input)),
                alive: Arc::new(|| true),
                validate: Arc::new(|| true),
                identity: "one".into(),
            },
        )
        .unwrap();
        let bytes = decode(job.clone(), Side::A, 0., 30).unwrap();
        assert_eq!(&bytes[..4], b"SAB1");
        let frames = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(frames, 4 * RATE);
        assert_eq!(bytes.len(), 40 + frames as usize * 8);
        assert!(!job.valid());
    }
    #[test]
    fn stale_and_same_identity_inputs_are_refused_before_decode() {
        let job = Arc::new(Job {
            token: "fixture".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        for side in [Side::A, Side::B] {
            let result = job.install(
                side,
                Input {
                    media: Box::new(Cursor::new(Vec::<u8>::new())),
                    alive: Arc::new(|| true),
                    validate: Arc::new(|| true),
                    identity: "one".into(),
                },
            );
            assert_eq!(result.is_ok(), matches!(side, Side::A));
        }
        job.cancel.store(true, Ordering::Release);
        assert_eq!(decode(job, Side::A, 0., 30).unwrap_err().code, "AB_CHANGED");
    }
    #[test]
    fn requested_offset_is_reflected_in_the_actual_shorter_sample() {
        let _serial = SERIAL.lock().unwrap();
        let job = Arc::new(Job {
            token: "seek-fixture".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        job.install(
            Side::B,
            Input {
                media: Box::new(Cursor::new(wav(5, 48_000, 2))),
                alive: Arc::new(|| true),
                validate: Arc::new(|| true),
                identity: "two".into(),
            },
        )
        .unwrap();
        let bytes = decode(job, Side::B, 1., 60).unwrap();
        assert_eq!(
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            4 * RATE
        );
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 1000);
        let rms = f32::from_le_bytes(bytes[32..36].try_into().unwrap());
        assert!((rms - 0.1767).abs() < 0.002);
    }

    #[test]
    fn server_part_paths_cannot_inject_a_url_query_token_or_traversal() {
        assert!(valid_part_path("/library/parts/12/456/file.flac"));
        for value in [
            "https://other/track.flac",
            "/library/metadata/12",
            "/library/parts/",
            "/library/parts/12/../outside",
            "/library/parts/12/%2e%2e/outside",
            "/library/parts/12/file?X-Plex-Token=secret",
            "/library/parts/12/file%3fsecret",
            "/library/parts/12/file%0d%0aheader",
        ] {
            assert!(!valid_part_path(value), "{value}");
        }
    }
}
