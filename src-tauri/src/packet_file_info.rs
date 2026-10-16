//! Header-only paths where the public Lofty readers scan/allocate audio or tags.
//! Contracts: Lofty 0.25.4 aac/header, musepack/sv8; RFC 3533/7845/9639.
use super::*;

pub(super) fn adts<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<LocalFileInfo, InfoError> {
    r.max_bytes = 8192;
    r.max_reads = 2048;
    r.max_seeks = 64;
    let mut pos = 0;
    let mut identity = None;
    for _ in 0..3 {
        let h = r.at::<7>(pos)?;
        if h[0] != 0xff || h[1] & 0xf6 != 0xf0 {
            return bad();
        }
        let profile = (h[2] >> 6) + 1;
        let idx = (h[2] >> 2) & 15;
        if profile != 2 || idx > 12 || h[6] & 3 != 0 {
            return unsupported();
        }
        let rate = [
            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
        ][idx as usize];
        let conf = ((h[2] & 1) << 2) | (h[3] >> 6);
        let channels = match conf {
            1..=6 => conf,
            7 => 8,
            _ => return unsupported(),
        };
        let length = (u64::from(h[3] & 3) << 11) | (u64::from(h[4]) << 3) | u64::from(h[5] >> 5);
        let min = if h[1] & 1 == 0 { 9 } else { 7 };
        if length <= min || pos.checked_add(length).filter(|n| *n <= r.len).is_none() {
            return bad();
        }
        let key = (profile, rate, channels, h[1] & 8);
        if identity.is_some_and(|v| v != key) {
            return bad();
        }
        identity = Some(key);
        pos += length;
    }
    let (_, rate, ch, _) = identity.unwrap();
    let mut out = info("ADTS", "AAC", "lossy", rate, ch, None);
    // ADTS does not prove absence of implicit SBR. Never call the core clock
    // the effective output sample rate of HE-AAC.
    out.sample_rate_basis = "aac_core_header";
    Ok(out)
}

