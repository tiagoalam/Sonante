//! Sony DSF 1.01 (II) and Philips DSDIFF 1.5 (§§2–3).
//! Headers only. No allocation based on file fields; no payload/tag reads.
//! DST is recognized but unsupported here: inspecting frames is outside this parser.
use super::{InfoError, LocalFileInfo};
use std::io::{self, Read, Seek, SeekFrom};

// Includes the signature read by the dispatcher. Bounds calls as well as bytes:
// a pathological reader returning one byte/Interrupted cannot cause an endless loop.
const MAX_BYTES: usize = 16 * 1024;
const MAX_READS: usize = 2048;
const MAX_SEEKS: usize = 512;
const MAX_CHUNKS: usize = 128; // shared across FRM8 and PROP, no recursion
const MAX_CHANNELS: usize = 32;

fn io_error(error: io::Error) -> InfoError {
    match error.kind() {
        io::ErrorKind::UnexpectedEof => InfoError::CorruptHeader,
        io::ErrorKind::PermissionDenied => InfoError::PermissionDenied,
        _ => InfoError::Unavailable,
    }
}
struct Headers<'a, R> {
    reader: &'a mut R,
    len: u64,
    pos: u64,
    bytes: usize,
    reads: usize,
    seeks: usize,
    chunks: usize,
}
impl<'a, R: Read + Seek> Headers<'a, R> {
    fn new(reader: &'a mut R, len: u64) -> Result<Self, InfoError> {
        if len < 4 {
            return Err(InfoError::CorruptHeader);
        }
        Ok(Self {
            reader,
            len,
            pos: 4,
            bytes: 4,
            reads: 64,
            seeks: 0,
            chunks: 0,
        })
    }
    fn read<const N: usize>(&mut self) -> Result<[u8; N], InfoError> {
        if self
            .pos
            .checked_add(N as u64)
            .filter(|end| *end <= self.len)
            .is_none()
        {
            return Err(InfoError::CorruptHeader);
        }
        if self.bytes + N > MAX_BYTES {
            return Err(InfoError::UnsupportedFormat);
        }
        let mut data = [0; N];
        let mut offset = 0;
        while offset < N {
            if self.reads >= MAX_READS {
                return Err(InfoError::UnsupportedFormat);
            }
            self.reads += 1;
            match self.reader.read(&mut data[offset..]) {
                Ok(0) => return Err(InfoError::CorruptHeader),
                Ok(n) => {
                    offset += n;
                    self.pos += n as u64;
                    self.bytes += n;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io_error(e)),
            }
        }
        Ok(data)
    }
    fn seek(&mut self, pos: u64) -> Result<(), InfoError> {
        if pos > self.len {
            return Err(InfoError::CorruptHeader);
        }
        if pos == self.pos {
            return Ok(());
        }
        if self.seeks >= MAX_SEEKS {
            return Err(InfoError::UnsupportedFormat);
        }
        self.seeks += 1;
        if self.reader.seek(SeekFrom::Start(pos)).map_err(io_error)? != pos {
            return Err(InfoError::Unavailable);
        }
        self.pos = pos;
        Ok(())
    }
    fn chunk(&mut self, parent_end: u64) -> Result<Chunk, InfoError> {
        if parent_end > self.len
            || self
                .pos
                .checked_add(12)
                .filter(|p| *p <= parent_end)
                .is_none()
        {
            return Err(InfoError::CorruptHeader);
        }
        if self.chunks >= MAX_CHUNKS {
            return Err(InfoError::UnsupportedFormat);
        }
        self.chunks += 1;
        let id = self.read::<4>()?;
        if !valid_id(&id) {
            return Err(InfoError::CorruptHeader);
        }
        let size = u64::from_be_bytes(self.read()?);
        let end = self.pos.checked_add(size).ok_or(InfoError::CorruptHeader)?;
        let next = end
            .checked_add(size % 2)
            .filter(|p| *p <= parent_end)
            .ok_or(InfoError::CorruptHeader)?;
        Ok(Chunk {
            id,
            size,
            end,
            next,
        })
    }
    fn finish(&mut self, chunk: &Chunk) -> Result<(), InfoError> {
        if self.pos > chunk.end {
            return Err(InfoError::CorruptHeader);
        }
        self.seek(chunk.end)?;
        if chunk.next != chunk.end && self.read::<1>()? != [0] {
            return Err(InfoError::CorruptHeader);
        }
        Ok(())
    }
}
struct Chunk {
    id: [u8; 4],
    size: u64,
    end: u64,
    next: u64,
}
fn valid_id(id: &[u8; 4]) -> bool {
    id[0] != b' ' && id.iter().all(|c| (0x20..=0x7e).contains(c))
}
fn dsd_info(
    container: &'static str,
    rate: u32,
    channels: u8,
    duration: Option<f64>,
) -> LocalFileInfo {
    LocalFileInfo {
        provenance: "original_file_dsd_header",
        format: container,
        codec: "DSD",
        lossy: false,
        compression: "uncompressed",
        sample_type: "dsd",
        sample_rate_basis: "encoded_header",
        original_sample_rate: None,
        frame_bitrate_kbps: None,
        sample_rate: Some(rate),
        bit_depth: None,
        channels,
        duration_seconds: duration,
        // No rounding or classification of unrecognized rates as a nearby level.
        dsd_level: match rate {
            2_822_400 => Some(64),
            5_644_800 => Some(128),
            11_289_600 => Some(256),
            _ => None,
        },
        revision: String::new(),
    }
}

