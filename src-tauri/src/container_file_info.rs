//! Structural limits before Lofty 0.25.4 property readers. No tags or covers.
//! Unsupported variants are refused before their allocation-sensitive paths.
use super::{InfoError, LocalFileInfo};
use lofty::{
    config::{ParseOptions, ParsingMode},
    file::AudioFile,
};
use std::io::{self, Read, Seek, SeekFrom};
#[path = "mp4_file_info.rs"]
mod mp4;
#[path = "packet_file_info.rs"]
mod packet;

fn bad<T>() -> Result<T, InfoError> {
    Err(InfoError::CorruptHeader)
}
fn unsupported<T>() -> Result<T, InfoError> {
    Err(InfoError::UnsupportedFormat)
}
fn le16(b: &[u8]) -> u16 {
    u16::from_le_bytes(b[..2].try_into().unwrap())
}
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}
fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes(b[..2].try_into().unwrap())
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes(b[..4].try_into().unwrap())
}
fn options() -> ParseOptions {
    ParseOptions::new()
        .read_tags(false)
        .read_cover_art(false)
        .read_properties(true)
        .parsing_mode(ParsingMode::Strict)
}
fn info(
    format: &'static str,
    codec: &'static str,
    compression: &'static str,
    rate: u32,
    channels: u8,
    depth: Option<u8>,
) -> LocalFileInfo {
    LocalFileInfo {
        provenance: "original_file_container_headers",
        format,
        codec,
        compression,
        sample_type: if compression == "lossy" {
            "not_applicable"
        } else {
            "integer"
        },
        sample_rate_basis: "encoded_header",
        original_sample_rate: None,
        lossy: compression == "lossy" || compression == "hybrid",
        frame_bitrate_kbps: None,
        dsd_level: None,
        sample_rate: Some(rate),
        bit_depth: depth,
        channels,
        duration_seconds: None,
        revision: String::new(),
    }
}
fn valid(rate: u32, channels: u16) -> Result<u8, InfoError> {
    if rate == 0 || channels == 0 || channels > 32 {
        return bad();
    }
    Ok(channels as u8)
}

/// Every physical Read/Seek, including Interrupted, is counted. Limits do not
/// provide a syscall timeout. Parent bounds are narrowed before library parsing.
struct Budget<'a, R> {
    inner: &'a mut R,
    len: u64,
    pos: u64,
    bytes: usize,
    reads: usize,
    seeks: usize,
    max_bytes: usize,
    max_reads: usize,
    max_seeks: usize,
    failure: Option<InfoError>,
    cache: Vec<(u64, Vec<u8>)>,
    frozen: bool,
    capture: bool,
}
impl<'a, R: Read + Seek> Budget<'a, R> {
    fn new(inner: &'a mut R, len: u64, bytes: usize, reads: usize, seeks: usize) -> Self {
        Self {
            inner,
            len,
            pos: 4,
            bytes: 4,
            reads: 64,
            seeks: 0,
            max_bytes: bytes,
            max_reads: reads,
            max_seeks: seeks,
            failure: None,
            cache: Vec::new(),
            frozen: false,
            capture: true,
        }
    }
    fn error(&mut self, error: InfoError) -> io::Error {
        self.failure = Some(error);
        io::Error::other("bounded file inspection")
    }
    fn map(&self, _: impl std::fmt::Debug) -> InfoError {
        self.failure.unwrap_or(InfoError::CorruptHeader)
    }
    fn at<const N: usize>(&mut self, pos: u64) -> Result<[u8; N], InfoError> {
        if pos
            .checked_add(N as u64)
            .filter(|end| *end <= self.len)
            .is_none()
        {
            return bad();
        }
        let mut b = [0; N];
        self.seek(SeekFrom::Start(pos)).map_err(|e| self.map(e))?;
        self.read_exact(&mut b).map_err(|e| self.map(e))?;
        Ok(b)
    }
    fn blob(&mut self, pos: u64, size: u64) -> Result<Vec<u8>, InfoError> {
        // Only structural metadata. This cap is checked BEFORE allocation.
        if size > 4096 {
            return unsupported();
        }
        if pos
            .checked_add(size)
            .filter(|end| *end <= self.len)
            .is_none()
        {
            return bad();
        }
        self.seek(SeekFrom::Start(pos)).map_err(|e| self.map(e))?;
        let mut b = vec![0; size as usize];
        self.read_exact(&mut b).map_err(|e| self.map(e))?;
        Ok(b)
    }
    fn freeze(&mut self) {
        self.frozen = true;
        self.pos = 0;
    }
    fn rewind(&mut self) -> Result<(), InfoError> {
        self.seek(SeekFrom::Start(0))
            .map(|_| ())
            .map_err(|e| self.map(e))
    }
}
impl<R: Read + Seek> Read for Budget<'_, R> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if b.is_empty() {
            return Ok(0);
        }
        if self.reads >= self.max_reads || self.bytes >= self.max_bytes {
            return Err(self.error(InfoError::UnsupportedFormat));
        }
        if self.pos == self.len {
            return Ok(0);
        }
        let n = b
            .len()
            .min(self.max_bytes - self.bytes)
            .min((self.len - self.pos).min(usize::MAX as u64) as usize);
        self.reads += 1;
        if self.frozen {
            let found = self
                .cache
                .iter()
                .find(|(start, data)| self.pos >= *start && self.pos - *start < data.len() as u64);
            if let Some((start, data)) = found {
                let offset = (self.pos - *start) as usize;
                let n = n.min(data.len() - offset);
                b[..n].copy_from_slice(&data[offset..offset + n]);
                self.pos += n as u64;
                self.bytes += n;
                return Ok(n);
            }
            return Err(self.error(InfoError::UnsupportedFormat));
        }
        if self.capture && self.cache.len() >= 2048 {
            return Err(self.error(InfoError::UnsupportedFormat));
        }
        let start = self.pos;
        match self.inner.read(&mut b[..n]) {
            Ok(0) => Err(self.error(InfoError::CorruptHeader)),
            Ok(n) => {
                if self.capture {
                    self.cache.push((start, b[..n].to_vec()));
                }
                self.pos += n as u64;
                self.bytes += n;
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(e),
            Err(e) => {
                self.failure = Some(if e.kind() == io::ErrorKind::PermissionDenied {
                    InfoError::PermissionDenied
                } else {
                    InfoError::Unavailable
                });
                Err(e)
            }
        }
    }
}
impl<R: Read + Seek> Seek for Budget<'_, R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let n = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.pos) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.len) + i128::from(n),
        };
        if n < 0 || n > i128::from(self.len) {
            return Err(self.error(InfoError::CorruptHeader));
        }
        let n = n as u64;
        if n == self.pos {
            return Ok(n);
        }
        if self.seeks >= self.max_seeks {
            return Err(self.error(InfoError::UnsupportedFormat));
        }
        self.seeks += 1;
        if self.frozen {
            self.pos = n;
            return Ok(n);
        }
        match self.inner.seek(SeekFrom::Start(n)) {
            Ok(actual) if actual == n => {
                self.pos = n;
                Ok(n)
            }
            _ => Err(self.error(InfoError::Unavailable)),
        }
    }
}

