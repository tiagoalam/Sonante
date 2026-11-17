//! Bounded parser for external CUE sheets. It records only AUDIO tracks with
//! INDEX 01 and leaves physical path authorization to the secure scanner.

use std::path::{Component, Path, PathBuf};

pub(crate) const MAX_CUE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CueTrack {
    pub number: u32,
    pub file: PathBuf,
    pub start_frame: u64,
    pub end_frame: Option<u64>,
    pub title: Option<String>,
    pub performers: Vec<String>,
    pub album: Option<String>,
    pub album_performers: Vec<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub disc_number: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CueSheet {
    /// Every FILE reference is retained so the catalog can authorize all of
    /// them against the source walk, including files not used by an AUDIO
    /// track.
    pub files: Vec<PathBuf>,
    pub tracks: Vec<CueTrack>,
}

#[derive(Default)]
struct Tags {
    title: Option<String>,
    performers: Vec<String>,
    genre: Option<String>,
    date: Option<String>,
    disc_number: Option<u32>,
}

struct RawTrack {
    number: u32,
    file_index: usize,
    is_audio: bool,
    start_frame: Option<u64>,
    tags: Tags,
}

/// Parse UTF-8 CUE text. Callers must still bind every FILE reference to an
/// enumerated regular file in the same source before persisting the result.
pub(crate) fn parse(bytes: &[u8]) -> Result<CueSheet, String> {
    if bytes.len() > MAX_CUE_BYTES {
        return Err("CUE sheet exceeds the configured size limit".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "CUE sheet is not valid UTF-8")?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut files = Vec::<PathBuf>::new();
    let mut global = Tags::default();
    let mut tracks = Vec::<RawTrack>::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let (keyword, rest) = split_keyword(line)?;
        match keyword.to_ascii_uppercase().as_str() {
            "FILE" => {
                let reference = parse_file_value(rest)?;
                let path = safe_file_reference(&reference)?;
                files.push(path);
            }
            "TRACK" => {
                let mut fields = rest.split_whitespace();
                let number = fields
                    .next()
                    .ok_or("CUE TRACK is missing its number")?
                    .parse::<u32>()
                    .map_err(|_| "CUE TRACK number is invalid")?;
                if number == 0 || number > 99 || fields.next().is_none() || fields.next().is_some()
                {
                    return Err("CUE TRACK directive is invalid".into());
                }
                let mode = rest.split_whitespace().nth(1).unwrap().to_ascii_uppercase();
                let file_index = files
                    .len()
                    .checked_sub(1)
                    .ok_or("CUE TRACK has no preceding FILE")?;
                if tracks.last().is_some_and(|track| track.number >= number) {
                    return Err("CUE track numbers must be strictly increasing".into());
                }
                tracks.push(RawTrack {
                    number,
                    file_index,
                    is_audio: mode == "AUDIO",
                    start_frame: None,
                    tags: Tags::default(),
                });
            }
            "INDEX" => {
                let mut fields = rest.split_whitespace();
                let index = fields.next().ok_or("CUE INDEX is missing its number")?;
                let timestamp = fields.next().ok_or("CUE INDEX is missing its timestamp")?;
                if fields.next().is_some() {
                    return Err("CUE INDEX directive is invalid".into());
                }
                if index == "01" {
                    let track = tracks
                        .last_mut()
                        .ok_or("CUE INDEX has no preceding TRACK")?;
                    if track
                        .start_frame
                        .replace(parse_timestamp(timestamp)?)
                        .is_some()
                    {
                        return Err("CUE AUDIO track has duplicate INDEX 01".into());
                    }
                }
            }
            "TITLE" => {
                let value = parse_text_value(rest)?;
                if let Some(track) = tracks.last_mut() {
                    track.tags.title = Some(value);
                } else {
                    global.title = Some(value);
                }
            }
            "PERFORMER" => {
                let value = parse_text_value(rest)?;
                if let Some(track) = tracks.last_mut() {
                    push_unique(&mut track.tags.performers, value);
                } else {
                    push_unique(&mut global.performers, value);
                }
            }
            "REM" => parse_rem(
                rest,
                tracks
                    .last_mut()
                    .map(|track| &mut track.tags)
                    .unwrap_or(&mut global),
            )?,
            // The baseline MPD CUE plugin ignores the control directives below
            // for track identity; they do not alter INDEX 01 segment bounds.
            "CATALOG" | "FLAGS" | "ISRC" | "PREGAP" | "POSTGAP" => {}
            _ => return Err("CUE sheet contains an unsupported directive".into()),
        }
    }

    if files.is_empty() {
        return Err("CUE sheet contains no FILE directive".into());
    }

    let mut output = Vec::new();
    for (index, track) in tracks.iter().enumerate() {
        if !track.is_audio {
            continue;
        }
        let start = track
            .start_frame
            .ok_or("CUE AUDIO track is missing INDEX 01")?;
        let end = tracks[index + 1..]
            .iter()
            .find(|next| next.is_audio && next.file_index == track.file_index)
            .and_then(|next| next.start_frame);
        if end.is_some_and(|end| end <= start) {
            return Err("CUE segment has an empty or reversed time range".into());
        }
        let tags = &track.tags;
        output.push(CueTrack {
            number: track.number,
            file: files[track.file_index].clone(),
            start_frame: start,
            end_frame: end,
            title: tags.title.clone().or_else(|| global.title.clone()),
            performers: if tags.performers.is_empty() {
                global.performers.clone()
            } else {
                tags.performers.clone()
            },
            album: global.title.clone(),
            album_performers: global.performers.clone(),
            genre: tags.genre.clone().or_else(|| global.genre.clone()),
            date: tags.date.clone().or_else(|| global.date.clone()),
            disc_number: tags.disc_number.or(global.disc_number),
        });
    }
    Ok(CueSheet {
        files,
        tracks: output,
    })
}

fn split_keyword(line: &str) -> Result<(&str, &str), String> {
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    if end == 0 || end == line.len() {
        return Err("CUE directive is incomplete".into());
    }
    Ok((&line[..end], line[end..].trim()))
}

fn parse_file_value(rest: &str) -> Result<String, String> {
    if let Some(quoted) = rest.strip_prefix('"') {
        let close = quoted
            .find('"')
            .ok_or("CUE FILE has an unterminated quote")?;
        let name = &quoted[..close];
        let suffix = quoted[close + 1..].trim();
        if name.is_empty() || suffix.split_whitespace().count() != 1 {
            return Err("CUE FILE reference is ambiguous".into());
        }
        Ok(name.to_owned())
    } else {
        let mut fields = rest.split_whitespace();
        let name = fields.next().ok_or("CUE FILE is missing a name")?;
        if fields.next().is_none() || fields.next().is_some() {
            return Err("CUE FILE directive is invalid".into());
        }
        Ok(name.to_owned())
    }
}

fn parse_text_value(rest: &str) -> Result<String, String> {
    let value = if let Some(quoted) = rest.strip_prefix('"') {
        let close = quoted
            .find('"')
            .ok_or("CUE text value has an unterminated quote")?;
        if !quoted[close + 1..].trim().is_empty() {
            return Err("CUE text value has trailing data".into());
        }
        &quoted[..close]
    } else {
        rest.trim()
    };
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("CUE text value is empty or contains control characters".into());
    }
    Ok(value.to_owned())
}