pub(super) fn read_dsf(
    reader: &mut (impl Read + Seek),
    len: u64,
) -> Result<LocalFileInfo, InfoError> {
    let mut h = Headers::new(reader, len)?;
    if u64::from_le_bytes(h.read()?) != 28 || u64::from_le_bytes(h.read()?) != len {
        return Err(InfoError::CorruptHeader);
    }
    let metadata = u64::from_le_bytes(h.read()?);
    if h.read::<4>()? != *b"fmt " {
        return Err(InfoError::CorruptHeader);
    }
    let fmt_size = u64::from_le_bytes(h.read()?);
    if fmt_size != 52 {
        return Err(InfoError::UnsupportedFormat);
    }
    let version = u32::from_le_bytes(h.read()?);
    let encoding = u32::from_le_bytes(h.read()?);
    if version != 1 || encoding != 0 {
        return Err(InfoError::UnsupportedFormat);
    }
    let channel_type = u32::from_le_bytes(h.read()?);
    let channels = u32::from_le_bytes(h.read()?);
    let rate = u32::from_le_bytes(h.read()?);
    let bit_order = u32::from_le_bytes(h.read()?);
    let samples = u64::from_le_bytes(h.read()?);
    let block = u32::from_le_bytes(h.read()?);
    let reserved = u32::from_le_bytes(h.read()?);
    let declared_channels = [1, 2, 3, 4, 4, 5, 6]
        .get(channel_type.wrapping_sub(1) as usize)
        .copied();
    if declared_channels != Some(channels)
        || rate == 0
        || ![1, 8].contains(&bit_order)
        || reserved != 0
    {
        return Err(InfoError::CorruptHeader);
    }
    if block != 4096 {
        return Err(InfoError::UnsupportedFormat);
    }
    if h.read::<4>()? != *b"data" {
        return Err(InfoError::CorruptHeader);
    }
    let data_size = u64::from_le_bytes(h.read()?);
    let data_end = 80u64
        .checked_add(data_size)
        .filter(|p| *p <= len)
        .ok_or(InfoError::CorruptHeader)?;
    let blocks = samples / 32768 + u64::from(samples % 32768 != 0);
    let payload = blocks
        .checked_mul(4096)
        .and_then(|v| v.checked_mul(channels as u64))
        .ok_or(InfoError::CorruptHeader)?;
    if data_size.checked_sub(12) != Some(payload)
        || (metadata == 0 && data_end != len)
        || (metadata != 0 && (metadata != data_end || len.saturating_sub(metadata) < 10))
    {
        return Err(InfoError::CorruptHeader);
    }
    // Exactly 92 bytes read. Sample count is per channel, not bytes/block padding.
    Ok(dsd_info(
        "DSF",
        rate,
        channels as u8,
        Some(samples as f64 / rate as f64),
    ))
}