pub(super) fn ogg<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<LocalFileInfo, InfoError> {
    let mut pos = 0;
    let mut serial = None;
    let mut sequence = 0u32;
    let mut out = None;
    let mut ended = false;
    let mut last_granule = None;
    let mut pending = false;
    for _ in 0..1024 {
        if pos == r.len {
            break;
        }
        if ended {
            return unsupported();
        } // chained streams are not selected silently
        let h = r.at::<27>(pos)?;
        if &h[..4] != b"OggS" || h[4] != 0 || h[5] & !7 != 0 {
            return bad();
        }
        let id = le32(&h[14..]);
        let seq = le32(&h[18..]);
        if let Some(old) = serial {
            if old != id || h[5] & 2 != 0 {
                return unsupported();
            }
            if seq != sequence {
                return bad();
            }
        } else {
            if h[5] & 3 != 2 || seq != 0 {
                return bad();
            }
            serial = Some(id);
        }
        sequence = seq.checked_add(1).ok_or(InfoError::UnsupportedFormat)?;
        if (h[5] & 1 != 0) != pending {
            return bad();
        }
        let n = usize::from(h[26]);
        if n == 0 {
            return unsupported();
        }
        let laces = r.blob(pos + 27, n as u64)?;
        pending = laces.last() == Some(&255);
        let body = pos + 27 + n as u64;
        let size = laces.iter().map(|x| u64::from(*x)).sum::<u64>();
        let end = body
            .checked_add(size)
            .filter(|end| *end <= r.len)
            .ok_or(InfoError::CorruptHeader)?;
        if out.is_none() {
            let mut packet_size = 0u64;
            let mut complete = false;
            for x in &laces {
                packet_size += u64::from(*x);
                if *x < 255 {
                    complete = true;
                    break;
                }
            }
            if !complete || packet_size > 512 {
                return unsupported();
            }
            let p = r.blob(body, packet_size)?;
            out = Some(identification(&p)?);
        }
        let granule = u64::from_le_bytes(h[6..14].try_into().unwrap());
        if granule != u64::MAX
            && (granule > i64::MAX as u64 || last_granule.is_some_and(|old| granule < old))
        {
            return bad();
        }
        last_granule = (granule != u64::MAX).then_some(granule);
        ended = h[5] & 4 != 0;
        pos = end;
    }
    if pos != r.len {
        return unsupported();
    } // page/IO budget: refuse, never pretend single stream
    if !ended {
        return bad();
    }
    let mut result = out.ok_or(InfoError::CorruptHeader)?;
    if result.codec == "Vorbis" {
        result.duration_seconds = last_granule
            .filter(|n| *n != 0)
            .map(|n| n as f64 / f64::from(result.sample_rate.unwrap()));
    }
    // Opus duration would need pre-skip/end-trim validation; not inferred here.
    Ok(result)
}
fn identification(p: &[u8]) -> Result<LocalFileInfo, InfoError> {
    if p.starts_with(b"\x01vorbis") {
        if p.len() != 30 || le32(&p[7..]) != 0 {
            return bad();
        }
        let rate = le32(&p[12..]);
        let ch = valid(rate, u16::from(p[11]))?;
        let small = p[28] & 15;
        let large = p[28] >> 4;
        if small < 6 || large > 13 || small > large || p[29] != 1 {
            return bad();
        }
        Ok(info("Ogg", "Vorbis", "lossy", rate, ch, None))
    } else if p.starts_with(b"OpusHead") {
        if p.len() < 19 || p[8] > 15 || p[8] == 0 {
            return unsupported();
        }
        let ch = p[9];
        if p[18] != 0 || ch == 0 || ch > 2 || p.len() != 19 {
            return unsupported();
        }
        let mut result = info("Ogg", "Opus", "lossy", 48000, ch, None);
        result.sample_rate_basis = "opus_decode_clock";
        result.original_sample_rate = match le32(&p[12..]) {
            0 => None,
            n => Some(n),
        };
        Ok(result)
    } else if p.starts_with(b"\x7fFLAC") {
        if p.len() != 51 || p[5] != 1 || p[6] != 0 || be16(&p[7..]) == 0 || &p[9..13] != b"fLaC" {
            return unsupported();
        }
        let mut result = super::super::read_streaminfo(&mut io::Cursor::new(&p[9..]))?;
        result.format = "Ogg";
        result.provenance = "original_file_container_headers";
        Ok(result)
    } else {
        unsupported()
    }
}
fn variable(data: &[u8], pos: &mut usize) -> Result<u64, InfoError> {
    let mut value = 0u64;
    for _ in 0..9 {
        let byte = *data.get(*pos).ok_or(InfoError::CorruptHeader)?;
        *pos += 1;
        value = value
            .checked_mul(128)
            .and_then(|n| n.checked_add(u64::from(byte & 127)))
            .ok_or(InfoError::CorruptHeader)?;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    unsupported()
}
pub(super) fn mpc8<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<LocalFileInfo, InfoError> {
    let mut pos = 4;
    let mut result = None;
    for _ in 0..32 {
        let key = r.at::<2>(pos)?;
        if !key.iter().all(u8::is_ascii_uppercase) {
            return bad();
        }
        let mut bytes = [0u8; 9];
        let mut n = 0;
        loop {
            if n == 9 {
                return unsupported();
            }
            bytes[n] = r.at::<1>(pos + 2 + n as u64)?[0];
            n += 1;
            if bytes[n - 1] & 128 == 0 {
                break;
            }
        }
        let mut index = 0;
        let size = variable(&bytes[..n], &mut index)?;
        let head = 2 + n as u64;
        if size < head || pos.checked_add(size).filter(|end| *end <= r.len).is_none() {
            return bad();
        }
        let body = pos + head;
        if key == *b"SH" {
            if result.is_some() || size - head > 64 {
                return unsupported();
            }
            let h = r.blob(body, size - head)?;
            if h.len() < 9 || h[4] != 8 {
                return unsupported();
            }
            let mut i = 5;
            let count = variable(&h, &mut i)?;
            let skip = variable(&h, &mut i)?;
            if i + 2 != h.len() || skip > count {
                return bad();
            }
            let rate = *[44100, 48000, 37800, 32000]
                .get((h[i] >> 5) as usize)
                .ok_or(InfoError::CorruptHeader)?;
            let mut out = info(
                "Musepack SV8",
                "Musepack",
                "lossy",
                rate,
                (h[i + 1] >> 4) + 1,
                None,
            );
            if count != 0 {
                out.duration_seconds = Some((count - skip) as f64 / f64::from(rate));
            }
            result = Some(out);
        } else if key == *b"AP" {
            if size == head {
                return bad();
            }
            return result.ok_or(InfoError::CorruptHeader);
        } else if key == *b"SE" {
            return bad();
        }
        pos += size;
    }
    unsupported()
}
