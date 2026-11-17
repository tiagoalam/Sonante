//! Windows presentation thumbnail policy. No paths, filesystem or media IDs.
//! WIC replaces GTK only in this Windows adapter; UI still gets bounded PNG.
use super::super::super::protocol::Result;
#[cfg(windows)]
use base64::Engine;
pub(super) const LIMIT: usize = 512 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Png,
    Jpeg,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plan {
    kind: Kind,
    width: u32,
    height: u32,
    thumb_width: u32,
    thumb_height: u32,
}
fn plan(bytes: &[u8]) -> Option<Plan> {
    if bytes.is_empty() || bytes.len() > LIMIT {
        return None;
    }
    let (kind, width, height) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (end, w, h) = png(bytes)?;
        if end != bytes.len() {
            return None;
        }
        (Kind::Png, w, h)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        let mut at = 2usize;
        let (w, h) = loop {
            if *bytes.get(at)? != 0xff {
                return None;
            }
            while *bytes.get(at)? == 0xff {
                at = at.checked_add(1)?;
            }
            let marker = *bytes.get(at)?;
            at = at.checked_add(1)?;
            if marker == 0xda || marker == 0xd9 {
                return None;
            }
            let len =
                u16::from_be_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?) as usize;
            if len < 2 {
                return None;
            }
            if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) {
                if len < 8 {
                    return None;
                }
                let part = bytes.get(at..at.checked_add(len)?)?;
                break (
                    u16::from_be_bytes(part[5..7].try_into().ok()?) as u32,
                    u16::from_be_bytes(part[3..5].try_into().ok()?) as u32,
                );
            }
            at = at.checked_add(len)?;
        };
        (Kind::Jpeg, w, h)
    } else {
        return None;
    };
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return None;
    }
    let max = width.max(height);
    let (thumb_width, thumb_height) = if max <= 256 {
        (width, height)
    } else {
        ((width * 256 / max).max(1), (height * 256 / max).max(1))
    };
    Some(Plan {
        kind,
        width,
        height,
        thumb_width,
        thumb_height,
    })
}
// Structural bounds only. The OS codec must still decode the actual image;
// this is not a home-grown PNG/JPEG decoder or a claim of CRC verification.
fn png(bytes: &[u8]) -> Option<(usize, u32, u32)> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() > LIMIT {
        return None;
    }
    let mut at = 8usize;
    let mut size = None;
    let mut data = false;
    loop {
        let len = u32::from_be_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize;
        let tag = bytes.get(at + 4..at + 8)?;
        let end = at.checked_add(12)?.checked_add(len)?;
        if end > bytes.len() || tag == b"acTL" {
            return None;
        }
        if at == 8 {
            if tag != b"IHDR" || len != 13 {
                return None;
            }
            size = Some((
                u32::from_be_bytes(bytes.get(at + 8..at + 12)?.try_into().ok()?),
                u32::from_be_bytes(bytes.get(at + 12..at + 16)?.try_into().ok()?),
            ));
        } else if tag == b"IHDR" {
            return None;
        }
        if tag == b"IDAT" {
            data = true;
        }
        if tag == b"IEND" {
            if len != 0 || !data {
                return None;
            }
            let (w, h) = size?;
            return Some((end, w, h));
        }
        at = end;
    }
}
pub(super) fn thumbnail(bytes: &[u8]) -> Result<Option<String>> {
    let Some(plan) = plan(bytes) else {
        return Ok(None);
    };
    #[cfg(windows)]
    {
        let Some(output) = native::render(bytes, plan) else {
            return Ok(None);
        };
        return Ok(Some(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(output)
        )));
    }
    #[cfg(not(windows))]
    {
        let _ = plan;
        Err("windows_feature_unavailable")
    }
}
#[cfg(windows)]
#[path = "library_import_image_native.rs"]
mod native;

#[cfg(test)]
#[path = "library_import_image_tests.rs"]
mod tests;