struct Properties {
    rate: u32,
    channels: u8,
    codec: [u8; 4],
}
fn properties<R: Read + Seek>(h: &mut Headers<R>, chunk: &Chunk) -> Result<Properties, InfoError> {
    if chunk.size < 4 || h.read::<4>()? != *b"SND " {
        return Err(InfoError::UnsupportedFormat);
    }
    let (mut rate, mut channels, mut codec) = (None, None, None);
    let (mut abss, mut lsco) = (false, false);
    while h.pos < chunk.end {
        let c = h.chunk(chunk.end)?;
        match &c.id {
            b"FS  " => {
                if rate.is_some() || c.size != 4 {
                    return Err(InfoError::CorruptHeader);
                }
                let value = u32::from_be_bytes(h.read()?);
                if value == 0 {
                    return Err(InfoError::CorruptHeader);
                }
                rate = Some(value);
            }
            b"CHNL" => {
                if channels.is_some() || c.size < 2 {
                    return Err(InfoError::CorruptHeader);
                }
                let count = u16::from_be_bytes(h.read()?) as usize;
                if count == 0 || c.size != 2 + count as u64 * 4 {
                    return Err(InfoError::CorruptHeader);
                }
                if count > MAX_CHANNELS {
                    return Err(InfoError::UnsupportedFormat);
                }
                let mut ids = [[0; 4]; MAX_CHANNELS];
                for i in 0..count {
                    let id = h.read::<4>()?;
                    if !valid_id(&id) || ids[..i].contains(&id) {
                        return Err(InfoError::CorruptHeader);
                    }
                    ids[i] = id;
                }
                channels = Some(count as u8);
            }
            b"CMPR" => {
                if codec.is_some() || c.size < 5 {
                    return Err(InfoError::CorruptHeader);
                }
                let id = h.read::<4>()?;
                let name_len = h.read::<1>()?[0];
                if c.size != 5 + name_len as u64 {
                    return Err(InfoError::CorruptHeader);
                }
                if !matches!(&id, b"DSD " | b"DST ") {
                    return Err(InfoError::UnsupportedFormat);
                }
                codec = Some(id); // compression name is skipped, never allocated
            }
            b"ABSS" => {
                if abss || c.size != 8 {
                    return Err(InfoError::CorruptHeader);
                }
                abss = true;
            }
            b"LSCO" => {
                if lsco || c.size != 2 {
                    return Err(InfoError::CorruptHeader);
                }
                lsco = true;
            }
            _ => {} // specification permits unknown chunks, with validated bounds/pad
        }
        h.finish(&c)?;
    }
    Ok(Properties {
        rate: rate.ok_or(InfoError::CorruptHeader)?,
        channels: channels.ok_or(InfoError::CorruptHeader)?,
        codec: codec.ok_or(InfoError::CorruptHeader)?,
    })
}