fn safe_file_reference(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.contains(['\\', '\0', '\r', '\n', ':'])
        || value.contains("://")
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("CUE FILE reference is not a safe relative path".into());
    }
    Ok(path.to_path_buf())
}

/// Resolve a CUE FILE name relative to the directory containing the sheet.
/// The parser rejects parent components, so joining cannot escape the source;
/// this function also checks the sheet's own catalog-relative identity before
/// producing the source-relative path consumed by the secure scanner.
pub(crate) fn resolve_file_from_sheet(sheet: &Path, reference: &Path) -> Result<PathBuf, String> {
    if sheet.is_absolute()
        || !sheet
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        || reference.is_absolute()
        || !reference
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("CUE FILE reference is not a safe source-relative path".into());
    }
    let parent = sheet.parent().unwrap_or_else(|| Path::new(""));
    let resolved = parent.join(reference);
    if resolved.as_os_str().is_empty()
        || !resolved
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("CUE FILE reference escapes its source-relative directory".into());
    }
    Ok(resolved)
}

fn parse_timestamp(value: &str) -> Result<u64, String> {
    let mut fields = value.split(':');
    let minutes = fields
        .next()
        .ok_or("CUE timestamp is invalid")?
        .parse::<u64>()
        .map_err(|_| "CUE timestamp is invalid")?;
    let seconds = fields
        .next()
        .ok_or("CUE timestamp is invalid")?
        .parse::<u64>()
        .map_err(|_| "CUE timestamp is invalid")?;
    let frames = fields
        .next()
        .ok_or("CUE timestamp is invalid")?
        .parse::<u64>()
        .map_err(|_| "CUE timestamp is invalid")?;
    if fields.next().is_some() || seconds >= 60 || frames >= 75 {
        return Err("CUE timestamp is outside the CD frame range".into());
    }
    minutes
        .checked_mul(60 * 75)
        .and_then(|value| value.checked_add(seconds * 75))
        .and_then(|value| value.checked_add(frames))
        .ok_or_else(|| "CUE timestamp overflows".into())
}

