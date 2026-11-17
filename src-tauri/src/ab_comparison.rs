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
const DSD64_RATE: u32 = 2_822_400;
const DSF_PCM_RATE: u32 = 176_400;
const DSF_BLOCK_BYTES: usize = 4096;
const DSD_BYTES_PER_FRAME: usize = 4704;
const DSF_PCM_FRAMES_PER_DSD_FRAME: usize = 2352;
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
            | "AB_FORMAT_CHANGE" | "AB_RESAMPLE" | "AB_DSD_RATE" | "AB_DSD_FORMAT"
            | "AB_DSD_CLIPPING" => code,
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
    segment: Option<Segment>,
    ready: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Segment {
    pub start_seconds: f64,
    pub end_seconds: Option<f64>,
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
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub(crate) fn install(&self, side: Side, input: Input) -> Result<(), String> {
        self.install_segment(side, input, None)
    }
    pub(crate) fn install_segment(
        &self,
        side: Side,
        input: Input,
        segment: Option<Segment>,
    ) -> Result<(), String> {
        if !self.current() || !(input.validate)() || !(input.alive)() {
            return Err("AB_CHANGED".into());
        }
        if segment.is_some_and(|range| {
            !range.start_seconds.is_finite()
                || range.start_seconds < 0.
                || range
                    .end_seconds
                    .is_some_and(|end| !end.is_finite() || end <= range.start_seconds)
        }) {
            return Err("AB_SOURCE".into());
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
            segment,
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
fn requested_decode_seconds(seconds: u32, sync_context: bool, cue_remaining: Option<f64>) -> f64 {
    let requested = seconds as f64 + if sync_context { 3.0 } else { 0.0 };
    cue_remaining.map_or(requested, |remaining| remaining.min(requested))
}

#[derive(Clone, Copy, Debug)]
struct DsfHeader {
    sample_rate: u32,
    channels: usize,
    sample_count: u64,
    logical_bytes_per_channel: u64,
    block_groups: u64,
    data_offset: u64,
    frame_count: u64,
}

fn dsf_failure(code: &str, sample_rate: Option<u32>, channels: Option<u32>) -> DecodeFailure {
    DecodeFailure {
        code: DecodeFailure::from(code).code,
        codec: Some("DSD (DSF)"),
        sample_rate,
        channels,
    }
}

fn dsf_io_failure(
    error: &std::io::Error,
    check: &Check,
    rate: Option<u32>,
    channels: Option<u32>,
) -> DecodeFailure {
    let code = if !check() {
        "AB_CHANGED"
    } else if error.kind() == std::io::ErrorKind::Interrupted || error.to_string() == "AB_LIMIT" {
        "AB_LIMIT"
    } else if error.kind() == std::io::ErrorKind::UnexpectedEof {
        "AB_CONTAINER"
    } else {
        "AB_DECODE"
    };
    dsf_failure(code, rate, channels)
}

fn dsf_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn dsf_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
        bytes[at + 4],
        bytes[at + 5],
        bytes[at + 6],
        bytes[at + 7],
    ])
}