pub(super) fn read_dff(
    reader: &mut (impl Read + Seek),
    len: u64,
) -> Result<LocalFileInfo, InfoError> {
    let mut h = Headers::new(reader, len)?;
    let form_size = u64::from_be_bytes(h.read()?);
    if form_size < 4 || form_size % 2 != 0 || form_size.checked_add(12) != Some(len) {
        return Err(InfoError::CorruptHeader);
    }
    if h.read::<4>()? != *b"DSD " {
        return Err(InfoError::UnsupportedFormat);
    }
    let version = h.chunk(len)?;
    if version.id != *b"FVER" || version.size != 4 {
        return Err(InfoError::CorruptHeader);
    }
    let value = u32::from_be_bytes(h.read()?);
    if value >> 24 != 1 || value & 0xffff != 0 {
        return Err(InfoError::UnsupportedFormat);
    }
    h.finish(&version)?;
    let mut prop: Option<Properties> = None;
    let mut data_size = None;
    while h.pos < len {
        let c = h.chunk(len)?;
        match &c.id {
            b"FVER" => return Err(InfoError::CorruptHeader),
            b"PROP" => {
                if prop.is_some() || data_size.is_some() {
                    return Err(InfoError::CorruptHeader);
                }
                prop = Some(properties(&mut h, &c)?);
            }
            b"DSD " | b"DST " => {
                let p = prop.as_ref().ok_or(InfoError::CorruptHeader)?;
                if data_size.is_some() || c.id != p.codec {
                    return Err(InfoError::CorruptHeader);
                }
                if c.id == *b"DST " {
                    return Err(InfoError::UnsupportedFormat);
                }
                if c.size % p.channels as u64 != 0 {
                    return Err(InfoError::CorruptHeader);
                }
                data_size = Some(c.size);
            }
            _ => {}
        }
        h.finish(&c)?; // seek across data/tags; never read their contents
    }
    let p = prop.ok_or(InfoError::CorruptHeader)?;
    let size = data_size.ok_or(InfoError::CorruptHeader)?;
    let samples = (size / p.channels as u64)
        .checked_mul(8)
        .ok_or(InfoError::CorruptHeader)?;
    Ok(dsd_info(
        "DSDIFF",
        p.rate,
        p.channels,
        Some(samples as f64 / p.rate as f64),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::local_file_info::read_file_info;
    use std::io::Cursor;
    pub(crate) fn dsf(rate: u32, order: u32) -> Vec<u8> {
        let mut b = b"DSD ".to_vec();
        b.extend(28u64.to_le_bytes());
        b.extend(8284u64.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend(b"fmt ");
        b.extend(52u64.to_le_bytes());
        for n in [1, 0, 2, 2, rate, order] {
            b.extend(n.to_le_bytes());
        }
        b.extend(8u64.to_le_bytes());
        b.extend(4096u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(b"data");
        b.extend(8204u64.to_le_bytes());
        b.resize(8284, 0);
        b
    }
    pub(crate) fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut b = id.to_vec();
        b.extend((data.len() as u64).to_be_bytes());
        b.extend(data);
        if data.len() % 2 != 0 {
            b.push(0);
        }
        b
    }
    fn form(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut content = b"DSD ".to_vec();
        for c in chunks {
            content.extend(c);
        }
        chunk(b"FRM8", &content)
    }
    fn version() -> Vec<u8> {
        chunk(b"FVER", &0x01050000u32.to_be_bytes())
    }
    fn props(rate: u32, codec: &[u8; 4]) -> Vec<u8> {
        let mut b = b"SND ".to_vec();
        b.extend(chunk(b"FS  ", &rate.to_be_bytes()));
        b.extend(chunk(b"CHNL", b"\0\x02SLFTSRGT"));
        b.extend(chunk(b"CMPR", &[codec.as_slice(), b"\x03DSD"].concat()));
        b
    }
    pub(crate) fn dff(rate: u32) -> Vec<u8> {
        form(&[
            version(),
            chunk(b"PROP", &props(rate, b"DSD ")),
            chunk(b"DSD ", &[0; 16]),
        ])
    }
    fn inspect(b: &[u8]) -> Result<LocalFileInfo, InfoError> {
        read_file_info(&mut Cursor::new(b), b.len() as u64)
    }
    #[test]
    fn rates_orders_and_durations_are_from_headers_not_extension_or_transport() {
        for (rate, level) in [
            (2822400, Some(64)),
            (5644800, Some(128)),
            (11289600, Some(256)),
            (22579200, None),
            (2822401, None),
        ] {
            for order in [1, 8] {
                let bytes = dsf(rate, order);
                let mut r = Cursor::new(&bytes);
                let info = read_file_info(&mut r, bytes.len() as u64).unwrap();
                assert_eq!(
                    (
                        info.format,
                        info.codec,
                        info.bit_depth,
                        info.channels,
                        info.dsd_level
                    ),
                    ("DSF", "DSD", None, 2, level)
                );
                assert_eq!(info.sample_rate, Some(rate));
                assert_eq!(info.duration_seconds, Some(8.0 / rate as f64));
                assert_eq!(r.position(), 92);
            }
            let info = inspect(&dff(rate)).unwrap();
            assert_eq!(
                (
                    info.format,
                    info.codec,
                    info.bit_depth,
                    info.channels,
                    info.dsd_level
                ),
                ("DSDIFF", "DSD", None, 2, level)
            );
            assert_eq!(info.sample_rate, Some(rate));
            assert_eq!(info.duration_seconds, Some(64.0 / rate as f64));
        }
    }
    #[test]
    fn dsf_rejects_bad_sizes_fields_duplicates_and_overflows() {
        let good = dsf(2822400, 1);
        for n in 0..92 {
            assert_eq!(inspect(&good[..n]).unwrap_err(), InfoError::CorruptHeader);
        }
        for offset in [4, 12, 28, 48, 52, 60, 72, 76, 80] {
            let mut b = good.clone();
            b[offset] = 255;
            assert!(inspect(&b).is_err(), "offset {offset}");
        }
        for offset in [4, 12, 20, 32, 64, 84] {
            let mut b = good.clone();
            b[offset..offset + 8].fill(255);
            assert!(inspect(&b).is_err());
        }
        let mut b = good.clone();
        b[60..64].copy_from_slice(&16u32.to_le_bytes());
        assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
        for (offset, value) in [(40, 2u32), (44, 1), (72, 8192)] {
            let mut b = good.clone();
            b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(inspect(&b).unwrap_err(), InfoError::UnsupportedFormat);
        }
        // Extra fmt instead of data or after data is never accepted as another stream.
        let mut b = good.clone();
        b[80..84].copy_from_slice(b"fmt ");
        assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
        let mut b = good.clone();
        b.extend(b"fmt ");
        let len = b.len() as u64;
        b[12..20].copy_from_slice(&len.to_le_bytes());
        assert_eq!(inspect(&b).unwrap_err(), InfoError::CorruptHeader);
    }
    #[test]
    fn dff_requires_unique_consistent_required_chunks_and_valid_parents() {
        let p = props(2822400, b"DSD ");
        let prop = chunk(b"PROP", &p);
        let data = chunk(b"DSD ", &[0; 16]);
        for parts in [
            vec![prop.clone(), data.clone()],
            vec![version(), data.clone(), prop.clone()],
            vec![version(), prop.clone()],
            vec![version(), prop.clone(), prop.clone(), data.clone()],
            vec![version(), version(), prop.clone(), data.clone()],
            vec![version(), prop.clone(), data.clone(), data.clone()],
            vec![version(), prop.clone(), chunk(b"DST ", &[])],
        ] {
            assert_eq!(
                inspect(&form(&parts)).unwrap_err(),
                InfoError::CorruptHeader
            );
        }
        for (id, body) in [
            (b"FS  ", 2822400u32.to_be_bytes().to_vec()),
            (b"CHNL", b"\0\x02SLFTSRGT".to_vec()),
            (b"CMPR", b"DSD \x03DSD".to_vec()),
        ] {
            let mut dup = p.clone();
            dup.extend(chunk(id, &body));
            assert_eq!(
                inspect(&form(&[version(), chunk(b"PROP", &dup), data.clone()])).unwrap_err(),
                InfoError::CorruptHeader
            );
        }
        for body in [
            b"\0\0".to_vec(),
            b"\0\x02SLFTSLFT".to_vec(),
            b"\0\x02SLFT".to_vec(),
        ] {
            let mut pr = b"SND ".to_vec();
            pr.extend(chunk(b"FS  ", &2822400u32.to_be_bytes()));
            pr.extend(chunk(b"CHNL", &body));
            pr.extend(chunk(b"CMPR", b"DSD \x03DSD"));
            assert_eq!(
                inspect(&form(&[version(), chunk(b"PROP", &pr), data.clone()])).unwrap_err(),
                InfoError::CorruptHeader
            );
        }
        let b = dff(2822400);
        for n in 0..b.len() {
            assert!(inspect(&b[..n]).is_err());
        }
        for off in [4, 20, 36, 52, 68, 90] {
            let mut bad = b.clone();
            if off + 8 <= bad.len() {
                bad[off..off + 8].fill(255);
                assert!(inspect(&bad).is_err());
            }
        }
        let mut nested = props(2822400, b"DSD ");
        nested[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            inspect(&form(&[version(), chunk(b"PROP", &nested), data.clone()])).unwrap_err(),
            InfoError::CorruptHeader
        );
        let mut padded = props(2822400, b"DSD ");
        padded.extend(chunk(b"JUNK", &[1]));
        let last = padded.len() - 1;
        padded[last] = 1;
        assert_eq!(
            inspect(&form(&[version(), chunk(b"PROP", &padded), data.clone()])).unwrap_err(),
            InfoError::CorruptHeader
        );
        assert_eq!(
            inspect(&form(&[
                version(),
                chunk(b"PROP", &props(2822400, b"DST ")),
                chunk(b"DST ", &[0; 18])
            ]))
            .unwrap_err(),
            InfoError::UnsupportedFormat
        );
        assert_eq!(
            inspect(&form(&[
                version(),
                chunk(b"PROP", &props(2822400, b"WAVE")),
                data
            ]))
            .unwrap_err(),
            InfoError::UnsupportedFormat
        );
    }
    struct Audit {
        data: Cursor<Vec<u8>>,
        forbidden: Vec<std::ops::Range<u64>>,
        bytes: usize,
        reads: usize,
        seeks: usize,
    }
    impl Read for Audit {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            let pos = self.data.position();
            let end = pos + out.len() as u64;
            assert!(
                !self.forbidden.iter().any(|r| pos < r.end && end > r.start),
                "attempted payload/tag read"
            );
            let n = self.data.read(out)?;
            self.bytes += n;
            Ok(n)
        }
    }
    impl Seek for Audit {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.seeks += 1;
            self.data.seek(p)
        }
    }
    #[test]
    fn no_payload_or_tag_read_and_fixed_allocation_even_when_tag_is_large() {
        let mut ds = dsf(2822400, 8);
        let tag = ds.len() as u64;
        ds.extend(vec![255; 1024 * 1024]);
        let len = ds.len() as u64;
        ds[12..20].copy_from_slice(&len.to_le_bytes());
        ds[20..28].copy_from_slice(&tag.to_le_bytes());
        let mut audit = Audit {
            data: Cursor::new(ds),
            forbidden: vec![92..len],
            bytes: 0,
            reads: 0,
            seeks: 0,
        };
        assert!(read_file_info(&mut audit, len).is_ok());
        assert_eq!(audit.bytes, 92);
        assert_eq!(audit.seeks, 0);
        let head = form(&[
            version(),
            chunk(b"PROP", &props(2822400, b"DSD ")),
            chunk(b"DSD ", &[0; 16]),
            chunk(b"ID3 ", &vec![255; 1024 * 1024]),
        ]);
        let raw_start = (dff(2822400).len() - 16) as u64;
        let tag_start = raw_start + 16 + 12;
        let len = head.len() as u64;
        let mut audit = Audit {
            data: Cursor::new(head),
            forbidden: vec![raw_start..raw_start + 16, tag_start..len],
            bytes: 0,
            reads: 0,
            seeks: 0,
        };
        assert!(read_file_info(&mut audit, len).is_ok());
        assert!(audit.bytes <= 256);
        assert!(audit.seeks <= 8);
    }
    #[test]
    fn chunk_and_interrupted_read_budgets_are_finite() {
        let mut parts = vec![version()];
        for _ in 0..MAX_CHUNKS {
            parts.push(chunk(b"JUNK", &[]));
        }
        let b = form(&parts);
        let len = b.len() as u64;
        let mut a = Audit {
            data: Cursor::new(b),
            forbidden: vec![],
            bytes: 0,
            reads: 0,
            seeks: 0,
        };
        assert_eq!(
            read_file_info(&mut a, len).unwrap_err(),
            InfoError::UnsupportedFormat
        );
        assert!(a.bytes <= MAX_BYTES);
        assert!(a.reads <= MAX_READS);
        assert!(a.seeks <= MAX_SEEKS);
        struct Interrupted {
            calls: usize,
        }
        impl Read for Interrupted {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                self.calls += 1;
                Err(io::ErrorKind::Interrupted.into())
            }
        }
        impl Seek for Interrupted {
            fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
                panic!("no seek expected")
            }
        }
        let mut r = Interrupted { calls: 0 };
        assert_eq!(
            read_file_info(&mut r, 100).unwrap_err(),
            InfoError::UnsupportedFormat
        );
        assert_eq!(r.calls, 64);
    }
    #[test]
    fn required_fields_variants_and_channel_counts_are_validated() {
        let fs = chunk(b"FS  ", &2822400u32.to_be_bytes());
        let ch = chunk(b"CHNL", b"\0\x02SLFTSRGT");
        let cm = chunk(b"CMPR", b"DSD \x03DSD");
        for missing in 0..3 {
            let mut pr = b"SND ".to_vec();
            for (i, c) in [&fs, &ch, &cm].iter().enumerate() {
                if i != missing {
                    pr.extend(*c);
                }
            }
            assert_eq!(
                inspect(&form(&[
                    version(),
                    chunk(b"PROP", &pr),
                    chunk(b"DSD ", &[0; 16])
                ]))
                .unwrap_err(),
                InfoError::CorruptHeader
            );
        }
        for count in [1u16, 6, 32, 33] {
            let mut channels = count.to_be_bytes().to_vec();
            for i in 0..count {
                channels.extend(format!("C{i:03}").as_bytes());
            }
            let mut pr = b"SND ".to_vec();
            pr.extend(&fs);
            pr.extend(chunk(b"CHNL", &channels));
            pr.extend(&cm);
            let result = inspect(&form(&[
                version(),
                chunk(b"PROP", &pr),
                chunk(b"DSD ", &vec![0; count as usize]),
            ]));
            if count > 32 {
                assert_eq!(result.unwrap_err(), InfoError::UnsupportedFormat);
            } else {
                assert_eq!(result.unwrap().channels, count as u8);
            }
        }
        let mut pr = b"SND ".to_vec();
        pr.extend(chunk(b"FS  ", &0u32.to_be_bytes()));
        pr.extend(&ch);
        pr.extend(&cm);
        assert_eq!(
            inspect(&form(&[
                version(),
                chunk(b"PROP", &pr),
                chunk(b"DSD ", &[0; 16])
            ]))
            .unwrap_err(),
            InfoError::CorruptHeader
        );
        let mut wrong_name = props(2822400, b"DSD ");
        let pos = wrong_name.windows(4).position(|s| s == b"CMPR").unwrap();
        wrong_name[pos + 16] = 255;
        assert_eq!(
            inspect(&form(&[
                version(),
                chunk(b"PROP", &wrong_name),
                chunk(b"DSD ", &[0; 16])
            ]))
            .unwrap_err(),
            InfoError::CorruptHeader
        );
        for v in [0x02000000u32, 0x01050001] {
            assert_eq!(
                inspect(&form(&[chunk(b"FVER", &v.to_be_bytes())])).unwrap_err(),
                InfoError::UnsupportedFormat
            );
        }
        let mut mono = dsf(2822400, 1);
        mono[48..52].copy_from_slice(&1u32.to_le_bytes());
        mono[52..56].copy_from_slice(&1u32.to_le_bytes());
        mono.truncate(4188);
        mono[12..20].copy_from_slice(&4188u64.to_le_bytes());
        mono[84..92].copy_from_slice(&4108u64.to_le_bytes());
        assert_eq!(inspect(&mono).unwrap().channels, 1);
        let mut zero = dsf(2822400, 1);
        zero[56..60].fill(0);
        assert_eq!(inspect(&zero).unwrap_err(), InfoError::CorruptHeader);
    }
    #[test]
    fn huge_skipped_chunks_use_only_validated_absolute_seeks_no_payload_reads() {
        // A sparse logical file: no allocation or byte read proportional to its size.
        struct Sparse {
            pos: u64,
            len: u64,
            head: Vec<u8>,
            tail: Vec<u8>,
            bytes: usize,
            seeks: usize,
        }
        impl Read for Sparse {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let tail_start = self.len - self.tail.len() as u64;
                let source = if self.pos < self.head.len() as u64 {
                    &self.head[self.pos as usize..]
                } else if self.pos >= tail_start && self.pos < tail_start + 12 {
                    &self.tail[(self.pos - tail_start) as usize..12]
                } else {
                    panic!("payload/tag was read")
                };
                let n = source.len().min(out.len());
                out[..n].copy_from_slice(&source[..n]);
                self.pos += n as u64;
                self.bytes += n;
                Ok(n)
            }
        }
        impl Seek for Sparse {
            fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
                let SeekFrom::Start(pos) = p else {
                    panic!("absolute seeks only")
                };
                assert!(pos <= self.len);
                self.pos = pos;
                self.seeks += 1;
                Ok(pos)
            }
        }
        let mut prefix = b"DSD ".to_vec();
        prefix.extend(version());
        prefix.extend(chunk(b"PROP", &props(2822400, b"DSD ")));
        prefix.extend(b"ID3 ");
        prefix.extend((1u64 << 40).to_be_bytes());
        let tail = chunk(b"DSD ", &[0; 16]);
        let len = 12 + prefix.len() as u64 + (1u64 << 40) + tail.len() as u64;
        let mut head = b"FRM8".to_vec();
        head.extend((len - 12).to_be_bytes());
        head.extend(prefix);
        let mut r = Sparse {
            pos: 0,
            len,
            head,
            tail,
            bytes: 0,
            seeks: 0,
        };
        // The only read into tail is its 12-byte chunk header; finish seeks to EOF.
        assert!(read_file_info(&mut r, len).is_ok());
        assert!(r.bytes < 256);
        assert!(r.seeks < 8);
    }
}