fn parse_rem(rest: &str, tags: &mut Tags) -> Result<(), String> {
    let Some((key, value)) = rest.split_once(char::is_whitespace) else {
        return Err("CUE REM directive is incomplete".into());
    };
    let key = key.to_ascii_uppercase();
    match key.as_str() {
        "GENRE" => tags.genre = Some(parse_text_value(value.trim())?),
        "DATE" | "YEAR" => tags.date = Some(parse_text_value(value.trim())?),
        "DISCNUMBER" | "DISC" => {
            let value = parse_text_value(value.trim())?;
            tags.disc_number = Some(value.parse().map_err(|_| "CUE disc number is invalid")?);
        }
        _ => {}
    }
    Ok(())
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_image_multitrack_and_unicode_metadata() {
        let sheet = parse(b"\xef\xbb\xbfTITLE \"Cafe\xcc\x81\"\nPERFORMER \"Banda\"\nREM GENRE \"Jazz\"\nREM DATE 1999-03\nFILE \"imagem.wav\" WAVE\n TRACK 01 AUDIO\n  TITLE \"Faixa um\"\n  INDEX 01 00:00:00\n TRACK 02 AUDIO\n  TITLE \"Faixa dois\"\n  PERFORMER \"Convidada A\"\n  PERFORMER \"Convidada B\"\n  INDEX 01 00:02:10\n").unwrap();
        assert_eq!(sheet.tracks.len(), 2);
        assert_eq!(sheet.tracks[0].album.as_deref(), Some("Cafe\u{301}"));
        assert_eq!(sheet.tracks[0].performers, ["Banda"]);
        assert_eq!(sheet.tracks[0].end_frame, Some(160));
        assert_eq!(sheet.tracks[1].start_frame, 160);
        assert_eq!(sheet.tracks[1].performers, ["Convidada A", "Convidada B"]);
        assert_eq!(sheet.tracks[1].end_frame, None);
        assert_eq!(sheet.tracks[1].date.as_deref(), Some("1999-03"));
    }

    #[test]
    fn computes_ranges_independently_for_multiple_audio_files() {
        let sheet = parse(b"FILE first.flac FLAC\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 01:00:00\nFILE second.flac FLAC\nTRACK 03 AUDIO\nINDEX 01 00:05:00\n").unwrap();
        assert_eq!(
            sheet
                .tracks
                .iter()
                .map(|track| track.end_frame)
                .collect::<Vec<_>>(),
            [Some(4500), None, None]
        );
        assert_eq!(sheet.tracks[2].start_frame, 375);
        assert_eq!(sheet.tracks[2].file, PathBuf::from("second.flac"));
    }

    #[test]
    fn resolves_file_names_relative_to_the_cue_sheet_directory() {
        let sheet = Path::new("Artist/Album/disc.cue");
        assert_eq!(
            resolve_file_from_sheet(sheet, Path::new("disc.flac")).unwrap(),
            Path::new("Artist/Album/disc.flac")
        );
        assert_eq!(
            resolve_file_from_sheet(sheet, Path::new("Audio/disc.flac")).unwrap(),
            Path::new("Artist/Album/Audio/disc.flac")
        );
        assert!(resolve_file_from_sheet(sheet, Path::new("../outside.flac")).is_err());
        assert!(resolve_file_from_sheet(Path::new("../disc.cue"), Path::new("disc.flac")).is_err());
    }

    #[test]
    fn rejects_traversal_absolute_urls_bad_time_and_duplicate_indexes() {
        for value in [
            b"FILE ../outside.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n".as_slice(),
            b"FILE /outside.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n".as_slice(),
            b"FILE https://example.invalid/a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\n"
                .as_slice(),
            b"FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:60:00\n".as_slice(),
            b"FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nINDEX 01 00:01:00\n".as_slice(),
        ] {
            assert!(parse(value).is_err());
        }
    }

    #[test]
    fn rejects_out_of_order_or_empty_segment_boundaries() {
        assert!(parse(b"FILE a.wav WAVE\nTRACK 02 AUDIO\nINDEX 01 00:01:00\nTRACK 03 AUDIO\nINDEX 01 00:00:00\n").is_err());
        assert!(parse(b"FILE a.wav WAVE\nTRACK 01 AUDIO\nINDEX 01 00:01:00\nTRACK 02 AUDIO\nINDEX 01 00:01:00\n").is_err());
    }

    #[test]
    fn accepts_non_audio_tracks_without_exposing_them_as_virtual_audio() {
        let sheet = parse(b"FILE disc.bin BINARY\nTRACK 01 MODE1/2352\nINDEX 01 00:00:00\nTRACK 02 AUDIO\nINDEX 01 00:04:00\n").unwrap();
        assert_eq!(sheet.tracks.len(), 1);
        assert_eq!(sheet.tracks[0].number, 2);
    }

    #[test]
    fn data_track_does_not_hide_the_next_audio_segment_boundary() {
        let sheet = parse(b"FILE mixed.bin BINARY\nTRACK 01 AUDIO\nINDEX 01 00:00:00\nTRACK 02 MODE1/2352\nINDEX 01 00:02:00\nTRACK 03 AUDIO\nINDEX 01 00:04:00\n").unwrap();
        assert_eq!(sheet.tracks.len(), 2);
        assert_eq!(sheet.tracks[0].end_frame, Some(300));
        assert_eq!(sheet.tracks[1].start_frame, 300);
        assert_eq!(sheet.tracks[1].end_frame, None);
    }

    #[test]
    fn enforces_input_size_limit_and_utf8() {
        assert!(parse(&vec![b' '; MAX_CUE_BYTES + 1]).is_err());
        assert!(parse(b"\xff").is_err());
    }
}
