//! Native FLAC STREAMINFO inspection only; never decode audio, tags or pictures.
//! Layout: RFC 9639 §§8.1–8.2; cross-checked with Lofty 0.25.4 flac/properties.rs.
use serde::Serialize;
use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InfoError {
    InvalidLocalPath,
    OutsideConfiguredRoots,
    FileNotFound,
    CueVirtual,
    UnsupportedFormat,
    CorruptHeader,
    PermissionDenied,
    Unavailable,
    Stale,
    Busy,
}

#[derive(Debug, Serialize)]
pub struct LocalFileInfo {
    pub provenance: &'static str,
    pub format: &'static str,
    pub sample_rate: Option<u32>,
    pub bit_depth: u8,
    pub channels: u8,
    pub duration_seconds: Option<f64>,
    pub revision: String,
}

// Reject, rather than queue, repeated IPC requests. The permit belongs to the
// blocking worker: dropping the async caller does NOT release physical I/O.
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub struct InspectionPermit;
impl InspectionPermit {
    pub fn acquire() -> Result<Self, InfoError> {
        ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| InfoError::Busy)
    }
}
impl Drop for InspectionPermit {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

fn read_exact(reader: &mut impl Read, bytes: &mut [u8]) -> Result<(), InfoError> {
    reader.read_exact(bytes).map_err(|e| match e.kind() {
        io::ErrorKind::UnexpectedEof => InfoError::CorruptHeader,
        io::ErrorKind::PermissionDenied => InfoError::PermissionDenied,
        _ => InfoError::Unavailable,
    })
}

pub fn read_streaminfo(reader: &mut impl Read) -> Result<LocalFileInfo, InfoError> {
    let mut marker = [0; 4];
    read_exact(reader, &mut marker)?;
    if &marker != b"fLaC" {
        return Err(InfoError::UnsupportedFormat);
    }
    let mut header = [0; 4];
    read_exact(reader, &mut header)?;
    if header[0] & 0x7f != 0 || header[1..] != [0, 0, 34] {
        return Err(InfoError::CorruptHeader);
    }
    let mut info = [0; 34];
    read_exact(reader, &mut info)?;
    let min_block = u16::from_be_bytes([info[0], info[1]]);
    let max_block = u16::from_be_bytes([info[2], info[3]]);
    let frame_size =
        |offset| u32::from_be_bytes([0, info[offset], info[offset + 1], info[offset + 2]]);
    let min_frame = frame_size(4);
    let max_frame = frame_size(7);
    let packed = u64::from_be_bytes(info[10..18].try_into().unwrap());
    let sample_rate = (packed >> 44) as u32;
    let channels = ((packed >> 41) & 7) as u8 + 1;
    let bit_depth = ((packed >> 36) & 31) as u8 + 1;
    let samples = packed & 0xf_ffff_ffff;
    if min_block < 16
        || max_block < min_block
        || bit_depth < 4
        || (min_frame != 0 && max_frame != 0 && min_frame > max_frame)
    {
        return Err(InfoError::CorruptHeader);
    }
    Ok(LocalFileInfo {
        provenance: "original_file_streaminfo",
        format: "FLAC",
        sample_rate: (sample_rate != 0).then_some(sample_rate),
        bit_depth,
        channels,
        duration_seconds: (sample_rate != 0 && samples != 0)
            .then(|| samples as f64 / sample_rate as f64),
        revision: String::new(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;
    pub fn fixture(rate: u32, depth: u8, channels: u8, samples: u64) -> Vec<u8> {
        let mut b = b"fLaC\x80\0\0\x22".to_vec();
        let mut info = [0; 34];
        info[..4].copy_from_slice(&[0x10, 0, 0x10, 0]);
        let packed = ((rate as u64) << 44)
            | (((channels - 1) as u64) << 41)
            | (((depth - 1) as u64) << 36)
            | samples;
        info[10..18].copy_from_slice(&packed.to_be_bytes());
        b.extend(info);
        b
    }
    #[test]
    fn streaminfo_decodes_fields_and_unknown_duration_without_reading_tail() {
        for (rate, depth, channels) in [
            (44100, 16, 2),
            (96000, 24, 2),
            (96000, 16, 1),
            (192000, 32, 8),
        ] {
            let b = fixture(rate, depth, channels, rate as u64 * 2);
            let mut reader = Cursor::new([b, vec![255; 1024 * 1024]].concat());
            let result = read_streaminfo(&mut reader).unwrap();
            assert_eq!(
                (result.sample_rate, result.bit_depth, result.channels),
                (Some(rate), depth, channels)
            );
            assert_eq!(result.duration_seconds, Some(2.0));
            assert_eq!(reader.position(), 42);
        }
        let r = read_streaminfo(&mut Cursor::new(fixture(0, 24, 2, 0))).unwrap();
        assert_eq!(r.sample_rate, None);
        assert_eq!(r.duration_seconds, None);
        assert_eq!(
            read_streaminfo(&mut Cursor::new(fixture(96000, 24, 2, 0)))
                .unwrap()
                .duration_seconds,
            None
        );
    }
    #[test]
    fn malformed_and_truncated_headers_fail_without_unbounded_read_or_allocation() {
        let b = fixture(96000, 24, 2, 42);
        for n in 0..42 {
            assert_eq!(
                read_streaminfo(&mut Cursor::new(&b[..n])).unwrap_err(),
                InfoError::CorruptHeader
            );
        }
        for (offset, value) in [(4, 1), (5, 255), (7, 33), (8, 0)] {
            let mut bad = b.clone();
            bad[offset] = value;
            // offset 8 changes the minimum block size to zero; offset 20 changes depth to one.
            assert_eq!(
                read_streaminfo(&mut Cursor::new(bad)).unwrap_err(),
                InfoError::CorruptHeader
            );
        }
        let mut bad_depth = b.clone();
        bad_depth[20] &= 0xfe;
        bad_depth[21] &= 0x0f;
        assert_eq!(
            read_streaminfo(&mut Cursor::new(bad_depth)).unwrap_err(),
            InfoError::CorruptHeader
        );
        assert_eq!(
            read_streaminfo(&mut Cursor::new(b"RIFF")).unwrap_err(),
            InfoError::UnsupportedFormat
        );
        struct Denied;
        impl Read for Denied {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        }
        assert_eq!(
            read_streaminfo(&mut Denied).unwrap_err(),
            InfoError::PermissionDenied
        );
    }
    #[test]
    fn permit_rejects_overlapping_work_until_worker_drops_it() {
        let permit = InspectionPermit::acquire().unwrap();
        assert!(matches!(InspectionPermit::acquire(), Err(InfoError::Busy)));
        drop(permit);
        assert!(InspectionPermit::acquire().is_ok());
    }
}
