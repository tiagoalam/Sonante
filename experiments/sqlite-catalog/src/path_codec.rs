//! Native path persistence codecs used by the shared catalog core.
//!
//! The format tag is stored per source. Track paths inherit the format of their
//! source. Windows uses raw UTF-16LE code units (including unpaired surrogates);
//! Unix uses the native path bytes.

use std::path::{Component, Path, PathBuf};

pub(crate) const UNIX_BYTES_V1: i64 = 1;
pub(crate) const WINDOWS_UTF16LE_V1: i64 = 2;

pub(crate) fn native_format() -> Result<i64, String> {
    #[cfg(unix)]
    {
        Ok(UNIX_BYTES_V1)
    }
    #[cfg(windows)]
    {
        Ok(WINDOWS_UTF16LE_V1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("native path persistence is unsupported on this platform".into())
    }
}

pub(crate) fn is_known_format(format: i64) -> bool {
    matches!(format, UNIX_BYTES_V1 | WINDOWS_UTF16LE_V1)
}

pub(crate) fn encode_root(path: &Path) -> Result<Vec<u8>, String> {
    validate_root(path)?;
    encode_native(path)
}

pub(crate) fn decode_root(format: i64, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = decode_native(format, bytes)?;
    validate_root(&path)?;
    Ok(path)
}

fn validate_root(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || has_parent_or_current_component(path) {
        return Err("source root is not a normalized absolute path".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        if path.as_os_str().as_bytes().contains(&0) {
            return Err("source root contains a NUL byte".into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if path.as_os_str().encode_wide().any(|unit| unit == 0) {
            return Err("source root contains a NUL code unit".into());
        }
    }
    Ok(())
}

pub(crate) fn encode_relative(path: &Path) -> Result<Vec<u8>, String> {
    validate_relative(path)?;
    encode_native(path)
}

pub(crate) fn decode_relative(format: i64, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = decode_native(format, bytes)?;
    validate_relative(&path)?;
    Ok(path)
}

pub(crate) fn validate_relative(path: &Path) -> Result<(), String> {
    let mut count = 0usize;
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("stored relative path is empty or contains unsafe components".into());
        }
        count += 1;
    }
    if count == 0 {
        return Err("stored relative path is empty or contains unsafe components".into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let bytes = path.as_os_str().as_bytes();
        if bytes.contains(&0)
            || bytes
                .split(|byte| *byte == b'/')
                .any(|component| component.is_empty() || component == b"." || component == b"..")
        {
            return Err("stored relative path is empty or contains unsafe components".into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let units = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if units.contains(&0)
            || units.contains(&(b':' as u16))
            || units
                .split(|unit| *unit == b'/' as u16 || *unit == b'\\' as u16)
                .any(|component| {
                    component.is_empty()
                        || component == [b'.' as u16]
                        || component == [b'.' as u16, b'.' as u16]
                })
        {
            return Err("stored relative path is empty or contains unsafe components".into());
        }
    }
    Ok(())
}

fn has_parent_or_current_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

fn encode_native(path: &Path) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        Ok(path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err("native path persistence is unsupported on this platform".into())
    }
}

fn decode_native(format: i64, bytes: &[u8]) -> Result<PathBuf, String> {
    match format {
        UNIX_BYTES_V1 => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                return Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())));
            }
            #[cfg(not(unix))]
            return Err("catalog path format Unix-bytes-v1 is unsupported here".into());
        }
        WINDOWS_UTF16LE_V1 => {
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStringExt;
                if bytes.len() % 2 != 0 {
                    return Err("stored Windows path has an odd UTF-16LE byte count".into());
                }
                let units = bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>();
                return Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)));
            }
            #[cfg(not(windows))]
            return Err("catalog path format Windows-UTF16LE-v1 is unsupported here".into());
        }
        _ => Err(format!("unknown catalog path format {format}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_unicode_root_and_relative_path_round_trip() {
        let root = std::env::current_dir().unwrap().join("sonante-é-音楽");
        let relative = PathBuf::from("Álbum").join("曲.flac");
        assert_eq!(
            decode_root(native_format().unwrap(), &encode_root(&root).unwrap()).unwrap(),
            root
        );
        assert_eq!(
            decode_relative(
                native_format().unwrap(),
                &encode_relative(&relative).unwrap()
            )
            .unwrap(),
            relative
        );
    }

    #[test]
    fn relative_path_rejects_empty_absolute_and_parent_components() {
        assert!(encode_relative(Path::new("")).is_err());
        assert!(encode_relative(Path::new("/absolute/file.flac")).is_err());
        assert!(encode_relative(Path::new("album/../escape.flac")).is_err());
        assert!(decode_relative(native_format().unwrap(), b"../escape.flac").is_err());
        assert!(decode_relative(999, b"track.flac")
            .unwrap_err()
            .contains("unknown catalog path format"));
        #[cfg(unix)]
        {
            assert!(decode_relative(UNIX_BYTES_V1, b"album//track.flac").is_err());
            assert!(decode_relative(UNIX_BYTES_V1, b"album/./track.flac").is_err());
            assert!(encode_root(Path::new("/root\0invalid")).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_path_codec_preserves_invalid_utf8_and_replacement_character_distinctly() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let invalid_a = PathBuf::from(std::ffi::OsString::from_vec(b"bad-\xff.flac".to_vec()));
        let invalid_b = PathBuf::from(std::ffi::OsString::from_vec(b"bad-\xfe.flac".to_vec()));
        let replacement = PathBuf::from("bad-\u{fffd}.flac");
        let encoded_a = encode_relative(&invalid_a).unwrap();
        let encoded_b = encode_relative(&invalid_b).unwrap();
        let encoded_replacement = encode_relative(&replacement).unwrap();

        assert_ne!(encoded_a, encoded_b);
        assert_ne!(encoded_a, encoded_replacement);
        assert_eq!(
            decode_relative(UNIX_BYTES_V1, &encoded_a).unwrap(),
            invalid_a
        );
        assert_eq!(
            decode_relative(UNIX_BYTES_V1, &encoded_b).unwrap(),
            invalid_b
        );
        assert_eq!(
            decode_relative(UNIX_BYTES_V1, &encoded_replacement).unwrap(),
            replacement
        );
        assert_eq!(invalid_a.as_os_str().as_bytes(), b"bad-\xff.flac");
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_codec_preserves_raw_utf16_code_units() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let path = PathBuf::from(std::ffi::OsString::from_wide(&[
            b'C' as u16,
            b':' as u16,
            b'\\' as u16,
            0xd800,
            b'\\' as u16,
            b'x' as u16,
        ]));
        let encoded = encode_root(&path).unwrap();
        let decoded = decode_root(WINDOWS_UTF16LE_V1, &encoded).unwrap();
        assert_eq!(
            decoded.as_os_str().encode_wide().collect::<Vec<_>>(),
            path.as_os_str().encode_wide().collect::<Vec<_>>()
        );

        let relative = PathBuf::from(std::ffi::OsString::from_wide(&[
            0xdfff,
            b'.' as u16,
            b'f' as u16,
            b'l' as u16,
            b'a' as u16,
            b'c' as u16,
        ]));
        let encoded = encode_relative(&relative).unwrap();
        let decoded = decode_relative(WINDOWS_UTF16LE_V1, &encoded).unwrap();
        assert_eq!(
            decoded.as_os_str().encode_wide().collect::<Vec<_>>(),
            relative.as_os_str().encode_wide().collect::<Vec<_>>()
        );
        assert!(decode_root(WINDOWS_UTF16LE_V1, &[0]).is_err());
    }
}