pub(super) fn read(
    reader: &mut (impl Read + Seek),
    len: u64,
    marker: [u8; 4],
) -> Result<LocalFileInfo, InfoError> {
    let mut r = Budget::new(reader, len, 64 * 1024, 8192, 2048);
    if !matches!(&marker, b"RIFF" | b"FORM" | b"OggS" | b"wvpk" | b"MPCK")
        && marker[..3] != *b"MP+"
        && !(marker[0] == 0xff && marker[1] & 0xf6 == 0xf0)
    {
        r.bytes = 8;
        r.reads = 128;
        r.seeks = 1;
    }
    match &marker {
        b"RIFF" | b"FORM" => {
            r.max_bytes = 16 * 1024;
            r.max_reads = 4096;
            iff(&mut r, marker == *b"RIFF")
        }
        b"OggS" => {
            r.capture = false;
            r.max_bytes = 256 * 1024;
            r.max_reads = 16384;
            r.max_seeks = 4096;
            packet::ogg(&mut r)
        }
        b"wvpk" => wavpack(&mut r),
        b"MPCK" => {
            r.capture = false;
            packet::mpc8(&mut r)
        }
        [b'M', b'P', b'+', _] => mpc7(&mut r),
        _ if marker[0] == 0xff && marker[1] & 0xf6 == 0xf0 => {
            r.capture = false;
            packet::adts(&mut r)
        }
        _ => {
            let next = if len >= 8 { r.at::<4>(4)? } else { [0; 4] };
            if next == *b"ftyp" {
                mp4::read(&mut r)
            } else {
                unsupported()
            }
        }
    }
}

