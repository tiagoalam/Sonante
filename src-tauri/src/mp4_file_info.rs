//! Preflight the allocation-sensitive Lofty 0.25.4 MP4 reader and prove codec
//! from decoder config, not mp4a/extension. One unencrypted audio track only.
use super::*;
#[derive(Clone, Copy)]
struct Atom {
    kind: [u8; 4],
    body: u64,
    end: u64,
}
fn children<R: Read + Seek>(
    r: &mut Budget<'_, R>,
    start: u64,
    end: u64,
    count: &mut usize,
) -> Result<Vec<Atom>, InfoError> {
    let mut pos = start;
    let mut list = Vec::new();
    while pos < end {
        *count += 1;
        if *count > 256 {
            return unsupported();
        }
        if end - pos < 8 {
            return bad();
        }
        let h = r.at::<8>(pos)?;
        let n = u64::from(be32(&h));
        // Lofty's extended-atom paths differ in bounds/skip semantics. Keep
        // this inspection to ordinary 32-bit atoms, including mdat/free.
        if n == 0 || n == 1 {
            return unsupported();
        }
        if n < 8 || n > end - pos {
            return bad();
        }
        list.push(Atom {
            kind: h[4..].try_into().unwrap(),
            body: pos + 8,
            end: pos + n,
        });
        pos += n;
    }
    Ok(list)
}
fn unique(list: &[Atom], kind: [u8; 4]) -> Result<Atom, InfoError> {
    let mut matches = list.iter().filter(|a| a.kind == kind);
    let item = *matches.next().ok_or(InfoError::CorruptHeader)?;
    if matches.next().is_some() {
        return unsupported();
    }
    Ok(item)
}
fn descriptor(data: &[u8], at: usize, tag: u8) -> Result<&[u8], InfoError> {
    if data.get(at) != Some(&tag) {
        return unsupported();
    }
    let mut pos = at + 1;
    let mut size = 0usize;
    let mut done = false;
    for _ in 0..4 {
        let b = *data.get(pos).ok_or(InfoError::CorruptHeader)?;
        pos += 1;
        size = (size << 7) | usize::from(b & 127);
        if b & 128 == 0 {
            done = true;
            break;
        }
    }
    if !done {
        return unsupported();
    }
    data.get(pos..pos.checked_add(size).ok_or(InfoError::CorruptHeader)?)
        .ok_or(InfoError::CorruptHeader)
}
fn aac_config(data: &[u8]) -> Result<(u32, u8), InfoError> {
    // Common AAC-LC, including the standard sync extension with SBR absent.
    // No PCE, ER, implicit/explicit SBR or PS inference from an mp4a entry.
    if data.len() < 2 || data.len() > 5 {
        return unsupported();
    }
    let aot = data[0] >> 3;
    let idx = ((data[0] & 7) << 1) | (data[1] >> 7);
    let conf = (data[1] >> 3) & 15;
    if aot != 2 || idx > 12 || data[1] & 7 != 0 {
        return unsupported();
    }
    let ch = match conf {
        1..=6 => conf,
        7 => 8,
        _ => return unsupported(),
    };
    if data.len() > 2 && data[2..] != [0x56, 0xe5, 0x00] {
        return unsupported();
    }
    Ok((
        [
            96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
        ][idx as usize],
        ch,
    ))
}
fn stsd<R: Read + Seek>(r: &mut Budget<'_, R>, atom: Atom) -> Result<LocalFileInfo, InfoError> {
    if atom.end - atom.body > 4096 {
        return unsupported();
    }
    let b = r.blob(atom.body, atom.end - atom.body)?;
    if b.len() < 44 || be32(&b) != 0 || be32(&b[4..]) != 1 {
        return unsupported();
    }
    let e = &b[8..];
    if be32(e) as usize != e.len() || be16(&e[16..]) != 0 || be16(&e[14..]) != 1 {
        return unsupported();
    }
    let mut child = 36;
    let mut config = None;
    let mut child_count = 0;
    while child < e.len() {
        child_count += 1;
        if child_count > 16 {
            return unsupported();
        }
        if e.len() - child < 8 {
            return bad();
        }
        let size = be32(&e[child..]) as usize;
        if size < 8 || size > e.len() - child {
            return bad();
        }
        let kind = &e[child + 4..child + 8];
        if kind == b"esds" || kind == b"alac" {
            if config.is_some() {
                return unsupported();
            }
            config = Some((kind, &e[child + 8..child + size]));
        } else if kind != b"btrt" {
            return unsupported();
        } // sinf/DRM/wave/unknown variants
        child += size;
    }
    let (kind, c) = config.ok_or(InfoError::UnsupportedFormat)?;
    match &e[4..8] {
        b"mp4a" if kind == b"esds" => {
            if c.len() < 4 || be32(c) != 0 {
                return unsupported();
            }
            let es = descriptor(c, 4, 3)?;
            if es.len() < 3 || es[2] != 0 {
                return unsupported();
            }
            let decoder = descriptor(es, 3, 4)?;
            if decoder.len() < 13 || decoder[0] != 0x40 || decoder[1] != 0x15 {
                return unsupported();
            }
            let asc = descriptor(decoder, 13, 5)?;
            let (rate, ch) = aac_config(asc)?;
            Ok(info("MP4/M4A", "AAC", "lossy", rate, ch, None))
        }
        b"alac" if kind == b"alac" => {
            if c.len() != 28 || be32(c) != 0 || c[8] != 0 {
                return unsupported();
            }
            let depth = c[9];
            let channels = c[13];
            let rate = be32(&c[24..]);
            valid(rate, u16::from(channels))?;
            if ![8, 16, 20, 24, 32].contains(&depth) || be32(&c[4..]) == 0 {
                return bad();
            }
            Ok(info(
                "MP4/M4A",
                "ALAC",
                "lossless",
                rate,
                channels,
                Some(depth),
            ))
        }
        _ => unsupported(),
    }
}
pub(super) fn read<R: Read + Seek>(r: &mut Budget<'_, R>) -> Result<LocalFileInfo, InfoError> {
    let mut count = 0;
    let root = children(r, 0, r.len, &mut count)?;
    if root
        .first()
        .is_none_or(|a| a.kind != *b"ftyp" || a.end - a.body < 8)
    {
        return unsupported();
    }
    if root.iter().any(|a| a.kind == *b"moof") {
        return unsupported();
    }
    let ftyp = root[0];
    r.at::<4>(ftyp.body)?;
    let moov = unique(&root, *b"moov")?;
    unique(&root, *b"mdat")?;
    let list = children(r, moov.body, moov.end, &mut count)?;
    if list.iter().any(|a| a.kind == *b"mvex") {
        return unsupported();
    }
    let trak = unique(&list, *b"trak")?;
    let list = children(r, trak.body, trak.end, &mut count)?;
    let mdia = unique(&list, *b"mdia")?;
    let list = children(r, mdia.body, mdia.end, &mut count)?;
    let handler = unique(&list, *b"hdlr")?;
    if handler.end - handler.body < 12 {
        return bad();
    }
    if &r.at::<12>(handler.body)?[8..] != b"soun" {
        return unsupported();
    }
    let mdhd = unique(&list, *b"mdhd")?;
    let version = r.at::<1>(mdhd.body)?[0];
    // Restrict to v0: Lofty's v1 duration*1000 can overflow for untrusted u64.
    if version != 0 {
        return unsupported();
    }
    if mdhd.end - mdhd.body < 24 {
        return bad();
    }
    let h = r.at::<24>(mdhd.body)?;
    let scale = be32(&h[12..]);
    let duration = be32(&h[16..]);
    let minf = unique(&list, *b"minf")?;
    let list = children(r, minf.body, minf.end, &mut count)?;
    let stbl = unique(&list, *b"stbl")?;
    let list = children(r, stbl.body, stbl.end, &mut count)?;
    let sample = unique(&list, *b"stsd")?;
    for atom in list.iter().filter(|a| a.kind == *b"stts") {
        if atom.end - atom.body < 8 {
            return bad();
        }
        let h = r.at::<8>(atom.body)?;
        let n = be32(&h[4..]);
        if n > 128 {
            return unsupported();
        }
        if atom.end - atom.body != 8 + u64::from(n) * 8 {
            return bad();
        }
        r.blob(atom.body, atom.end - atom.body)?;
    }
    let mut out = stsd(r, sample)?;
    if out.codec == "ALAC" {
        // Lofty 0.25.4 hard-codes stsd length 80, rejecting common valid ALAC
        // entries with btrt. Config above is fully bounded and authoritative.
        if scale != 0 && duration != 0 && duration != u32::MAX {
            out.duration_seconds = Some(f64::from(duration) / f64::from(scale));
        }
        return Ok(out);
    }
    r.freeze();
    r.rewind()?;
    let file = lofty::mp4::Mp4File::read_from(r, options()).map_err(|e| r.map(e))?;
    let p = file.properties();
    if p.is_drm_protected() {
        return unsupported();
    }
    let expected = if out.codec == "ALAC" {
        lofty::mp4::Mp4Codec::ALAC
    } else {
        lofty::mp4::Mp4Codec::AAC
    };
    if p.codec() != Some(expected)
        || p.sample_rate() != out.sample_rate
        || p.channels() != Some(out.channels)
    {
        return bad();
    }
    if out.codec == "ALAC" && p.bit_depth() != out.bit_depth {
        return bad();
    }
    if scale != 0 && duration != 0 && duration != u32::MAX {
        out.duration_seconds = Some(f64::from(duration) / f64::from(scale));
    }
    Ok(out)
}