fn parse_dsf(source: &mut BudgetSource, check: &Check) -> Result<DsfHeader, DecodeFailure> {
    let container = || dsf_failure("AB_CONTAINER", None, None);
    source
        .seek(SeekFrom::Start(0))
        .map_err(|e| dsf_io_failure(&e, check, None, None))?;
    let mut header = [0u8; 92];
    source
        .read_exact(&mut header)
        .map_err(|e| dsf_io_failure(&e, check, None, None))?;
    if &header[0..4] != b"DSD "
        || dsf_u64(&header, 4) != 28
        || &header[28..32] != b"fmt "
        || dsf_u64(&header, 32) != 52
        || &header[80..84] != b"data"
    {
        return Err(container());
    }
    let file_bytes = dsf_u64(&header, 12);
    let metadata_offset = dsf_u64(&header, 20);
    if source.byte_len() != Some(file_bytes) {
        return Err(container());
    }
    let version = dsf_u32(&header, 40);
    let format_id = dsf_u32(&header, 44);
    let channel_type = dsf_u32(&header, 48);
    let channel_count = dsf_u32(&header, 52);
    let sample_rate = dsf_u32(&header, 56);
    let bits_per_sample = dsf_u32(&header, 60);
    let sample_count = dsf_u64(&header, 64);
    let block_size = dsf_u32(&header, 72);
    let reserved = dsf_u32(&header, 76);
    if sample_rate != DSD64_RATE {
        return Err(dsf_failure(
            "AB_DSD_RATE",
            Some(sample_rate),
            Some(channel_count),
        ));
    }
    // Sony DSF channel types 1 and 2 mean mono and stereo. Multichannel and
    // inconsistent declarations are not downmixed implicitly.
    if version != 1
        || format_id != 0
        || bits_per_sample != 1
        || reserved != 0
        || !matches!((channel_type, channel_count), (1, 1) | (2, 2))
        || block_size as usize != DSF_BLOCK_BYTES
    {
        return Err(dsf_failure(
            "AB_DSD_FORMAT",
            Some(sample_rate),
            Some(channel_count),
        ));
    }
    if sample_count == 0 {
        return Err(container());
    }
    let channels = channel_count as usize;
    let logical_bytes_per_channel = sample_count.checked_add(7).ok_or_else(container)? / 8;
    let block_groups = logical_bytes_per_channel
        .checked_add(DSF_BLOCK_BYTES as u64 - 1)
        .ok_or_else(container)?
        / DSF_BLOCK_BYTES as u64;
    let data_payload = block_groups
        .checked_mul(channels as u64)
        .and_then(|n| n.checked_mul(DSF_BLOCK_BYTES as u64))
        .ok_or_else(container)?;
    let expected_data_chunk = data_payload.checked_add(12).ok_or_else(container)?;
    let data_end = 80u64
        .checked_add(expected_data_chunk)
        .ok_or_else(container)?;
    let frame_count = sample_count
        .checked_add(DSD_BYTES_PER_FRAME as u64 * 8 - 1)
        .ok_or_else(container)?
        / (DSD_BYTES_PER_FRAME as u64 * 8);
    if dsf_u64(&header, 84) != expected_data_chunk
        || data_end > file_bytes
        || (metadata_offset == 0 && data_end != file_bytes)
        || (metadata_offset != 0 && (metadata_offset < data_end || metadata_offset >= file_bytes))
    {
        return Err(container());
    }
    Ok(DsfHeader {
        sample_rate,
        channels,
        sample_count,
        logical_bytes_per_channel,
        block_groups,
        data_offset: 92,
        frame_count,
    })
}

struct DsfBlockReader<'a> {
    source: &'a mut BudgetSource,
    header: DsfHeader,
    check: Check,
    cache: std::collections::VecDeque<(u64, Vec<u8>)>,
    channel_buffers: Vec<Vec<u8>>,
}

impl<'a> DsfBlockReader<'a> {
    fn new(source: &'a mut BudgetSource, header: DsfHeader, check: Check) -> Self {
        Self {
            source,
            header,
            check,
            cache: std::collections::VecDeque::with_capacity(2),
            channel_buffers: (0..header.channels)
                .map(|_| vec![0x69; DSD_BYTES_PER_FRAME])
                .collect(),
        }
    }