fn iff<R: Read + Seek>(r: &mut Budget<'_, R>, wav: bool) -> Result<LocalFileInfo, InfoError> {
    let h = r.at::<12>(0)?;
    let kind = &h[8..12];
    if wav && kind != b"WAVE" || !wav && kind != b"AIFF" && kind != b"AIFC" {
        return unsupported();
    }
    let end = u64::from(if wav { le32(&h[4..]) } else { be32(&h[4..]) }) + 8;
    if end < 12 || end > r.len {
        return bad();
    }
    r.len = end;
    let (mut pos, mut property, mut audio, mut count) = (12, None, None, 0);
    while pos < end {
        count += 1;
        if count > 256 {
            return unsupported();
        }
        if end - pos < 8 {
            return bad();
        }
        let c = r.at::<8>(pos)?;
        let size = u64::from(if wav { le32(&c[4..]) } else { be32(&c[4..]) });
        let body = pos + 8;
        let next = body
            .checked_add(size)
            .and_then(|n| n.checked_add(size & 1))
            .ok_or(InfoError::CorruptHeader)?;
        if next > end {
            return bad();
        }
        match &c[..4] {
            b"fmt " if wav => {
                if property.is_some() || size < 16 {
                    return bad();
                }
                if size > 512 {
                    return unsupported();
                }
                property = Some(r.blob(body, size)?);
            }
            b"COMM" if !wav => {
                if property.is_some() || size < 18 {
                    return bad();
                }
                if size > 512 {
                    return unsupported();
                }
                property = Some(r.blob(body, size)?);
            }
            b"data" if wav => {
                if audio.replace(size).is_some() {
                    return unsupported();
                }
            }
            b"SSND" if !wav => {
                if size < 8 || audio.is_some() {
                    return bad();
                }
                let s = r.at::<8>(body)?;
                let offset = u64::from(be32(&s));
                if offset > size - 8 {
                    return bad();
                }
                audio = Some(size - 8 - offset);
            }
            // LIST is inspected by Lofty even with read_tags(false); constrain it.
            b"LIST" | b"fact" if wav => {
                if size < 4 {
                    return bad();
                }
                r.at::<4>(body)?;
            }
            _ => {}
        }
        pos = next;
    }
    let p = property.ok_or(InfoError::CorruptHeader)?;
    let data = audio.ok_or(InfoError::CorruptHeader)?;
    if wav {
        let mut code = le16(&p);
        let ch = le16(&p[2..]);
        let rate = le32(&p[4..]);
        let align = le16(&p[12..]);
        let storage = le16(&p[14..]);
        let channels = valid(rate, ch)?;
        let mut depth = storage;
        if code == 0xfffe {
            if p.len() < 40 || le16(&p[16..]) < 22 || usize::from(le16(&p[16..])) + 18 > p.len() {
                return bad();
            }
            if p[26..40] != [0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71] {
                return unsupported();
            }
            code = le16(&p[24..]);
            depth = le16(&p[18..]);
            if depth == 0 {
                depth = storage;
            }
        }
        if code != 1 && code != 3 {
            return unsupported();
        }
        if !(1..=64).contains(&storage)
            || storage % 8 != 0
            || depth == 0
            || depth > storage
            || code == 3 && (![32, 64].contains(&depth) || depth != storage)
        {
            return bad();
        }
        if align != ch * (storage / 8)
            || le32(&p[8..])
                != rate
                    .checked_mul(u32::from(align))
                    .ok_or(InfoError::CorruptHeader)?
            || data % u64::from(align) != 0
        {
            return bad();
        }
        r.freeze();
        r.rewind()?;
        let file = lofty::iff::wav::WavFile::read_from(r, options()).map_err(|e| r.map(e))?;
        let props = file.properties();
        if props.sample_rate() != rate
            || props.channels() != channels
            || props.bit_depth() != depth as u8
        {
            return bad();
        }
        let mut out = info(
            "WAV",
            if code == 3 { "PCM float" } else { "PCM" },
            "uncompressed",
            rate,
            channels,
            Some(depth as u8),
        );
        out.sample_type = if code == 3 { "float" } else { "integer" };
        out.duration_seconds = Some(data as f64 / f64::from(align) / f64::from(rate));
        Ok(out)
    } else {
        let ch = be16(&p);
        let depth = be16(&p[6..]);
        let frames = be32(&p[2..]);
        if ch == 0 || ch > 32 || depth == 0 || depth > 32 {
            return bad();
        }
        let code = if kind == b"AIFC" {
            if p.len() < 23 {
                return bad();
            }
            if 23 + usize::from(p[22]) > p.len() {
                return bad();
            }
            &p[18..22]
        } else {
            b"NONE"
        };
        if code != b"NONE" && code != b"sowt" {
            return unsupported();
        }
        if u64::from(frames) * u64::from(ch) * u64::from(depth.div_ceil(8)) > data {
            return bad();
        }
        r.freeze();
        r.rewind()?;
        let file = lofty::iff::aiff::AiffFile::read_from(r, options()).map_err(|e| r.map(e))?;
        let props = file.properties();
        let rate = props.sample_rate();
        let channels = valid(rate, ch)?;
        // Restrict the 80-bit field to an exact positive integral rate. Lofty's
        // floating conversion alone could truncate a fractional declaration.
        let exp = be16(&p[8..]);
        let mant = u64::from_be_bytes(p[10..18].try_into().unwrap());
        if exp < 16383 || exp > 16414 || mant >> 63 != 1 {
            return bad();
        }
        let shift = 63 - (exp - 16383);
        let integer = mant >> shift;
        if mant & ((1u64 << shift) - 1) != 0 || integer != u64::from(rate) {
            return bad();
        }
        let mut out = info(
            if kind == b"AIFC" { "AIFF-C" } else { "AIFF" },
            "PCM",
            "uncompressed",
            rate,
            channels,
            Some(depth as u8),
        );
        out.duration_seconds = Some(f64::from(frames) / f64::from(rate));
        Ok(out)
    }
}

