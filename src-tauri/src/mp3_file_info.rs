//! MPEG Layer III header inspection, not decoding or whole-stream validation.
//! Tables/frame formula cross-checked with Lofty 0.25.4 mpeg/{constants,header}.rs
//! (upstream 618731cee7aac784f5f08643bd5d8d005f3c01d6). Public parser does more I/O
//! than this inspection needs; its private header reader cannot be called directly.
//! ID3.org v2.2/v2.3 §3.1, v2.4 §§3.1/3.4: skip opaque tags, verify footer.
use super::{InfoError, LocalFileInfo};
use std::io::{self, Read, Seek, SeekFrom};

const MAX_BYTES: usize = 8192;
const MAX_READS: usize = 8192;
const MAX_SEEKS: usize = 256;
const MAX_JUNK: usize = 4096;
const MAX_TAGS: usize = 4;
const MAX_CANDIDATES: usize = 32;
const CONFIRM_FRAMES: usize = 3;

struct Bounded<'a, R> {
    reader: &'a mut R,
    len: u64,
    pos: u64,
    bytes: usize,
    reads: usize,
    seeks: usize,
}
fn io_error(e: io::Error) -> InfoError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => InfoError::CorruptHeader,
        io::ErrorKind::PermissionDenied => InfoError::PermissionDenied,
        _ => InfoError::Unavailable,
    }
}
impl<R: Read + Seek> Bounded<'_, R> {
    fn at<const N: usize>(&mut self, offset: u64) -> Result<[u8; N], InfoError> {
        if offset
            .checked_add(N as u64)
            .filter(|end| *end <= self.len)
            .is_none()
        {
            return Err(InfoError::CorruptHeader);
        }
        if self.bytes + N > MAX_BYTES {
            return Err(InfoError::UnsupportedFormat);
        }
        if self.pos != offset {
            if self.seeks >= MAX_SEEKS {
                return Err(InfoError::UnsupportedFormat);
            }
            self.seeks += 1;
            if self
                .reader
                .seek(SeekFrom::Start(offset))
                .map_err(io_error)?
                != offset
            {
                return Err(InfoError::Unavailable);
            }
            self.pos = offset;
        }
        let mut data = [0; N];
        let mut filled = 0;
        while filled < N {
            if self.reads >= MAX_READS {
                return Err(InfoError::UnsupportedFormat);
            }
            self.reads += 1;
            match self.reader.read(&mut data[filled..]) {
                Ok(0) => return Err(InfoError::CorruptHeader),
                Ok(n) => {
                    filled += n;
                    self.bytes += n;
                    self.pos += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        Ok(data)
    }
}
#[derive(Debug, Clone, Copy)]
struct Frame {
    version: u8,
    rate: u32,
    channels: u8,
    kbps: u16,
    len: u32,
}
impl Frame {
    fn parse(b: [u8; 4]) -> Option<Self> {
        let n = u32::from_be_bytes(b);
        if n >> 21 != 0x7ff || (n >> 17) & 3 != 1 || n & 3 == 2 {
            return None;
        }
        let version = ((n >> 19) & 3) as u8;
        let divisor = match version {
            3 => 1,
            2 => 2,
            0 => 4,
            _ => return None,
        };
        let rate = [44100, 48000, 32000]
            .get(((n >> 10) & 3) as usize)
            .copied()?
            / divisor;
        let index = ((n >> 12) & 15) as usize;
        // Index0 is free-format (not supported);15 and sample-rate3 are reserved.
        let kbps = if version == 3 {
            [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
            ][index]
        } else {
            [
                0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
            ][index]
        };
        if kbps == 0 {
            return None;
        }
        let channels = if (n >> 6) & 3 == 3 { 1 } else { 2 };
        let len =
            (if version == 3 { 144_000 } else { 72_000 }) * kbps as u32 / rate + ((n >> 9) & 1);
        let side = if version == 3 {
            if channels == 1 {
                17
            } else {
                32
            }
        } else if channels == 1 {
            9
        } else {
            17
        };
        let crc = if (n >> 16) & 1 == 0 { 2 } else { 0 };
        if len < 4 + crc + side {
            return None;
        }
        Some(Self {
            version,
            rate,
            channels,
            kbps,
            len,
        })
    }
    fn compatible(self, other: Self) -> bool {
        self.version == other.version && self.rate == other.rate && self.channels == other.channels
    }
}
fn confirm<R: Read + Seek>(
    h: &mut Bounded<R>,
    start: u64,
    first: Frame,
) -> Result<bool, InfoError> {
    let mut offset = start;
    let mut frame = first;
    for i in 0..CONFIRM_FRAMES {
        if i > 0 {
            if offset.checked_add(4).filter(|end| *end <= h.len).is_none() {
                return Ok(false);
            }
            let Some(next) = Frame::parse(h.at(offset)?) else {
                return Ok(false);
            };
            if !first.compatible(next) {
                return Ok(false);
            }
            frame = next;
        }
        let Some(end) = offset
            .checked_add(frame.len as u64)
            .filter(|end| *end <= h.len)
        else {
            return Ok(false);
        };
        offset = end; // payload/CRC/side information are never decoded or loaded
    }
    Ok(true)
}
fn skip_id3<R: Read + Seek>(
    h: &mut Bounded<R>,
    offset: u64,
    marker: [u8; 4],
) -> Result<u64, InfoError> {
    let rest = h.at::<6>(offset.checked_add(4).ok_or(InfoError::CorruptHeader)?)?;
    let (version, revision, flags) = (marker[3], rest[0], rest[1]);
    let allowed = match version {
        2 => 0xc0,
        3 => 0xe0,
        4 => 0xf0,
        _ => return Err(InfoError::UnsupportedFormat),
    };
    if revision == 255 || flags & !allowed != 0 || rest[2..].iter().any(|v| *v >= 128) {
        return Err(InfoError::CorruptHeader);
    }
    let size = rest[2..].iter().fold(0u64, |n, b| (n << 7) | *b as u64);
    if version >= 3 && flags & 0x40 != 0 && size < if version == 4 { 6 } else { 10 } {
        return Err(InfoError::CorruptHeader);
    }
    let body_end = offset
        .checked_add(10)
        .and_then(|p| p.checked_add(size))
        .filter(|p| *p <= h.len)
        .ok_or(InfoError::CorruptHeader)?;
    if version == 4 && flags & 0x10 != 0 {
        let footer = h.at::<10>(body_end)?;
        if &footer[..3] != b"3DI" || footer[3] != version || footer[4..] != rest {
            return Err(InfoError::CorruptHeader);
        }
        body_end
            .checked_add(10)
            .filter(|end| *end <= h.len)
            .ok_or(InfoError::CorruptHeader)
    } else {
        Ok(body_end)
    }
}
fn other_container(marker: &[u8; 4]) -> bool {
    matches!(
        marker,
        b"fLaC"
            | b"DSD "
            | b"FRM8"
            | b"RIFF"
            | b"FORM"
            | b"OggS"
            | b"MAC "
            | b"wvpk"
            | b"APET"
            | b"ftyp"
    )
}
/// The first four signature bytes were read by the dispatcher. All budgets
/// include them, including a conservative64 calls for a partial/Interrupted read.
/// No result cache, heap buffer, tag payload, frame decoding or EOF search.
/// The search may inspect up to MAX_JUNK bytes before a confirmed frame chain.
pub(super) fn read_mp3(
    reader: &mut (impl Read + Seek),
    len: u64,
    marker: [u8; 4],
) -> Result<LocalFileInfo, InfoError> {
    read_with_probe_budget(reader, len, marker, false)
}
pub(super) fn read_mp3_probed(
    reader: &mut (impl Read + Seek),
    len: u64,
    marker: [u8; 4],
) -> Result<LocalFileInfo, InfoError> {
    read_with_probe_budget(reader, len, marker, true)
}
fn read_with_probe_budget(
    reader: &mut (impl Read + Seek),
    len: u64,
    marker: [u8; 4],
    probed: bool,
) -> Result<LocalFileInfo, InfoError> {
    let mut h = Bounded {
        reader,
        len,
        pos: 4,
        bytes: if probed { 8 } else { 4 },
        reads: if probed { 128 } else { 64 },
        seeks: usize::from(probed),
    };
    let (mut offset, mut window) = (0u64, marker);
    let (mut junk, mut tags, mut candidates) = (0, 0, 0);
    loop {
        if other_container(&window) {
            return Err(InfoError::UnsupportedFormat);
        }
        if &window[..3] == b"ID3" {
            if tags == MAX_TAGS {
                return Err(InfoError::UnsupportedFormat);
            }
            tags += 1;
            offset = skip_id3(&mut h, offset, window)?;
            if offset == len {
                return Err(InfoError::UnsupportedFormat);
            }
            window = h.at(offset)?;
            continue;
        }
        if let Some(frame) = Frame::parse(window) {
            if candidates == MAX_CANDIDATES {
                return Err(InfoError::UnsupportedFormat);
            }
            candidates += 1;
            if confirm(&mut h, offset, frame)? {
                return Ok(LocalFileInfo {
                    provenance: "original_file_mpeg_headers",
                    format: "MPEG Audio",
                    codec: "MP3",
                    lossy: true,
                    compression: "lossy",
                    sample_type: "not_applicable",
                    sample_rate_basis: "encoded_header",
                    original_sample_rate: None,
                    frame_bitrate_kbps: Some(frame.kbps),
                    dsd_level: None,
                    sample_rate: Some(frame.rate),
                    bit_depth: None,
                    channels: frame.channels,
                    duration_seconds: None,
                    revision: String::new(),
                });
            }
        }
        if junk == MAX_JUNK || offset.checked_add(5).filter(|p| *p <= len).is_none() {
            return Err(InfoError::UnsupportedFormat);
        }
        junk += 1;
        offset += 1;
        window.rotate_left(1);
        window[3] = h.at::<1>(offset + 3)?[0];
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::local_file_info::read_file_info;
    use std::io::Cursor;
    fn header(version: u8, index: u8, rate: u8, padding: bool, crc: bool, mode: u8) -> [u8; 4] {
        (0xffe00000u32
            | (version as u32) << 19
            | 1 << 17
            | (u32::from(!crc)) << 16
            | (index as u32) << 12
            | (rate as u32) << 10
            | (padding as u32) << 9
            | (mode as u32) << 6)
            .to_be_bytes()
    }
    fn append(b: &mut Vec<u8>, header: [u8; 4], len: usize) {
        b.extend(header);
        b.resize(b.len() + len - 4, 0x55);
    }
    pub(crate) fn fixture() -> Vec<u8> {
        let mut b = Vec::new();
        for _ in 0..3 {
            append(&mut b, header(3, 9, 0, false, false, 0), 417);
        }
        b
    }
    fn inspect(b: &[u8]) -> Result<LocalFileInfo, InfoError> {
        read_file_info(&mut Cursor::new(b), b.len() as u64)
    }
    fn synchsafe(n: u32) -> [u8; 4] {
        [
            (n >> 21) as u8 & 127,
            (n >> 14) as u8 & 127,
            (n >> 7) as u8 & 127,
            n as u8 & 127,
        ]
    }
    fn tag(version: u8, flags: u8, size: u32) -> Vec<u8> {
        let mut b = b"ID3".to_vec();
        b.extend([version, 0, flags]);
        b.extend(synchsafe(size));
        b.resize(10 + size as usize, 0x55);
        if version == 4 && flags & 0x10 != 0 {
            let mut footer = b[..10].to_vec();
            footer[..3].copy_from_slice(b"3DI");
            b.extend(footer);
        }
        b
    }
    #[test]
    fn all_versions_rates_modes_padding_and_crc_use_fixed_reference_lengths() {
        // Length vectors are explicit, independent of the parser's frame formula.
        for (version, rate_index, rate, index, kbps, len) in [
            (3, 0, 44100, 9, 128, 417),
            (3, 1, 48000, 14, 320, 960),
            (3, 2, 32000, 1, 32, 144),
            (2, 0, 22050, 8, 64, 208),
            (2, 1, 24000, 1, 8, 24),
            (2, 2, 16000, 8, 64, 288),
            (0, 0, 11025, 8, 64, 417),
            (0, 1, 12000, 8, 64, 384),
            (0, 2, 8000, 1, 8, 72),
        ] {
            for mode in 0..4 {
                for padding in [false, true] {
                    for crc in [false, true] {
                        let raw = header(version, index, rate_index, padding, crc, mode);
                        let mut bytes = Vec::new();
                        for _ in 0..3 {
                            append(&mut bytes, raw, len + usize::from(padding));
                        }
                        let info = inspect(&bytes).unwrap();
                        assert_eq!(info.sample_rate, Some(rate));
                        assert_eq!(info.channels, if mode == 3 { 1 } else { 2 });
                        assert_eq!(info.frame_bitrate_kbps, Some(kbps));
                        assert_eq!(
                            (
                                info.format,
                                info.codec,
                                info.lossy,
                                info.bit_depth,
                                info.dsd_level,
                                info.duration_seconds
                            ),
                            ("MPEG Audio", "MP3", true, None, None, None)
                        );
                        assert_eq!(info.provenance, "original_file_mpeg_headers");
                    }
                }
            }
        }
        // Stereo↔joint stereo and bitrates may vary; neither establishes global CBR/VBR.
        let mut bytes = Vec::new();
        for (index, len, mode) in [(9, 417, 0), (11, 626, 1), (14, 1044, 2)] {
            append(&mut bytes, header(3, index, 0, false, false, mode), len);
        }
        let info = inspect(&bytes).unwrap();
        assert_eq!(info.frame_bitrate_kbps, Some(128));
        assert_eq!(info.duration_seconds, None);
    }
    #[test]
    fn isolated_false_conflicting_sync_and_unsupported_variants_are_rejected() {
        for raw in [
            *b"RIFF",
            *b"OggS",
            [255; 4],
            [0xff, 0xf1, 0x50, 0x80],
            header(1, 9, 0, false, false, 0),
            header(3, 0, 0, false, false, 0),
            header(3, 15, 0, false, false, 0),
            header(3, 9, 3, false, false, 0),
        ] {
            let mut b = Vec::new();
            for _ in 0..3 {
                append(&mut b, raw, 417);
            }
            assert!(inspect(&b).is_err());
        }
        for layer in [0, 2, 3] {
            let mut b = fixture();
            for pos in [0, 417, 834] {
                b[pos + 1] = (b[pos + 1] & !6) | (layer << 1);
            }
            assert!(inspect(&b).is_err());
        }
        for conflict in [
            header(2, 8, 0, false, false, 0),
            header(3, 9, 1, false, false, 0),
            header(3, 9, 0, false, false, 3),
        ] {
            let mut b = fixture();
            b[417..421].copy_from_slice(&conflict);
            assert!(inspect(&b).is_err());
        }
        // Do not mistake MP3 frames carried after a known MP4 file-type box
        // for an elementary MPEG Audio stream/container.
        let mut boxed = [0, 0, 0, 16].to_vec();
        boxed.extend(b"ftypisom");
        boxed.extend([0; 4]);
        boxed.extend(fixture());
        assert_eq!(inspect(&boxed).unwrap_err(), InfoError::CorruptHeader);
        let b = fixture();
        for n in [0, 1, 3, 4, 40, 416, 417, 420, 833, 834, 838, 1250] {
            assert!(inspect(&b[..n]).is_err(), "truncated {n}");
        }
        let mut b = vec![0; 2000];
        b[..4].copy_from_slice(&header(3, 9, 0, false, false, 0));
        assert!(inspect(&b).is_err());
        for payload in [b"fLaC".as_slice(), b"RIFF", b"DSD ", b"not audio"] {
            let mut b = tag(3, 0, 20);
            b.extend(payload);
            assert!(inspect(&b).is_err());
        }
    }
    #[test]
    fn id3_versions_flags_sizes_footer_and_tag_only_do_not_establish_mp3() {
        for version in [2, 3, 4] {
            for flags in if version == 2 {
                vec![0, 0x80, 0x40, 0xc0]
            } else if version == 3 {
                vec![0, 0x80, 0x40, 0xe0]
            } else {
                vec![0, 0x80, 0x40, 0xf0]
            } {
                let mut b = tag(version, flags, 16);
                b.extend(fixture());
                assert_eq!(inspect(&b).unwrap().codec, "MP3");
            }
        }
        for version in [2, 3, 4] {
            assert!(inspect(&tag(version, 0, 10)).is_err());
            let mut b = tag(version, 0, 0);
            b.extend(fixture());
            b[5] = 1;
            assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
            let mut b = tag(version, 0, 0);
            b.extend(fixture());
            b[4] = 255;
            assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
            for pos in 6..10 {
                let mut b = tag(version, 0, 0);
                b.extend(fixture());
                b[pos] = 128;
                assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
            }
            let mut b = tag(version, 0, 0);
            b[6..10].copy_from_slice(&synchsafe(0x0fffffff));
            b.extend(fixture());
            assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
        }
        let mut b = tag(4, 0x10, 16);
        for n in 0..b.len() {
            assert!(inspect(&b[..n]).is_err());
        }
        b.extend(fixture());
        for pos in [26, 29, 30, 31, 32, 35] {
            let mut bad = b.clone();
            bad[pos] ^= 1;
            assert_eq!(inspect(&bad).unwrap_err(), InfoError::CorruptHeader);
        }
        for (v, flags, size) in [(3, 0x40, 9), (4, 0x40, 5)] {
            let mut b = tag(v, flags, size);
            b.extend(fixture());
            assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
        }
        let mut b = tag(3, 0, 0);
        b[3] = 5;
        b.extend(fixture());
        assert_eq!(inspect(&b).unwrap_err(), InfoError::UnsupportedFormat);
        for tags in [MAX_TAGS, MAX_TAGS + 1] {
            let mut b = Vec::new();
            for _ in 0..tags {
                b.extend(tag(3, 0, 0));
            }
            b.extend(fixture());
            assert_eq!(inspect(&b).is_ok(), tags == MAX_TAGS);
        }
    }
    struct Audit {
        cursor: Cursor<Vec<u8>>,
        allowed: Vec<std::ops::Range<u64>>,
        bytes: usize,
        reads: usize,
        seeks: usize,
    }
    impl Read for Audit {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            let pos = self.cursor.position();
            assert!(
                self.allowed
                    .iter()
                    .any(|r| pos >= r.start && pos + out.len() as u64 <= r.end),
                "read outside headers"
            );
            let n = self.cursor.read(out)?;
            self.bytes += n;
            Ok(n)
        }
    }
    impl Seek for Audit {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            assert!(matches!(p, SeekFrom::Start(_)));
            self.seeks += 1;
            self.cursor.seek(p)
        }
    }
    #[test]
    fn only_calculated_headers_are_read_no_xing_audio_or_tail_tag_loading() {
        let mut b = fixture();
        b[36..40].copy_from_slice(b"Xing");
        b.extend(b"TAG");
        b.extend([0; 125]);
        let len = b.len() as u64;
        let mut a = Audit {
            cursor: Cursor::new(b),
            allowed: vec![0..4, 417..421, 834..838],
            bytes: 0,
            reads: 0,
            seeks: 0,
        };
        let info = read_file_info(&mut a, len).unwrap();
        assert_eq!(a.bytes, 12);
        assert_eq!(a.seeks, 2);
        assert_eq!(info.duration_seconds, None);
    }
    #[test]
    fn maximum_size_id3_is_skipped_without_tag_or_payload_allocation_or_read() {
        struct Sparse {
            pos: u64,
            len: u64,
            segments: Vec<(u64, Vec<u8>)>,
            bytes: usize,
            seeks: usize,
        }
        impl Read for Sparse {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let (start, bytes) = self
                    .segments
                    .iter()
                    .find(|(start, bytes)| {
                        self.pos >= *start
                            && self.pos + out.len() as u64 <= *start + bytes.len() as u64
                    })
                    .expect("tag/payload was read");
                let offset = (self.pos - *start) as usize;
                out.copy_from_slice(&bytes[offset..offset + out.len()]);
                self.pos += out.len() as u64;
                self.bytes += out.len();
                Ok(out.len())
            }
        }
        impl Seek for Sparse {
            fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
                let SeekFrom::Start(p) = p else {
                    panic!("absolute only")
                };
                assert!(p <= self.len);
                self.pos = p;
                self.seeks += 1;
                Ok(p)
            }
        }
        let size = 0x0fffffffu32;
        let mut tag = b"ID3\x04\0\x10".to_vec();
        tag.extend(synchsafe(size));
        let mut footer = tag.clone();
        footer[..3].copy_from_slice(b"3DI");
        let first = 20 + size as u64;
        let mut segments = vec![(0, tag), (10 + size as u64, footer)];
        for p in [first, first + 417, first + 834] {
            segments.push((p, header(3, 9, 0, false, false, 0).to_vec()));
        }
        let mut r = Sparse {
            pos: 0,
            len: first + 1251,
            segments,
            bytes: 0,
            seeks: 0,
        };
        let len = r.len;
        assert!(read_file_info(&mut r, len).is_ok());
        assert_eq!(r.bytes, 32);
        assert!(r.seeks < 8);
    }
    #[test]
    fn junk_candidate_and_io_budgets_are_finite_even_with_partial_or_interrupted_reads() {
        for junk in [0, MAX_JUNK, MAX_JUNK + 1] {
            let mut b = vec![0; junk];
            b.extend(fixture());
            assert_eq!(inspect(&b).is_ok(), junk <= MAX_JUNK);
        }
        let mut candidates = vec![0; MAX_JUNK + 2000];
        for i in 0..MAX_CANDIDATES + 1 {
            candidates[i * 8..i * 8 + 4].copy_from_slice(&header(3, 9, 0, false, false, 0));
        }
        let mut r = Cursor::new(candidates);
        let len = r.get_ref().len() as u64;
        assert_eq!(
            read_file_info(&mut r, len).unwrap_err(),
            InfoError::UnsupportedFormat
        );
        assert!(r.position() < MAX_JUNK as u64);
        struct Partial {
            r: Cursor<Vec<u8>>,
            calls: usize,
            interrupt: bool,
        }
        impl Read for Partial {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.calls += 1;
                if self.interrupt && self.r.position() >= 4 {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.r.read(&mut out[..1])
            }
        }
        impl Seek for Partial {
            fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
                self.r.seek(p)
            }
        }
        let b = fixture();
        let len = b.len() as u64;
        let mut r = Partial {
            r: Cursor::new(b),
            calls: 0,
            interrupt: false,
        };
        assert!(read_file_info(&mut r, len).is_ok());
        assert_eq!(r.calls, 12);
        let mut b = tag(3, 0, 0);
        b.extend(fixture());
        let len = b.len() as u64;
        let mut r = Partial {
            r: Cursor::new(b),
            calls: 0,
            interrupt: true,
        };
        assert_eq!(
            read_file_info(&mut r, len).unwrap_err(),
            InfoError::UnsupportedFormat
        );
        assert!(r.calls <= MAX_READS);
    }
}