    fn group(&mut self, index: u64) -> Result<&[u8], DecodeFailure> {
        if let Some(position) = self.cache.iter().position(|(group, _)| *group == index) {
            let entry = self.cache.remove(position).ok_or_else(|| {
                dsf_failure(
                    "AB_CONTAINER",
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?;
            self.cache.push_back(entry);
            return Ok(&self
                .cache
                .back()
                .ok_or_else(|| {
                    dsf_failure(
                        "AB_CONTAINER",
                        Some(self.header.sample_rate),
                        Some(self.header.channels as u32),
                    )
                })?
                .1);
        }
        if index >= self.header.block_groups {
            return Err(dsf_failure(
                "AB_CONTAINER",
                Some(self.header.sample_rate),
                Some(self.header.channels as u32),
            ));
        }
        let group_bytes = self.header.channels * DSF_BLOCK_BYTES;
        let group_start = self
            .header
            .data_offset
            .checked_add(index.checked_mul(group_bytes as u64).ok_or_else(|| {
                dsf_failure(
                    "AB_CONTAINER",
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?)
            .ok_or_else(|| {
                dsf_failure(
                    "AB_CONTAINER",
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?;
        self.source
            .seek(SeekFrom::Start(group_start))
            .map_err(|e| {
                dsf_io_failure(
                    &e,
                    &self.check,
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?;
        let mut bytes = vec![0u8; group_bytes];
        self.source.read_exact(&mut bytes).map_err(|e| {
            dsf_io_failure(
                &e,
                &self.check,
                Some(self.header.sample_rate),
                Some(self.header.channels as u32),
            )
        })?;
        if self.cache.len() == 2 {
            self.cache.pop_front();
        }
        self.cache.push_back((index, bytes));
        Ok(&self
            .cache
            .back()
            .ok_or_else(|| {
                dsf_failure(
                    "AB_CONTAINER",
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?
            .1)
    }

    fn read_channel(
        &mut self,
        channel: usize,
        offset: u64,
        out: &mut [u8],
    ) -> Result<(), DecodeFailure> {
        let mut read = 0usize;
        while read < out.len() {
            let logical = offset.checked_add(read as u64).ok_or_else(|| {
                dsf_failure(
                    "AB_CONTAINER",
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?;
            let group_index = logical / DSF_BLOCK_BYTES as u64;
            let in_group = (logical % DSF_BLOCK_BYTES as u64) as usize;
            let group = self.group(group_index)?;
            let take = (DSF_BLOCK_BYTES - in_group).min(out.len() - read);
            let start = channel * DSF_BLOCK_BYTES + in_group;
            out[read..read + take].copy_from_slice(&group[start..start + take]);
            read += take;
        }
        Ok(())
    }

    fn frame(&mut self, index: u64, interleaved: &mut Vec<u8>) -> Result<bool, DecodeFailure> {
        if index >= self.header.frame_count {
            return Ok(false);
        }
        let byte_offset = index
            .checked_mul(DSD_BYTES_PER_FRAME as u64)
            .ok_or_else(|| {
                dsf_failure(
                    "AB_CONTAINER",
                    Some(self.header.sample_rate),
                    Some(self.header.channels as u32),
                )
            })?;
        let available = self
            .header
            .logical_bytes_per_channel
            .saturating_sub(byte_offset)
            .min(DSD_BYTES_PER_FRAME as u64) as usize;
        for channel in 0..self.header.channels {
            let mut channel_frame = [0x69; DSD_BYTES_PER_FRAME];
            if available > 0 {
                self.read_channel(channel, byte_offset, &mut channel_frame[..available])?;
                for byte in &mut channel_frame[..available] {
                    *byte = byte.reverse_bits();
                }
                let valid_bits = self
                    .header
                    .sample_count
                    .saturating_sub(byte_offset * 8)
                    .min((available * 8) as u64) as usize;
                if valid_bits % 8 != 0 {
                    let mask = 0xffu8 << (8 - valid_bits % 8);
                    let last = &mut channel_frame[available - 1];
                    *last = (*last & mask) | (0x69 & !mask);
                }
            }
            self.channel_buffers[channel].copy_from_slice(&channel_frame);
        }
        interleaved.resize(DSD_BYTES_PER_FRAME * self.header.channels, 0x69);
        for byte in 0..DSD_BYTES_PER_FRAME {
            for channel in 0..self.header.channels {
                interleaved[byte * self.header.channels + channel] =
                    self.channel_buffers[channel][byte];
            }
        }
        Ok(true)
    }
}

fn decode_dsf(
    job: Arc<Job>,
    side: Side,
    start: f64,
    seconds: u32,
    sync_context: bool,
    source: &mut BudgetSource,
    check: Check,
    validate: Check,
    segment: Option<Segment>,
) -> Result<Vec<u8>, DecodeFailure> {
    let header = parse_dsf(source, &check)?;
    let media_start = segment.map_or(start, |range| range.start_seconds + start);
    let file_duration = header.sample_count as f64 / header.sample_rate as f64;
    let file_remaining = file_duration - media_start;
    if !media_start.is_finite() || file_remaining <= 0.0 {
        return Err("AB_TOO_SHORT".into());
    }
    let cue_remaining = segment.and_then(|range| range.end_seconds.map(|end| end - media_start));
    let sample_seconds =
        requested_decode_seconds(seconds, sync_context, cue_remaining).min(file_remaining);
    if sample_seconds * (RATE as f64) < MIN_FRAMES as f64 {
        return Err("AB_TOO_SHORT".into());
    }
    if !(check)() || !(validate)() {
        return Err("AB_CHANGED".into());
    }
    let first_frame = (media_start * 75.0).floor() as u64;
    let crop = ((media_start - first_frame as f64 / 75.0) * DSF_PCM_RATE as f64).round() as usize;
    let output_frames = (sample_seconds * DSF_PCM_RATE as f64).floor() as usize;
    let frame_span = crop.checked_add(output_frames).ok_or("AB_LIMIT")?;
    let frames_to_convert = frame_span
        .checked_add(DSF_PCM_FRAMES_PER_DSD_FRAME - 1)
        .ok_or("AB_LIMIT")?
        / DSF_PCM_FRAMES_PER_DSD_FRAME;
    if frames_to_convert == 0
        || first_frame
            .checked_add(frames_to_convert as u64)
            .is_none_or(|end| end > header.frame_count)
    {
        return Err("AB_TOO_SHORT".into());
    }
    if output_frames > DSF_PCM_RATE as usize * 93 {
        return Err("AB_LIMIT".into());
    }

    let mut reader = DsfBlockReader::new(source, header, check.clone());
    let frame_bytes = DSD_BYTES_PER_FRAME * header.channels;
    let mut before = vec![0x69; frame_bytes];
    let mut current = vec![0x69; frame_bytes];
    let mut after = vec![0x69; frame_bytes];
    let mut has_before = first_frame > 0 && reader.frame(first_frame - 1, &mut before)?;
    if !reader.frame(first_frame, &mut current)? {
        return Err("AB_CONTAINER".into());
    }
    let mut has_after = reader.frame(first_frame + 1, &mut after)?;
    let mut converter = scarletbook_pcm::Converter::new(scarletbook_pcm::PcmConfig::new(
        scarletbook_pcm::PcmRate::Hz176400,
        scarletbook_pcm::Cutoff::Default,
        header.channels as u8,
    ))
    .map_err(|_| {
        dsf_failure(
            "AB_DSD_FORMAT",
            Some(header.sample_rate),
            Some(header.channels as u32),
        )
    })?;
    let mut converted = vec![0i32; converter.samples_per_frame() * header.channels];
    let mut pcm = Vec::<f32>::with_capacity(output_frames.saturating_mul(2));
    for frame_index in 0..frames_to_convert {
        if !job.current() || !(check)() {
            return Err("AB_CHANGED".into());
        }
        if frame_index % 75 == 0 && !(validate)() {
            return Err("AB_CHANGED".into());
        }
        let stats = converter
            .convert_frame(
                has_before.then_some(before.as_slice()),
                &current,
                has_after.then_some(after.as_slice()),
                &mut converted,
            )
            .map_err(|_| {
                dsf_failure(
                    "AB_DECODE",
                    Some(header.sample_rate),
                    Some(header.channels as u32),
                )
            })?;
        if stats.clipped_total() != 0 {
            return Err(dsf_failure(
                "AB_DSD_CLIPPING",
                Some(header.sample_rate),
                Some(header.channels as u32),
            ));
        }
        let source_base = frame_index * DSF_PCM_FRAMES_PER_DSD_FRAME;
        let take_start = crop
            .saturating_sub(source_base)
            .min(DSF_PCM_FRAMES_PER_DSD_FRAME);
        let take_end = (crop + output_frames)
            .saturating_sub(source_base)
            .min(DSF_PCM_FRAMES_PER_DSD_FRAME);
        for sample in take_start..take_end {
            let at = sample * header.channels;
            let left = converted[at] as f32 / 8_388_608.0;
            let right = converted[at + header.channels - 1] as f32 / 8_388_608.0;
            if !left.is_finite() || !right.is_finite() {
                return Err("AB_DECODE".into());
            }
            pcm.push(left);
            pcm.push(right);
        }
        if frame_index + 1 < frames_to_convert {
            std::mem::swap(&mut before, &mut current);
            std::mem::swap(&mut current, &mut after);
            has_before = true;
            has_after = reader.frame(first_frame + frame_index as u64 + 2, &mut after)?;
        }
        if pcm.len() / 2 > DSF_PCM_RATE as usize * 93 {
            return Err("AB_LIMIT".into());
        }
    }
    if pcm.len() / 2 != output_frames {
        return Err("AB_TOO_SHORT".into());
    }
    if !(check)() || !(validate)() {
        return Err("AB_CHANGED".into());
    }
    let pcm = resample(pcm, DSF_PCM_RATE).map_err(|_| {
        dsf_failure(
            "AB_RESAMPLE",
            Some(header.sample_rate),
            Some(header.channels as u32),
        )
    })?;
    if pcm.len() / 2 < MIN_FRAMES || pcm.len() / 2 > RATE as usize * 93 {
        return Err("AB_TOO_SHORT".into());
    }
    if !(check)() || !(validate)() {
        return Err("AB_CHANGED".into());
    }
    let encoded = encode(
        &pcm,
        header.sample_rate,
        header.channels as u32,
        start,
        true,
    )?;
    job.slots.lock().map_err(|_| "AB_UNAVAILABLE")?[side.index()].ready = true;
    Ok(encoded)
}
#[cfg(test)]
pub(crate) fn decode(
    job: Arc<Job>,
    side: Side,
    start: f64,
    seconds: u32,
) -> Result<Vec<u8>, DecodeFailure> {
    decode_with_context(job, side, start, seconds, false)
}
pub(crate) fn decode_with_context(
    job: Arc<Job>,
    side: Side,
    start: f64,
    seconds: u32,
    sync_context: bool,
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
    let (input, segment) = {
        let mut slots = job.slots.lock().map_err(|_| "AB_UNAVAILABLE")?;
        let slot = &mut slots[side.index()];
        (slot.input.take().ok_or("AB_CHANGED")?, slot.segment)
    };
    if !(input.validate)() || !(input.alive)() {
        return Err("AB_CHANGED".into());
    }
    let checked_job = job.clone();
    let policy = input.alive.clone();
    let check: Check = Arc::new(move || checked_job.current() && policy());
    let validate = input.validate;
    let detected = Arc::new(Mutex::new(None));
    let mut source = BudgetSource {
        inner: input.media,
        check: check.clone(),
        deadline: Instant::now() + DEADLINE,
        bytes: 0,
        detected: detected.clone(),
    };
    let mut signature = [0u8; 4];
    source
        .read_exact(&mut signature)
        .map_err(|_| DecodeFailure {
            codec: signature_codec(&signature),
            ..DecodeFailure::from(if !check() {
                "AB_CHANGED"
            } else {
                "AB_CONTAINER"
            })
        })?;
    if &signature == b"DSD " {
        return decode_dsf(
            job,
            side,
            start,
            seconds,
            sync_context,
            &mut source,
            check,
            validate,
            segment,
        );
    }
    source.seek(SeekFrom::Start(0)).map_err(|_| "AB_SEEK")?;
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
    let media_start = segment.map_or(start, |range| range.start_seconds + start);
    let cue_remaining = segment.and_then(|range| range.end_seconds.map(|end| end - media_start));
    let sample_seconds = requested_decode_seconds(seconds, sync_context, cue_remaining);
    let wanted_frames = (sample_seconds * rate as f64).floor() as usize;
    if wanted_frames < MIN_FRAMES {
        return Err("AB_TOO_SHORT".into());
    }
    if start > 0. || media_start > 0. {
        format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(media_start),
                    track_id: Some(id),
                },
            )
            .map_err(|_| "AB_SEEK")?;
        decoder.reset();
    }
    let wanted = wanted_frames;
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
        if end <= media_start {
            continue;
        }
        if pcm.is_empty() && at > media_start + tolerance {
            return Err("AB_SEEK".into());
        }
        if previous_end.is_some_and(|p| (at - p).abs() > tolerance) {
            return Err("AB_SEEK".into());
        }
        previous_end = Some(end);
        let skip = ((media_start - at).max(0.) * rate as f64).round() as usize;
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
    let encoded = encode(&pcm, rate, channels as u32, start, false)?;
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
fn encode(
    samples: &[f32],
    rate: u32,
    channels: u32,
    start: f64,
    converted_from_dsd: bool,
) -> Result<Vec<u8>, String> {
    if samples.len() % 2 != 0 || samples.len() > RATE as usize * 93 * 2 {
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
    bytes.extend_from_slice(&u32::from(converted_from_dsd).to_le_bytes());
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

    fn synthetic_dsf() -> Vec<u8> {
        static FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let sample_count = u64::from(DSD64_RATE) * 34 / 10;
                let logical_bytes = (sample_count as usize).div_ceil(8);
                let groups = logical_bytes.div_ceil(DSF_BLOCK_BYTES);
                let channels = 2usize;
                let mut channel_data = Vec::with_capacity(channels);
                for channel in 0..channels {
                    let frequency = if channel == 0 { 440.0 } else { 660.0 };
                    let amplitude = if channel == 0 { 0.30 } else { 0.22 };
                    let step = std::f64::consts::TAU * frequency / f64::from(DSD64_RATE);
                    let (sin_step, cos_step) = step.sin_cos();
                    let (mut sine, mut cosine) = (0.0f64, 1.0f64);
                    let mut integrator = 0.0f64;
                    let mut data = vec![0u8; logical_bytes];
                    for index in 0..sample_count as usize {
                        let target = amplitude * sine;
                        integrator += target;
                        let bit = integrator >= 0.0;
                        integrator -= if bit { 1.0 } else { -1.0 };
                        if bit {
                            data[index / 8] |= 1 << (index % 8);
                        }
                        (sine, cosine) = (
                            sine * cos_step + cosine * sin_step,
                            cosine * cos_step - sine * sin_step,
                        );
                    }
                    channel_data.push(data);
                }
                let payload_bytes = groups * channels * DSF_BLOCK_BYTES;
                let file_bytes = 92 + payload_bytes;
                let mut bytes = vec![0u8; 92];
                bytes[0..4].copy_from_slice(b"DSD ");
                bytes[4..12].copy_from_slice(&28u64.to_le_bytes());
                bytes[12..20].copy_from_slice(&(file_bytes as u64).to_le_bytes());
                bytes[20..28].copy_from_slice(&0u64.to_le_bytes());
                bytes[28..32].copy_from_slice(b"fmt ");
                bytes[32..40].copy_from_slice(&52u64.to_le_bytes());
                bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
                bytes[44..48].copy_from_slice(&0u32.to_le_bytes());
                bytes[48..52].copy_from_slice(&2u32.to_le_bytes());
                bytes[52..56].copy_from_slice(&2u32.to_le_bytes());
                bytes[56..60].copy_from_slice(&DSD64_RATE.to_le_bytes());
                bytes[60..64].copy_from_slice(&1u32.to_le_bytes());
                bytes[64..72].copy_from_slice(&sample_count.to_le_bytes());
                bytes[72..76].copy_from_slice(&(DSF_BLOCK_BYTES as u32).to_le_bytes());
                bytes[76..80].copy_from_slice(&0u32.to_le_bytes());
                bytes[80..84].copy_from_slice(b"data");
                bytes[84..92].copy_from_slice(&((payload_bytes + 12) as u64).to_le_bytes());
                for group in 0..groups {
                    for channel in channel_data.iter().take(channels) {
                        let start = group * DSF_BLOCK_BYTES;
                        let end = (start + DSF_BLOCK_BYTES).min(channel.len());
                        if start < end {
                            bytes.extend_from_slice(&channel[start..end]);
                        }
                        bytes.resize(bytes.len() + DSF_BLOCK_BYTES - end.saturating_sub(start), 0);
                    }
                }
                bytes
            })
            .clone()
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
    fn sync_decode_context_is_bounded_and_never_crosses_a_cue_end() {
        for seconds in [30, 60, 90] {
            assert_eq!(
                requested_decode_seconds(seconds, false, None),
                seconds as f64
            );
            assert_eq!(
                requested_decode_seconds(seconds, true, None),
                seconds as f64 + 3.0
            );
        }
        assert_eq!(requested_decode_seconds(30, true, Some(2.4)), 2.4);
        assert_eq!(requested_decode_seconds(90, true, Some(91.0)), 91.0);
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

    fn install_fixture(job: &Arc<Job>, side: Side, bytes: Vec<u8>, identity: &str) {
        job.install(
            side,
            Input {
                media: Box::new(Cursor::new(bytes)),
                alive: Arc::new(|| true),
                validate: Arc::new(|| true),
                identity: identity.to_owned(),
            },
        )
        .unwrap();
    }

    #[test]
    fn synthetic_dsf_is_actually_decoded_filtered_and_marked_for_the_ui() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = synthetic_dsf();
        let job = Arc::new(Job {
            token: "dsf-effective".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        install_fixture(&job, Side::A, bytes, "dsf-stereo");
        let output = decode(job.clone(), Side::A, 0.0, 30).unwrap();
        assert_eq!(&output[..4], b"SAB1");
        assert_eq!(
            u32::from_le_bytes(output[12..16].try_into().unwrap()),
            3 * RATE + RATE * 2 / 5
        );
        assert_eq!(
            u32::from_le_bytes(output[16..20].try_into().unwrap()),
            DSD64_RATE
        );
        assert_eq!(u32::from_le_bytes(output[20..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(output[36..40].try_into().unwrap()), 1);
        let peak = f32::from_le_bytes(output[28..32].try_into().unwrap());
        let rms = f32::from_le_bytes(output[32..36].try_into().unwrap());
        assert!(peak.is_finite() && peak > 0.1 && peak < 0.8, "peak={peak}");
        assert!(rms.is_finite() && rms > 0.05 && rms < 0.4, "rms={rms}");
        let left = f32::from_le_bytes(output[40..44].try_into().unwrap());
        let right = f32::from_le_bytes(output[44..48].try_into().unwrap());
        assert!(left.is_finite() && right.is_finite());
        assert_ne!(left, right);
    }

    #[test]
    fn dsf_rejects_truncated_invalid_rate_and_unsupported_channel_layouts() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let valid = synthetic_dsf();
        let mut invalid_rate = valid.clone();
        invalid_rate[56..60].copy_from_slice(&5_644_800u32.to_le_bytes());
        let mut invalid_channels = valid.clone();
        invalid_channels[48..52].copy_from_slice(&6u32.to_le_bytes());
        invalid_channels[52..56].copy_from_slice(&6u32.to_le_bytes());
        for (bytes, expected) in [
            (valid[..91].to_vec(), "AB_CONTAINER"),
            (valid[..valid.len() - 1].to_vec(), "AB_CONTAINER"),
            (invalid_rate, "AB_DSD_RATE"),
            (invalid_channels, "AB_DSD_FORMAT"),
        ] {
            let job = Arc::new(Job {
                token: "dsf-invalid".into(),
                cancel: AtomicBool::new(false),
                slots: Mutex::new([Slot::default(), Slot::default()]),
            });
            install_fixture(&job, Side::A, bytes, "invalid-dsf");
            let failure = decode(job, Side::A, 0.0, 30).unwrap_err();
            assert_eq!(failure.code, expected);
            assert_eq!(failure.codec, Some("DSD (DSF)"));
            assert!(!serde_json::to_string(&failure).unwrap().contains("private"));
        }
        let mut short = valid;
        let short_samples = u64::from(DSD64_RATE) * 25 / 10;
        let short_groups = (short_samples as usize)
            .div_ceil(8)
            .div_ceil(DSF_BLOCK_BYTES);
        let short_payload = short_groups * 2 * DSF_BLOCK_BYTES;
        short.truncate(92 + short_payload);
        let short_file_bytes = short.len() as u64;
        short[12..20].copy_from_slice(&short_file_bytes.to_le_bytes());
        short[64..72].copy_from_slice(&short_samples.to_le_bytes());
        short[84..92].copy_from_slice(&((short_payload + 12) as u64).to_le_bytes());
        let short_job = Arc::new(Job {
            token: "dsf-short".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        install_fixture(&short_job, Side::A, short, "short-dsf");
        assert_eq!(
            decode(short_job, Side::A, 0.0, 30).unwrap_err().code,
            "AB_TOO_SHORT"
        );
    }

    #[test]
    fn dsf_pcm_and_dsf_dsf_prepare_in_the_same_48k_pipeline() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dsf = synthetic_dsf();
        let job = Arc::new(Job {
            token: "dsf-pcm-pair".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        install_fixture(&job, Side::A, dsf.clone(), "dsf-a");
        install_fixture(&job, Side::B, wav(4, 48_000, 2), "pcm-b");
        let a = decode(job.clone(), Side::A, 0.0, 30).unwrap();
        let b = decode(job.clone(), Side::B, 0.0, 30).unwrap();
        assert_eq!(u32::from_le_bytes(a[36..40].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(b[36..40].try_into().unwrap()), 0);
        assert!(job.valid());

        let pair = Arc::new(Job {
            token: "dsf-pair".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        install_fixture(&pair, Side::A, dsf.clone(), "dsf-left");
        install_fixture(&pair, Side::B, dsf, "dsf-right");
        let left = decode(pair.clone(), Side::A, 0.0, 30).unwrap();
        let right = decode(pair.clone(), Side::B, 0.0, 30).unwrap();
        assert_eq!(&left[12..20], &right[12..20]);
        assert_eq!(u32::from_le_bytes(left[36..40].try_into().unwrap()), 1);
        assert!(pair.valid());
    }

    #[test]
    fn dsf_cue_start_manual_offset_and_duration_are_applied_once() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let job = Arc::new(Job {
            token: "dsf-cue".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        job.install_segment(
            Side::B,
            Input {
                media: Box::new(Cursor::new(synthetic_dsf())),
                alive: Arc::new(|| true),
                validate: Arc::new(|| true),
                identity: "dsf-cue".into(),
            },
            Some(Segment {
                start_seconds: 0.1,
                end_seconds: Some(3.6),
            }),
        )
        .unwrap();
        let output = decode(job, Side::B, 0.123, 30).unwrap();
        let media_start: f64 = 0.223;
        let expected_duration = (3.4f64 - media_start).min(3.6f64 - media_start);
        let expected_source_frames = (expected_duration * DSF_PCM_RATE as f64).floor() as usize;
        let expected_frames =
            (expected_source_frames * RATE as usize / DSF_PCM_RATE as usize) as u32;
        assert_eq!(
            u32::from_le_bytes(output[12..16].try_into().unwrap()),
            expected_frames
        );
        assert_eq!(u32::from_le_bytes(output[24..28].try_into().unwrap()), 123);
        assert_eq!(u32::from_le_bytes(output[36..40].try_into().unwrap()), 1);
    }

    #[test]
    fn dsf_decode_observes_cancellation_between_bounded_reads() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let calls = Arc::new(AtomicU64::new(0));
        let observed = calls.clone();
        let job = Arc::new(Job {
            token: "dsf-cancel".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        let alive: Check = Arc::new(move || observed.fetch_add(1, Ordering::AcqRel) < 12);
        job.install(
            Side::A,
            Input {
                media: Box::new(Cursor::new(synthetic_dsf())),
                alive,
                validate: Arc::new(|| true),
                identity: "dsf-cancel".into(),
            },
        )
        .unwrap();
        assert_eq!(
            decode(job, Side::A, 0.0, 30).unwrap_err().code,
            "AB_CHANGED"
        );
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
    fn cue_segment_decode_seeks_to_its_start_and_never_reads_past_its_end() {
        let _serial = SERIAL.lock().unwrap();
        let job = Arc::new(Job {
            token: "cue-range-fixture".into(),
            cancel: AtomicBool::new(false),
            slots: Mutex::new([Slot::default(), Slot::default()]),
        });
        job.install_segment(
            Side::A,
            Input {
                media: Box::new(Cursor::new(wav(8, RATE, 2))),
                alive: Arc::new(|| true),
                validate: Arc::new(|| true),
                identity: "sqlite-cue-fixture".into(),
            },
            Some(Segment {
                start_seconds: 2.0,
                end_seconds: Some(6.0),
            }),
        )
        .unwrap();

        // The UI asks for 30 seconds, but only the four seconds in this CUE
        // segment are eligible for comparison.
        let bytes = decode(job, Side::A, 0.0, 30).unwrap();
        assert_eq!(
            u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
            4 * RATE
        );
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 0);
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