fn wavpack<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<LocalFileInfo, InfoError> {
    let h = r.at::<32>(0)?;
    let size = le32(&h[4..]);
    let version = le16(&h[8..]);
    let flags = le32(&h[24..]);
    if size < 24 || u64::from(size) + 8 > r.len {
        return bad();
    }
    if size > 1_048_576
        || !(0x402..=0x410).contains(&version)
        || flags & 0x1800 != 0x1800
        || flags & 0x8000_0000 != 0
        || flags >> 23 & 15 == 15
    {
        return unsupported();
    }
    if h[10] != 0 || h[11] != 0 || le32(&h[16..]) != 0 || le32(&h[20..]) == 0 {
        return unsupported();
    }
    let rate = [
        6000, 8000, 9600, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200,
        96000, 192000,
    ][(flags >> 23 & 15) as usize];
    let hybrid = flags & 8 != 0;
    let float = flags & 0x80 != 0;
    if float && flags & 3 != 3 {
        return bad();
    }
    // One initial/final mono/stereo block; no multichannel or custom-rate path
    // that would make Lofty allocate the compressed block.
    tail_probes(r)?;
    r.freeze();
    r.rewind()?;
    let file = lofty::wavpack::WavPackFile::read_from(r, options()).map_err(|e| r.map(e))?;
    let p = file.properties();
    let ch = valid(rate, p.channels())?;
    let depth = if float {
        Some(32)
    } else if flags & 0x100 != 0 {
        None
    } else {
        Some(p.bit_depth())
    };
    if depth == Some(0) {
        return bad();
    }
    let mut out = info(
        "WavPack",
        "WavPack",
        if hybrid { "hybrid" } else { "lossless" },
        rate,
        ch,
        depth,
    );
    out.sample_type = if float { "float" } else { "integer" };
    let samples = le32(&h[12..]);
    if samples != u32::MAX && samples != 0 {
        out.duration_seconds = Some(f64::from(samples) / f64::from(rate));
    }
    Ok(out)
}
fn mpc7<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<LocalFileInfo, InfoError> {
    let h = r.at::<28>(0)?;
    if h[3] & 15 != 7 {
        return unsupported();
    }
    if le32(&h[4..]) == 0 {
        return bad();
    }
    let frames = u64::from(le32(&h[4..]));
    let end_flags = le32(&h[20..]);
    let last = (end_flags >> 20) & 0x7ff;
    if last > 1152 {
        return bad();
    }
    let samples = if end_flags >> 31 != 0 {
        frames * 1152 - (1152 - u64::from(last))
    } else {
        frames * 1152 - 481
    };
    if samples == 0 {
        return bad();
    }
    if r.len
        .checked_mul(8)
        .and_then(|n| n.checked_mul(48000))
        .is_none()
    {
        return unsupported();
    }
    tail_probes(r)?;
    r.freeze();
    r.rewind()?;
    let file = lofty::musepack::MpcFile::read_from(r, options()).map_err(|e| r.map(e))?;
    let p: lofty::properties::FileProperties = file.properties().clone().into();
    let rate = p.sample_rate().ok_or(InfoError::CorruptHeader)?;
    let ch = valid(
        rate,
        u16::from(p.channels().ok_or(InfoError::CorruptHeader)?),
    )?;
    let mut out = info("Musepack SV7", "Musepack", "lossy", rate, ch, None);
    out.duration_seconds = Some(samples as f64 / f64::from(rate));
    Ok(out)
}

#[cfg(test)]
#[path = "container_file_info_tests.rs"]
mod tests;

// Snapshot only the tail signatures/APE footer fields that the public readers
// inspect with tags disabled. Never freeze tag bodies or compressed blocks.
fn tail_probes<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<(), InfoError> {
    let mut end = r.len;
    if end >= 128 && r.at::<3>(end - 128)? == *b"TAG" {
        end -= 128;
    }
    if end < 32 {
        return bad();
    }
    let lyrics = r.at::<15>(end - 15)?;
    if lyrics.ends_with(b"LYRICS200") {
        return unsupported();
    }
    let ape = r.at::<8>(end - 32)?;
    if ape == *b"APETAGEX" {
        let h = r.at::<20>(end - 32)?;
        let size = u64::from(le32(&h[12..]));
        if size < 32 || size > end {
            return bad();
        }
    }
    Ok(())
}
