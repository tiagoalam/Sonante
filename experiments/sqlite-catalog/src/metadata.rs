//! WAV PCM, AIFF PCM, FLAC, MP3, WavPack, Ogg Vorbis/Opus, MP4 AAC/ALAC, DSF, and DFF metadata extraction. Files are opened by the secure
//! scanner before they reach this module; this module never opens paths itself.

use lofty::{
    aac::AacFile,
    ape::ApeFile,
    config::{ParseOptions, ParsingMode},
    file::{AudioFile, FileType, TaggedFile, TaggedFileExt},
    flac::FlacFile,
    iff::aiff::AiffFile,
    iff::wav::{WavFile, WavFormat},
    mp4::{Mp4Codec, Mp4File},
    mpeg::{Layer, MpegFile},
    probe::Probe,
    tag::{Accessor, ItemKey, Tag, TagType},
    wavpack::WavPackFile,
};
use std::{
    io::{Read, Seek, SeekFrom},
    time::Instant,
};

pub(crate) const PARSER_VERSION: i64 = 2;
pub(crate) const MP3_PARSER_VERSION: i64 = 4;
pub(crate) const OGG_PARSER_VERSION: i64 = 1;
pub(crate) const AIFF_PARSER_VERSION: i64 = 2;
pub(crate) const MP4_PARSER_VERSION: i64 = 1;
pub(crate) const DSF_PARSER_VERSION: i64 = 3;
pub(crate) const DFF_PARSER_VERSION: i64 = 3;
// Keep versions scoped so changed date/artist interpretation retries only the
// parser families whose persisted metadata can change.
pub(crate) const AAC_PARSER_VERSION: i64 = 4;
pub(crate) const APE_PARSER_VERSION: i64 = 4;
pub(crate) const AIFC_PARSER_VERSION: i64 = 4;
pub(crate) const WAVPACK_PARSER_VERSION: i64 = 3;

const MAX_ID3_BYTES: usize = 4 * 1024 * 1024;
const MAX_APE_TAG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_AIFF_TEXT_BYTES: u32 = 64 * 1024;
const MAX_AIFF_CHUNKS: usize = 4096;
const MAX_AAC_LEADING_PADDING: u64 = 1024;
const MAX_DFF_CHUNKS: usize = 4096;
const MAX_DFF_TEXT_BYTES: usize = 64 * 1024;

/// Keep parser versions scoped to the format family whose extraction rules changed.
/// Adding one family must not invalidate successful metadata for the others.
pub(crate) fn parser_version_for_extension(extension: &str) -> i64 {
    if extension.eq_ignore_ascii_case("mp3") {
        MP3_PARSER_VERSION
    } else if matches!(
        extension.to_ascii_lowercase().as_str(),
        "ogg" | "oga" | "opus"
    ) {
        OGG_PARSER_VERSION
    } else if matches!(extension.to_ascii_lowercase().as_str(), "aif" | "aiff") {
        AIFF_PARSER_VERSION
    } else if matches!(extension.to_ascii_lowercase().as_str(), "m4a" | "mp4") {
        MP4_PARSER_VERSION
    } else if extension.eq_ignore_ascii_case("dsf") {
        DSF_PARSER_VERSION
    } else if extension.eq_ignore_ascii_case("dff") {
        DFF_PARSER_VERSION
    } else if extension.eq_ignore_ascii_case("aac") {
        AAC_PARSER_VERSION
    } else if extension.eq_ignore_ascii_case("ape") {
        APE_PARSER_VERSION
    } else if extension.eq_ignore_ascii_case("aifc") {
        AIFC_PARSER_VERSION
    } else if extension.eq_ignore_ascii_case("wv") {
        WAVPACK_PARSER_VERSION
    } else {
        PARSER_VERSION
    }
}

/// Read DSF's optional ID3v2 tag using the metadata pointer in its 28-byte
/// header. Audio data is skipped by seeking; only structural headers and the
/// capped tag payload are read.
pub(crate) fn parse_dsf<R: Read + Seek>(mut reader: R) -> Attempt {
    parse_dsf_inner(&mut reader).unwrap_or_else(|error| parse_failure("dsf", error))
}

fn parse_dsf_inner<R: Read + Seek>(reader: &mut R) -> Result<Attempt, String> {
    let started = Instant::now();
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_| "dsf_size_unavailable".to_owned())?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| "dsf_header_unavailable".to_owned())?;
    let mut header = [0u8; 28];
    reader
        .read_exact(&mut header)
        .map_err(|_| "dsf_header_truncated".to_owned())?;
    if &header[..4] != b"DSD " || u64::from_le_bytes(header[4..12].try_into().unwrap()) != 28 {
        return Err("dsf_header_invalid".to_owned());
    }
    let declared_len = u64::from_le_bytes(header[12..20].try_into().unwrap());
    let metadata_offset = u64::from_le_bytes(header[20..28].try_into().unwrap());
    if declared_len != file_len {
        return Err("dsf_file_size_mismatch".to_owned());
    }

    // DSF stores its format and data chunk headers before the audio payload.
    // Validate their boundaries without reading any audio bytes.
    let mut format_header = [0u8; 12];
    reader
        .read_exact(&mut format_header)
        .map_err(|_| "dsf_format_chunk_truncated".to_owned())?;
    if &format_header[..4] != b"fmt " {
        return Err("dsf_format_chunk_invalid".to_owned());
    }
    let format_len = u64::from_le_bytes(format_header[4..12].try_into().unwrap());
    if format_len != 52
        || 28u64
            .checked_add(format_len)
            .is_none_or(|end| end > file_len)
    {
        return Err("dsf_format_chunk_bounds".to_owned());
    }
    reader
        .seek(SeekFrom::Start(28 + format_len))
        .map_err(|_| "dsf_data_chunk_unavailable".to_owned())?;
    let mut data_header = [0u8; 12];
    reader
        .read_exact(&mut data_header)
        .map_err(|_| "dsf_data_chunk_truncated".to_owned())?;
    if &data_header[..4] != b"data" {
        return Err("dsf_data_chunk_invalid".to_owned());
    }
    let data_len = u64::from_le_bytes(data_header[4..12].try_into().unwrap());
    let data_end = (28u64 + format_len)
        .checked_add(data_len)
        .filter(|end| data_len >= 12 && *end <= file_len)
        .ok_or_else(|| "dsf_data_chunk_bounds".to_owned())?;

    let values = if metadata_offset == 0 {
        Values::default()
    } else {
        if metadata_offset < data_end || metadata_offset >= file_len {
            return Err("dsf_metadata_offset_invalid".to_owned());
        }
        let bytes = read_id3v2_bytes_at(reader, metadata_offset, file_len)?;
        let mut values = parse_id3v2_bytes(&bytes)?
            .as_ref()
            .map(values_from_tag)
            .unwrap_or_default();
        if let Some(date) = raw_id3_date(&bytes) {
            values.date_text = Some(date);
        }
        values
    };
    Ok(Attempt {
        status: if has_text_metadata(&values) {
            AttemptStatus::ParsedWithTags
        } else {
            AttemptStatus::ParsedWithoutTags
        },
        format: "dsf".to_owned(),
        values: Some(values),
        error: None,
        duration_us: started.elapsed().as_micros(),
    })
}

/// Read DSDIFF (DFF) metadata. Chunk lengths are big-endian and audio/unknown
/// chunks are skipped; only DIIN text and bounded ID3 chunks are materialized.
pub(crate) fn parse_dff<R: Read + Seek>(mut reader: R) -> Attempt {
    parse_dff_inner(&mut reader).unwrap_or_else(|error| parse_failure("dff", error))
}

fn parse_dff_inner<R: Read + Seek>(reader: &mut R) -> Result<Attempt, String> {
    let started = Instant::now();
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_| "dff_size_unavailable".to_owned())?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| "dff_header_unavailable".to_owned())?;
    let mut header = [0u8; 16];
    reader
        .read_exact(&mut header)
        .map_err(|_| "dff_header_truncated".to_owned())?;
    if &header[..4] != b"FRM8" || &header[12..16] != b"DSD " {
        return Err("dff_header_invalid".to_owned());
    }
    let form_size = u64::from_be_bytes(header[4..12].try_into().unwrap());
    let form_end = 12u64
        .checked_add(form_size)
        .filter(|end| form_size >= 4 && *end == file_len)
        .ok_or_else(|| "dff_form_bounds".to_owned())?;

    let mut values = Values::default();
    let mut chunks = 0usize;
    let mut saw_version = false;
    let mut saw_properties = false;
    let mut saw_audio = false;
    let mut saw_diinfo = false;
    let mut saw_id3 = false;
    while reader
        .stream_position()
        .map_err(|_| "dff_position_failed".to_owned())?
        < form_end
    {
        chunks += 1;
        if chunks > MAX_DFF_CHUNKS {
            return Err("dff_chunk_count_limit".to_owned());
        }
        let (id, size, data_start, data_end, next) = read_dff_chunk_header(reader, form_end)?;
        match &id {
            b"FVER" => {
                if saw_version || size != 4 {
                    return Err("dff_version_chunk_invalid".to_owned());
                }
                saw_version = true;
            }
            b"PROP" => {
                if saw_properties || size < 4 {
                    return Err("dff_properties_chunk_invalid".to_owned());
                }
                let mut property_type = [0u8; 4];
                reader
                    .read_exact(&mut property_type)
                    .map_err(|_| "dff_properties_chunk_truncated".to_owned())?;
                if &property_type != b"SND " {
                    return Err("dff_properties_type_invalid".to_owned());
                }
                saw_properties = true;
            }
            b"DSD " | b"DST " => saw_audio = true,
            b"DIIN" => {
                if saw_diinfo {
                    return Err("dff_duplicate_diinfo".to_owned());
                }
                saw_diinfo = true;
                parse_dff_diinfo(reader, data_start, data_end, &mut values)?;
            }
            b"ID3 " => {
                if saw_id3 {
                    return Err("dff_duplicate_id3".to_owned());
                }
                saw_id3 = true;
                let id3_len = usize::try_from(size)
                    .ok()
                    .filter(|len| *len <= MAX_ID3_BYTES)
                    .ok_or_else(|| "dff_id3_size_limit".to_owned())?;
                reader
                    .seek(SeekFrom::Start(data_start))
                    .map_err(|_| "dff_id3_seek_failed".to_owned())?;
                let mut bytes = vec![0; id3_len];
                reader
                    .read_exact(&mut bytes)
                    .map_err(|_| "dff_id3_truncated".to_owned())?;
                if let Some(tag) = parse_id3v2_bytes(&bytes)? {
                    merge_tag_values(&mut values, &tag);
                }
                if let Some(date) = raw_id3_date(&bytes) {
                    values.date_text = Some(date);
                }
            }
            _ => {}
        }
        reader
            .seek(SeekFrom::Start(next))
            .map_err(|_| "dff_chunk_seek_failed".to_owned())?;
    }
    if !saw_version || !saw_properties || !saw_audio {
        return Err("dff_required_chunks_missing".to_owned());
    }
    Ok(Attempt {
        status: if has_text_metadata(&values) {
            AttemptStatus::ParsedWithTags
        } else {
            AttemptStatus::ParsedWithoutTags
        },
        format: "dff".to_owned(),
        values: Some(values),
        error: None,
        duration_us: started.elapsed().as_micros(),
    })
}

fn read_dff_chunk_header<R: Read + Seek>(
    reader: &mut R,
    parent_end: u64,
) -> Result<([u8; 4], u64, u64, u64, u64), String> {
    let start = reader
        .stream_position()
        .map_err(|_| "dff_position_failed".to_owned())?;
    let mut header = [0u8; 12];
    reader
        .read_exact(&mut header)
        .map_err(|_| "dff_chunk_header_truncated".to_owned())?;
    let id = header[..4].try_into().unwrap();
    let size = u64::from_be_bytes(header[4..12].try_into().unwrap());
    let data_start = start
        .checked_add(12)
        .ok_or_else(|| "dff_chunk_bounds".to_owned())?;
    let data_end = data_start
        .checked_add(size)
        .filter(|end| *end <= parent_end)
        .ok_or_else(|| "dff_chunk_bounds".to_owned())?;
    let next = data_end
        .checked_add(size & 1)
        .filter(|end| *end <= parent_end)
        .ok_or_else(|| "dff_chunk_padding_bounds".to_owned())?;
    Ok((id, size, data_start, data_end, next))
}

fn parse_dff_diinfo<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    values: &mut Values,
) -> Result<(), String> {
    let mut position = start;
    let mut count = 0usize;
    let mut saw_artist = false;
    let mut saw_title = false;
    while position < end {
        count += 1;
        if count > 128 {
            return Err("dff_diinfo_chunk_count_limit".to_owned());
        }
        reader
            .seek(SeekFrom::Start(position))
            .map_err(|_| "dff_diinfo_seek_failed".to_owned())?;
        let (id, size, text_start, text_end, next) = read_dff_chunk_header(reader, end)?;
        if id == *b"DIAR" || id == *b"DITI" {
            if size < 4 || size > MAX_DFF_TEXT_BYTES as u64 + 4 {
                return Err("dff_diinfo_text_size".to_owned());
            }
            let mut length_bytes = [0u8; 4];
            reader
                .read_exact(&mut length_bytes)
                .map_err(|_| "dff_diinfo_text_truncated".to_owned())?;
            let text_len = u32::from_be_bytes(length_bytes) as usize;
            if text_len > MAX_DFF_TEXT_BYTES || text_start + 4 + text_len as u64 != text_end {
                return Err("dff_diinfo_text_bounds".to_owned());
            }
            let mut bytes = vec![0; text_len];
            reader
                .read_exact(&mut bytes)
                .map_err(|_| "dff_diinfo_text_truncated".to_owned())?;
            let text_value =
                std::str::from_utf8(&bytes).map_err(|_| "dff_diinfo_text_encoding".to_owned())?;
            if text_value.chars().any(char::is_control) {
                return Err("dff_diinfo_text_control".to_owned());
            }
            let value = text(Some(text_value));
            if id == *b"DIAR" {
                if saw_artist {
                    return Err("dff_duplicate_artist".to_owned());
                }
                saw_artist = true;
                values.track_artists.extend(value);
            } else {
                if saw_title {
                    return Err("dff_duplicate_title".to_owned());
                }
                saw_title = true;
                values.title = value;
            }
        }
        position = next;
    }
    Ok(())
}

fn read_id3v2_bytes_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    file_len: u64,
) -> Result<Vec<u8>, String> {
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|_| "id3_seek_failed".to_owned())?;
    let mut header = [0u8; 10];
    reader
        .read_exact(&mut header)
        .map_err(|_| "id3_header_truncated".to_owned())?;
    if &header[..3] != b"ID3" || !(2..=4).contains(&header[3]) || header[4] == 0xff {
        return Err("id3_header_invalid".to_owned());
    }
    let size = synchsafe_size(&header[6..10])?;
    let footer = header[3] == 4 && header[5] & 0x10 != 0;
    let total = 10usize
        .checked_add(size)
        .and_then(|value| value.checked_add(if footer { 10 } else { 0 }))
        .filter(|value| *value <= MAX_ID3_BYTES)
        .ok_or_else(|| "id3_size_limit".to_owned())?;
    if offset
        .checked_add(total as u64)
        .is_none_or(|end| end > file_len)
    {
        return Err("id3_tag_truncated".to_owned());
    }
    let mut bytes = Vec::with_capacity(total);
    bytes.extend_from_slice(&header);
    bytes.resize(total, 0);
    reader
        .read_exact(&mut bytes[10..])
        .map_err(|_| "id3_tag_truncated".to_owned())?;
    Ok(bytes)
}

fn synchsafe_size(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() != 4 || bytes.iter().any(|byte| byte & 0x80 != 0) {
        return Err("id3_size_invalid".to_owned());
    }
    Ok(bytes
        .iter()
        .fold(0usize, |size, byte| (size << 7) | usize::from(*byte)))
}

/// Reuse Lofty's ID3 implementation by wrapping the already-capped tag in a
/// tiny synthetic MPEG stream. No supplied audio bytes are copied or decoded.
fn parse_id3v2_bytes(bytes: &[u8]) -> Result<Option<Tag>, String> {
    if bytes.len() < 10 || &bytes[..3] != b"ID3" || !(2..=4).contains(&bytes[3]) || bytes[4] == 0xff
    {
        return Err("id3_header_invalid".to_owned());
    }
    let payload_len = synchsafe_size(&bytes[6..10])?;
    let footer = bytes[3] == 4 && bytes[5] & 0x10 != 0;
    let expected = 10usize
        .checked_add(payload_len)
        .and_then(|size| size.checked_add(if footer { 10 } else { 0 }))
        .ok_or_else(|| "id3_size_invalid".to_owned())?;
    if expected != bytes.len() || expected > MAX_ID3_BYTES {
        return Err("id3_size_mismatch".to_owned());
    }
    let frame_size = 417usize;
    let mut stream = Vec::with_capacity(bytes.len() + frame_size * 2);
    stream.extend_from_slice(bytes);
    let frame = [0xff, 0xfb, 0x90, 0x64]; // MPEG-1 Layer III, 128 kbps, 44.1 kHz
    let mut first = vec![0; frame_size];
    first[..4].copy_from_slice(&frame);
    let mut second = vec![0; frame_size];
    second[..4].copy_from_slice(&frame);
    stream.extend_from_slice(&first);
    stream.extend_from_slice(&second);
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = MpegFile::read_from(&mut std::io::Cursor::new(stream), options)
        .map_err(|_| "id3_parse_failed".to_owned())?;
    let tagged: TaggedFile = parsed.into();
    Ok(tagged.tag(TagType::Id3v2).cloned())
}

/// DSD tags may carry legacy ID3 dates that Lofty does not combine. Preserve a
/// literal TDRC, or combine TYER+TDAT only when the DDMM calendar date is valid.
fn raw_id3_date(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 10 || &bytes[..3] != b"ID3" || !(2..=4).contains(&bytes[3]) {
        return None;
    }
    let version = bytes[3];
    let flags = bytes[5];
    // Extended headers and unsynchronization need version-specific transforms;
    // do not guess at a raw fallback when either is present.
    if flags & 0xc0 != 0 {
        return None;
    }
    let declared_size = synchsafe_size(&bytes[6..10]).ok()?;
    let end = 10usize.checked_add(declared_size)?;
    if end > bytes.len() {
        return None;
    }
    let header_len = if version == 2 { 6 } else { 10 };
    let mut position = 10usize;
    let mut tdrc = None;
    let mut year = None;
    let mut day_month = None;
    while position.checked_add(header_len)? <= end {
        let id_len = if version == 2 { 3 } else { 4 };
        let id = &bytes[position..position + id_len];
        if id.iter().all(|byte| *byte == 0) {
            break;
        }
        if !id.iter().all(u8::is_ascii_alphanumeric) {
            return None;
        }
        let frame_size = match version {
            2 => {
                (usize::from(bytes[position + 3]) << 16)
                    | (usize::from(bytes[position + 4]) << 8)
                    | usize::from(bytes[position + 5])
            }
            3 => u32::from_be_bytes(bytes[position + 4..position + 8].try_into().ok()?) as usize,
            _ => synchsafe_size(&bytes[position + 4..position + 8]).ok()?,
        };
        let data_start = position.checked_add(header_len)?;
        let data_end = data_start.checked_add(frame_size)?;
        if data_end > end {
            return None;
        }
        let frame = match version {
            2 if id == b"TYE" => Some(0),
            3 | 4 if id == b"TDRC" => Some(0),
            3 | 4 if id == b"TYER" => Some(1),
            3 | 4 if id == b"TDAT" => Some(2),
            _ => None,
        };
        let has_frame_flags = version > 2
            && bytes[position + 8..position + 10]
                .iter()
                .any(|flag| *flag != 0);
        if let Some(frame) = frame.filter(|_| !has_frame_flags && frame_size <= MAX_DFF_TEXT_BYTES)
        {
            if let Some(value) = decode_id3_text(&bytes[data_start..data_end]) {
                let value = value.trim();
                if !value.is_empty() && !value.chars().any(char::is_control) {
                    match frame {
                        0 => tdrc = Some(value.to_owned()),
                        1 => year = Some(value.to_owned()),
                        2 => day_month = Some(value.to_owned()),
                        _ => unreachable!(),
                    }
                }
            }
        }
        position = data_end;
    }
    if let Some(tdrc) = tdrc {
        return Some(tdrc);
    }
    let year = year?;
    if year.len() != 4 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let year_number = year.parse::<u32>().ok().filter(|value| *value > 0)?;
    let Some(day_month) = day_month else {
        return Some(year);
    };
    if day_month.len() != 4 || !day_month.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(year);
    }
    let day = day_month[..2].parse::<u32>().ok()?;
    let month = day_month[2..].parse::<u32>().ok()?;
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year_number) => 29,
        2 => 28,
        _ => return Some(year),
    };
    if day == 0 || day > max_day {
        return Some(year);
    }
    Some(format!("{year}-{month:02}-{day:02}"))
}

fn raw_id3v2_date_at<R: Read + Seek>(reader: &mut R, offset: u64, file_len: u64) -> Option<String> {
    if offset.checked_add(3).is_none_or(|end| end > file_len) {
        return None;
    }
    reader.seek(SeekFrom::Start(offset)).ok()?;
    let mut signature = [0; 3];
    reader.read_exact(&mut signature).ok()?;
    if &signature != b"ID3" {
        return None;
    }
    let bytes = read_id3v2_bytes_at(reader, offset, file_len).ok()?;
    raw_id3_date(&bytes)
}

fn raw_id3v2_prefix_date<R: Read + Seek>(
    reader: &mut R,
    allow_zero_padding: bool,
) -> Option<String> {
    let file_len = reader.seek(SeekFrom::End(0)).ok()?;
    let mut offset = 0u64;
    if allow_zero_padding {
        reader.seek(SeekFrom::Start(0)).ok()?;
        while offset < MAX_AAC_LEADING_PADDING {
            let mut byte = [0];
            reader.read_exact(&mut byte).ok()?;
            if byte[0] != 0 {
                break;
            }
            offset += 1;
        }
    }
    raw_id3v2_date_at(reader, offset, file_len)
}

fn raw_id3v2_date_in_iff<R: Read + Seek>(reader: &mut R, form_type: &[u8; 4]) -> Option<String> {
    let file_len = reader.seek(SeekFrom::End(0)).ok()?;
    if file_len < 12 {
        return None;
    }
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut form = [0; 12];
    reader.read_exact(&mut form).ok()?;
    let form_size = u32::from_be_bytes(form[4..8].try_into().ok()?) as u64;
    if &form[..4] != b"FORM" || &form[8..12] != form_type || form_size.checked_add(8)? != file_len {
        return None;
    }

    let mut position = 12u64;
    let mut chunks = 0usize;
    let mut id3_total = 0u64;
    while position < file_len {
        chunks += 1;
        if chunks > MAX_AIFF_CHUNKS {
            return None;
        }
        reader.seek(SeekFrom::Start(position)).ok()?;
        let mut header = [0; 8];
        reader.read_exact(&mut header).ok()?;
        let size = u32::from_be_bytes(header[4..8].try_into().ok()?) as u64;
        let data_start = position.checked_add(8)?;
        let data_end = data_start.checked_add(size)?;
        let next = data_end.checked_add(size & 1)?;
        if next > file_len {
            return None;
        }
        if &header[..4] == b"ID3 " || &header[..4] == b"id3 " {
            id3_total = id3_total.checked_add(size)?;
            if id3_total > MAX_ID3_BYTES as u64 {
                return None;
            }
            if let Some(date) = raw_id3v2_date_at(reader, data_start, data_end) {
                return Some(date);
            }
        }
        position = next;
    }
    None
}

fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn decode_id3_text(bytes: &[u8]) -> Option<String> {
    let (&encoding, payload) = bytes.split_first()?;
    match encoding {
        0 => Some(
            payload
                .split(|byte| *byte == 0)
                .next()?
                .iter()
                .map(|byte| char::from(*byte))
                .collect(),
        ),
        3 => std::str::from_utf8(payload.split(|byte| *byte == 0).next()?)
            .ok()
            .map(str::to_owned),
        1 | 2 => {
            let mut payload = payload;
            if payload.len() % 2 != 0 {
                return None;
            }
            let little_endian = encoding == 1 && payload.starts_with(&[0xff, 0xfe]);
            if encoding == 1
                && (payload.starts_with(&[0xff, 0xfe]) || payload.starts_with(&[0xfe, 0xff]))
            {
                payload = &payload[2..];
            }
            let words = payload.chunks_exact(2).map(|pair| {
                if little_endian {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            });
            let mut decoded = String::new();
            for character in char::decode_utf16(words) {
                let character = character.ok()?;
                if character == '\0' {
                    break;
                }
                decoded.push(character);
            }
            Some(decoded)
        }
        _ => None,
    }
}

fn values_from_tag(tag: &Tag) -> Values {
    let mut values = Values::default();
    merge_tag_values(&mut values, tag);
    values
}

fn merge_tag_values(values: &mut Values, tag: &Tag) {
    values.title = text(tag.get_string(ItemKey::TrackTitle)).or_else(|| values.title.take());
    let track_artists = strings(tag, ItemKey::TrackArtist)
        .into_iter()
        .chain(strings(tag, ItemKey::TrackArtists))
        .collect::<Vec<_>>();
    if !track_artists.is_empty() {
        values.track_artists = track_artists;
    }
    let album_artists = strings(tag, ItemKey::AlbumArtist)
        .into_iter()
        .chain(strings(tag, ItemKey::AlbumArtists))
        .collect::<Vec<_>>();
    if !album_artists.is_empty() {
        values.album_artists = album_artists;
    }
    values.album = text(tag.get_string(ItemKey::AlbumTitle)).or_else(|| values.album.take());
    values.genre = text(tag.get_string(ItemKey::Genre)).or_else(|| values.genre.take());
    values.date_text =
        text(tag.get_string(ItemKey::RecordingDate)).or_else(|| values.date_text.take());
    values.track_number = positive_u32(tag.track()).or(values.track_number);
    values.disc_number = positive_u32(tag.disk()).or(values.disc_number);
}

fn merge_aiff_tags(values: &mut Values, tagged: &TaggedFile) {
    let id3 = tagged.tag(TagType::Id3v2);
    let text_chunks = tagged.tag(TagType::AiffText);
    if let Some(tag) = id3 {
        merge_tag_values(values, tag);
    }
    values.title = values
        .title
        .take()
        .or_else(|| text_chunks.and_then(|tag| text(tag.get_string(ItemKey::TrackTitle))));
    if values.track_artists.is_empty() {
        values.track_artists = text_chunks
            .and_then(|tag| strings(tag, ItemKey::TrackArtist).into_iter().next())
            .into_iter()
            .collect();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum AttemptStatus {
    ParsedWithTags = 1,
    ParsedWithoutTags = 2,
    Unsupported = 3,
    Failed = 4,
}

impl AttemptStatus {
    pub(crate) fn from_i64(value: i64) -> Option<Self> {
        match value {
            1 => Some(Self::ParsedWithTags),
            2 => Some(Self::ParsedWithoutTags),
            3 => Some(Self::Unsupported),
            4 => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Values {
    pub title: Option<String>,
    pub track_artists: Vec<String>,
    pub album_artists: Vec<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub date_text: Option<String>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub duration_ms: Option<i64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attempt {
    pub(crate) status: AttemptStatus,
    pub(crate) format: String,
    /// Present only when a supported file was parsed successfully. Technical
    /// properties are retained even when all supported text fields are absent.
    pub(crate) values: Option<Values>,
    pub(crate) error: Option<String>,
    pub(crate) duration_us: u128,
}

pub(crate) fn unsupported(extension: &str) -> Attempt {
    Attempt {
        status: AttemptStatus::Unsupported,
        format: format!("extension:{}", extension.to_ascii_lowercase()),
        values: None,
        error: None,
        duration_us: 0,
    }
}

/// Record a readable-handle failure as an unsuccessful metadata attempt while
/// keeping the physically enumerated file in the catalog. Callers must not use
/// this for path, identity, symlink, or directory-integrity failures.
pub(crate) fn failed(extension: &str, error: impl AsRef<str>) -> Attempt {
    Attempt {
        status: AttemptStatus::Failed,
        format: format!("extension:{}", extension.to_ascii_lowercase()),
        values: None,
        error: Some(error.as_ref().chars().take(512).collect()),
        duration_us: 0,
    }
}

/// Extract one embedded front-cover image from an already-open file. The
/// caller must enforce a source-file size ceiling before invoking Lofty: its
/// format readers materialize embedded picture blocks while parsing. Only
/// formats admitted by this catalog's metadata parsers are considered, and
/// the returned image is independently capped and signature checked.
pub(crate) fn extract_embedded_artwork<R: Read + Seek>(
    reader: R,
    max_image_bytes: usize,
) -> Option<super::AlbumArtwork> {
    use lofty::picture::PictureType;

    let options = ParseOptions::new()
        .read_cover_art(true)
        .read_properties(false)
        .parsing_mode(ParsingMode::BestAttempt);
    let probe = Probe::new(std::io::BufReader::new(reader))
        .options(options)
        .guess_file_type()
        .ok()?;
    if !matches!(
        probe.file_type(),
        Some(
            FileType::Aiff
                | FileType::Flac
                | FileType::Mpeg
                | FileType::Mp4
                | FileType::Opus
                | FileType::Vorbis
                | FileType::Wav
        )
    ) {
        return None;
    }
    let tagged = probe.read().ok()?;
    let pictures = tagged.tags().iter().flat_map(|tag| tag.pictures());
    let pictures = pictures.collect::<Vec<_>>();
    let front = pictures
        .iter()
        .copied()
        .find(|picture| picture.pic_type() == PictureType::CoverFront);
    front
        .into_iter()
        .chain(
            pictures
                .into_iter()
                .filter(|picture| picture.pic_type() != PictureType::CoverFront),
        )
        .find_map(|picture| {
            let bytes = picture.data();
            if bytes.is_empty() || bytes.len() > max_image_bytes {
                return None;
            }
            let mime_type = embedded_image_mime_type(bytes)?;
            Some(super::AlbumArtwork {
                mime_type,
                bytes: bytes.to_vec(),
            })
        })
}

fn embedded_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    let mime = if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        return None;
    };
    let (width, height) = embedded_image_dimensions(bytes, mime)?;
    (width > 0
        && height > 0
        && width <= 4096
        && height <= 4096
        && u64::from(width) * u64::from(height) <= 16_777_216)
        .then_some(mime)
}

/// Validate declared raster dimensions before returning embedded art to the
/// WebView. Byte limits alone do not prevent a tiny compressed image from
/// expanding to a very large pixel buffer.
fn embedded_image_dimensions(bytes: &[u8], mime: &str) -> Option<(u32, u32)> {
    match mime {
        "image/png" if bytes.len() >= 24 => Some((
            u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?),
            u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?),
        )),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut offset = 2;
    while offset + 4 <= bytes.len() {
        if bytes[offset] != 0xff {
            return None;
        }
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = *bytes.get(offset)?;
        offset += 1;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            continue;
        }
        let segment_length = usize::from(u16::from_be_bytes([
            *bytes.get(offset)?,
            *bytes.get(offset + 1)?,
        ]));
        if segment_length < 2 || offset + segment_length > bytes.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if segment_length < 7 {
                return None;
            }
            let height = u32::from(u16::from_be_bytes([
                *bytes.get(offset + 3)?,
                *bytes.get(offset + 4)?,
            ]));
            let width = u32::from(u16::from_be_bytes([
                *bytes.get(offset + 5)?,
                *bytes.get(offset + 6)?,
            ]));
            return Some((width, height));
        }
        offset += segment_length;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || !bytes.starts_with(b"RIFF") || bytes.get(8..12)? != b"WEBP" {
        return None;
    }
    match bytes.get(12..16)? {
        b"VP8X" => {
            let width = 1
                + u32::from(bytes[24])
                + (u32::from(bytes[25]) << 8)
                + (u32::from(bytes[26]) << 16);
            let height = 1
                + u32::from(bytes[27])
                + (u32::from(bytes[28]) << 8)
                + (u32::from(bytes[29]) << 16);
            Some((width, height))
        }
        b"VP8 " if bytes.get(23..26)? == [0x9d, 0x01, 0x2a] => Some((
            u32::from(u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3fff),
            u32::from(u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3fff),
        )),
        b"VP8L" if bytes.get(20) == Some(&0x2f) => {
            let a = u32::from(bytes[21]);
            let b = u32::from(bytes[22]);
            let c = u32::from(bytes[23]);
            let d = u32::from(bytes[24]);
            Some((
                1 + a + ((b & 0x3f) << 8),
                1 + (b >> 6) + (c << 2) + ((d & 0x0f) << 10),
            ))
        }
        _ => None,
    }
}

pub(crate) fn parse_wav<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
    let mut options = ParseOptions::new()
        .read_cover_art(false)
        .parsing_mode(ParsingMode::BestAttempt);
    // WAV properties are part of the initial catalog contract.
    options = options.read_properties(true);

    let parsed = WavFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => Attempt {
            status: AttemptStatus::Failed,
            format: "wav".to_owned(),
            values: None,
            error: Some(error.to_string().chars().take(512).collect()),
            duration_us: 0,
        },
        Ok(wav) => {
            let properties = wav.properties();
            let format = match properties.format() {
                WavFormat::PCM => "wav_pcm".to_owned(),
                WavFormat::IEEE_FLOAT => "wav_ieee_float".to_owned(),
                WavFormat::Other(code) => format!("wav_other_{code:04x}"),
            };
            if properties.format() != &WavFormat::PCM {
                Attempt {
                    status: AttemptStatus::Unsupported,
                    format,
                    values: None,
                    error: None,
                    duration_us: 0,
                }
            } else {
                let properties = wav.properties();
                let mut values = Values {
                    duration_ms: positive_i64(properties.duration().as_millis()),
                    sample_rate: positive_i64(u128::from(properties.sample_rate())),
                    channels: positive_i64(u128::from(properties.channels())),
                    ..Values::default()
                };
                let tagged: lofty::file::TaggedFile = wav.into();

                // RIFF INFO is single-valued for the fields represented here.
                // Do not split text fields heuristically; multi-artist values
                // are intentionally left for formats with explicit semantics.
                for tag in tagged.tags() {
                    values.title = values
                        .title
                        .or_else(|| text(tag.get_string(ItemKey::TrackTitle)));
                    values
                        .track_artists
                        .extend(strings(tag, ItemKey::TrackArtist));
                    values
                        .track_artists
                        .extend(strings(tag, ItemKey::TrackArtists));
                    values
                        .album_artists
                        .extend(strings(tag, ItemKey::AlbumArtist));
                    values
                        .album_artists
                        .extend(strings(tag, ItemKey::AlbumArtists));
                    values.album = values
                        .album
                        .or_else(|| text(tag.get_string(ItemKey::AlbumTitle)));
                    values.genre = values
                        .genre
                        .or_else(|| text(tag.get_string(ItemKey::Genre)));
                    values.date_text = values
                        .date_text
                        .or_else(|| text(tag.get_string(ItemKey::RecordingDate)));
                    values.track_number = values.track_number.or_else(|| positive_u32(tag.track()));
                    values.disc_number = values.disc_number.or_else(|| positive_u32(tag.disk()));
                }

                let has_tags = values.title.is_some()
                    || !values.track_artists.is_empty()
                    || !values.album_artists.is_empty()
                    || values.album.is_some()
                    || values.genre.is_some()
                    || values.date_text.is_some()
                    || values.track_number.is_some()
                    || values.disc_number.is_some();
                Attempt {
                    status: if has_tags {
                        AttemptStatus::ParsedWithTags
                    } else {
                        AttemptStatus::ParsedWithoutTags
                    },
                    format,
                    values: Some(values),
                    error: None,
                    duration_us: 0,
                }
            }
        }
    };

    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

pub(crate) fn parse_flac<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(true)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = FlacFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => Attempt {
            status: AttemptStatus::Failed,
            format: "flac".to_owned(),
            values: None,
            error: Some(error.to_string().chars().take(512).collect()),
            duration_us: 0,
        },
        Ok(flac) => {
            let properties = flac.properties();
            let mut values = Values {
                duration_ms: positive_i64(properties.duration().as_millis()),
                sample_rate: positive_i64(u128::from(properties.sample_rate())),
                channels: positive_i64(u128::from(properties.channels())),
                ..Values::default()
            };
            let tagged: lofty::file::TaggedFile = flac.into();
            for tag in tagged.tags() {
                values.title = values
                    .title
                    .or_else(|| text(tag.get_string(ItemKey::TrackTitle)));
                values
                    .track_artists
                    .extend(strings(tag, ItemKey::TrackArtist));
                values
                    .track_artists
                    .extend(strings(tag, ItemKey::TrackArtists));
                values
                    .album_artists
                    .extend(strings(tag, ItemKey::AlbumArtist));
                values
                    .album_artists
                    .extend(strings(tag, ItemKey::AlbumArtists));
                values.album = values
                    .album
                    .or_else(|| text(tag.get_string(ItemKey::AlbumTitle)));
                values.genre = values
                    .genre
                    .or_else(|| text(tag.get_string(ItemKey::Genre)));
                values.date_text = values
                    .date_text
                    .or_else(|| text(tag.get_string(ItemKey::RecordingDate)));
                values.track_number = values.track_number.or_else(|| positive_u32(tag.track()));
                values.disc_number = values.disc_number.or_else(|| positive_u32(tag.disk()));
            }
            let has_tags = values.title.is_some()
                || !values.track_artists.is_empty()
                || !values.album_artists.is_empty()
                || values.album.is_some()
                || values.genre.is_some()
                || values.date_text.is_some()
                || values.track_number.is_some()
                || values.disc_number.is_some();
            Attempt {
                status: if has_tags {
                    AttemptStatus::ParsedWithTags
                } else {
                    AttemptStatus::ParsedWithoutTags
                },
                format: "flac".to_owned(),
                values: Some(values),
                error: None,
                duration_us: 0,
            }
        }
    };

    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

/// Read bounded APEv2/ID3v1 metadata and stream properties from WavPack.
/// Audio blocks are not decoded and cover art is intentionally skipped.
pub(crate) fn parse_wavpack<R: Read + Seek>(mut reader: R) -> Attempt {
    let started = Instant::now();
    if let Err(error) = validate_ape_tag_bounds(&mut reader) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("wavpack", error)
        };
    }
    if let Err(error) = reader.seek(SeekFrom::Start(0)) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("wavpack", error.to_string())
        };
    }
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(true)
        .max_junk_bytes(MAX_AAC_LEADING_PADDING as usize)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = WavPackFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => parse_failure("wavpack", error.to_string()),
        Ok(wavpack) => {
            let properties = wavpack.properties();
            let duration_ms = positive_i64(properties.duration().as_millis());
            let sample_rate = positive_i64(u128::from(properties.sample_rate()));
            let channels = positive_i64(u128::from(properties.channels()));
            let tagged: TaggedFile = wavpack.into();
            let tag = tagged
                .tag(TagType::Ape)
                .or_else(|| tagged.tag(TagType::Id3v1));
            let mut values = tag.map(values_from_tag).unwrap_or_default();
            values.duration_ms = duration_ms;
            values.sample_rate = sample_rate;
            values.channels = channels;
            Attempt {
                status: if has_text_metadata(&values) {
                    AttemptStatus::ParsedWithTags
                } else {
                    AttemptStatus::ParsedWithoutTags
                },
                format: "wavpack".to_owned(),
                values: Some(values),
                error: None,
                duration_us: 0,
            }
        }
    };
    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

/// Parse classic uncompressed AIFF. AIFF-C is deliberately recognized but not
/// treated as PCM: its compression variants need a separate support policy.
pub(crate) fn parse_aiff<R: Read + Seek>(mut reader: R) -> Attempt {
    let started = Instant::now();
    let raw_date = raw_id3v2_date_in_iff(&mut reader, b"AIFF");
    let _ = reader.seek(SeekFrom::Start(0));
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(true)
        .parsing_mode(ParsingMode::BestAttempt);
    let mut reader = std::io::BufReader::new(reader);
    let mut header = [0; 12];
    let header_result = reader.read_exact(&mut header);
    if let Err(error) = header_result {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("aiff", error.to_string())
        };
    }
    let is_aifc = &header[..4] == b"FORM" && &header[8..12] == b"AIFC";
    if let Err(error) = reader.seek(SeekFrom::Start(0)) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("aiff", error.to_string())
        };
    }
    let parsed = AiffFile::read_from(&mut reader, options);
    let attempt = match parsed {
        Err(error) => parse_failure("aiff", error.to_string()),
        Ok(aiff) => {
            let properties = aiff.properties();
            if is_aifc || properties.compression_type().is_some() {
                Attempt {
                    status: AttemptStatus::Unsupported,
                    format: "aifc_unsupported".to_owned(),
                    values: None,
                    error: None,
                    duration_us: 0,
                }
            } else {
                let mut values = Values {
                    duration_ms: positive_i64(properties.duration().as_millis()),
                    sample_rate: positive_i64(u128::from(properties.sample_rate())),
                    channels: positive_i64(u128::from(properties.channels())),
                    ..Values::default()
                };
                let tagged: TaggedFile = aiff.into();
                merge_aiff_tags(&mut values, &tagged);
                if let Some(date) = raw_date {
                    values.date_text = Some(date);
                }
                let has_tags = has_text_metadata(&values);
                Attempt {
                    status: if has_tags {
                        AttemptStatus::ParsedWithTags
                    } else {
                        AttemptStatus::ParsedWithoutTags
                    },
                    format: "aiff_pcm".to_owned(),
                    values: Some(values),
                    error: None,
                    duration_us: 0,
                }
            }
        }
    };

    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

fn validate_aifc_bounds<R: Read + Seek>(reader: &mut R) -> Result<bool, String> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_| "aifc_size_unavailable".to_owned())?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| "aifc_header_unavailable".to_owned())?;
    let mut form = [0u8; 12];
    reader
        .read_exact(&mut form)
        .map_err(|_| "aifc_header_truncated".to_owned())?;
    if &form[..4] != b"FORM" || &form[8..12] != b"AIFC" {
        return Ok(false);
    }
    let form_size = u32::from_be_bytes(form[4..8].try_into().unwrap()) as u64;
    let form_end = 8u64
        .checked_add(form_size)
        .filter(|end| *end == file_len && form_size >= 4)
        .ok_or_else(|| "aifc_form_bounds".to_owned())?;

    let mut position = 12u64;
    let mut chunks = 0usize;
    let mut saw_comm = false;
    let mut saw_sound = false;
    let mut id3_bytes = 0u64;
    let mut text_bytes = 0u64;
    while position < form_end {
        chunks += 1;
        if chunks > MAX_AIFF_CHUNKS {
            return Err("aifc_chunk_count_limit".to_owned());
        }
        reader
            .seek(SeekFrom::Start(position))
            .map_err(|_| "aifc_chunk_seek_failed".to_owned())?;
        let mut header = [0u8; 8];
        reader
            .read_exact(&mut header)
            .map_err(|_| "aifc_chunk_header_truncated".to_owned())?;
        let id = &header[..4];
        let size = u32::from_be_bytes(header[4..8].try_into().unwrap());
        let data_start = position
            .checked_add(8)
            .ok_or_else(|| "aifc_chunk_bounds".to_owned())?;
        let data_end = data_start
            .checked_add(u64::from(size))
            .filter(|end| *end <= form_end)
            .ok_or_else(|| "aifc_chunk_bounds".to_owned())?;
        let next = data_end
            .checked_add(u64::from(size & 1))
            .filter(|end| *end <= form_end)
            .ok_or_else(|| "aifc_chunk_padding_bounds".to_owned())?;

        match id {
            b"COMM" => {
                if saw_comm || size < 22 {
                    return Err("aifc_common_chunk_invalid".to_owned());
                }
                saw_comm = true;
            }
            b"SSND" => {
                if saw_sound || size < 8 {
                    return Err("aifc_sound_chunk_invalid".to_owned());
                }
                saw_sound = true;
            }
            b"ID3 " | b"id3 " => {
                id3_bytes = id3_bytes
                    .checked_add(u64::from(size))
                    .filter(|total| *total <= MAX_ID3_BYTES as u64)
                    .ok_or_else(|| "aifc_id3_size_limit".to_owned())?;
            }
            b"NAME" | b"AUTH" | b"ANNO" | b"(c) " | b"COMT" => {
                if size > MAX_AIFF_TEXT_BYTES {
                    return Err("aifc_text_chunk_size_limit".to_owned());
                }
                text_bytes = text_bytes
                    .checked_add(u64::from(size))
                    .filter(|total| *total <= u64::from(MAX_AIFF_TEXT_BYTES))
                    .ok_or_else(|| "aifc_text_total_size_limit".to_owned())?;
            }
            _ => {}
        }
        position = next;
    }
    if !saw_comm || !saw_sound {
        return Err("aifc_required_chunks_missing".to_owned());
    }
    Ok(true)
}

fn id3v2_total_size_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    file_len: u64,
) -> Result<Option<u64>, String> {
    if offset.checked_add(10).is_none_or(|end| end > file_len) {
        return Ok(None);
    }
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|_| "id3_header_seek_failed".to_owned())?;
    let mut header = [0u8; 10];
    reader
        .read_exact(&mut header)
        .map_err(|_| "id3_header_truncated".to_owned())?;
    if &header[..3] != b"ID3" {
        return Ok(None);
    }
    if !(2..=4).contains(&header[3]) {
        return Err("id3_version_unsupported".to_owned());
    }
    let payload_size = synchsafe_size(&header[6..10]).map_err(|_| "id3_size_invalid".to_owned())?;
    if payload_size > MAX_ID3_BYTES {
        return Err("id3_size_limit".to_owned());
    }
    let footer_size = u64::from(header[3] == 4 && header[5] & 0x10 != 0) * 10;
    let total = 10u64
        .checked_add(payload_size as u64)
        .and_then(|size| size.checked_add(footer_size))
        .ok_or_else(|| "id3_size_overflow".to_owned())?;
    if offset.checked_add(total).is_none_or(|end| end > file_len) {
        return Err("id3_bounds_invalid".to_owned());
    }
    Ok(Some(total))
}

/// Return true for ADIF, which Lofty does not support, and reject overlarge
/// ID3 prefixes before Lofty can materialize individual frames.
fn validate_aac_tag_bounds<R: Read + Seek>(reader: &mut R) -> Result<bool, String> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_| "aac_size_unavailable".to_owned())?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| "aac_header_unavailable".to_owned())?;
    let mut skipped = 0u64;
    let mut first = [0u8; 1];
    loop {
        if reader.read_exact(&mut first).is_err() {
            return Ok(false);
        }
        if first[0] != 0 {
            break;
        }
        skipped += 1;
        if skipped > MAX_AAC_LEADING_PADDING {
            return Err("aac_leading_padding_limit".to_owned());
        }
    }
    reader
        .seek(SeekFrom::Start(skipped))
        .map_err(|_| "aac_prefix_seek_failed".to_owned())?;
    let mut prefix = [0u8; 10];
    if reader.read_exact(&mut prefix).is_err() {
        return Ok(false);
    }
    if &prefix[..4] == b"ADIF" {
        return Ok(true);
    }
    if &prefix[..3] == b"ID3" {
        let mut tag_offset = skipped;
        let mut total_tag_size = 0u64;
        while let Some(tag_size) = id3v2_total_size_at(reader, tag_offset, file_len)? {
            total_tag_size = total_tag_size
                .checked_add(tag_size)
                .filter(|total| *total <= MAX_ID3_BYTES as u64)
                .ok_or_else(|| "aac_id3_total_size_limit".to_owned())?;
            tag_offset = tag_offset
                .checked_add(tag_size)
                .ok_or_else(|| "aac_id3_bounds_invalid".to_owned())?;
        }
        if total_tag_size == 0 {
            return Err("aac_id3_header_invalid".to_owned());
        }
        if tag_offset.checked_add(4).is_some_and(|end| end <= file_len) {
            reader
                .seek(SeekFrom::Start(tag_offset))
                .map_err(|_| "aac_audio_header_seek_failed".to_owned())?;
            let mut audio_header = [0u8; 4];
            reader
                .read_exact(&mut audio_header)
                .map_err(|_| "aac_audio_header_truncated".to_owned())?;
            if &audio_header == b"ADIF" {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn ape_header_at<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    file_len: u64,
    is_footer: bool,
) -> Result<bool, String> {
    if offset.checked_add(32).is_none_or(|end| end > file_len) {
        return Ok(false);
    }
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|_| "ape_header_seek_failed".to_owned())?;
    let mut header = [0u8; 32];
    reader
        .read_exact(&mut header)
        .map_err(|_| "ape_header_truncated".to_owned())?;
    if &header[..8] != b"APETAGEX" {
        return Ok(false);
    }
    let version = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let size = u32::from_le_bytes(header[12..16].try_into().unwrap()) as u64;
    if !matches!(version, 1000 | 2000) || size < 32 || size > MAX_APE_TAG_BYTES {
        return Err("ape_tag_header_invalid_or_too_large".to_owned());
    }
    let bounded = if is_footer {
        size <= offset.saturating_add(32)
    } else {
        offset.checked_add(size).is_some_and(|end| end <= file_len)
    };
    if !bounded {
        return Err("ape_tag_bounds_invalid".to_owned());
    }
    Ok(true)
}

fn validate_ape_tag_bounds<R: Read + Seek>(reader: &mut R) -> Result<(), String> {
    let file_len = reader
        .seek(SeekFrom::End(0))
        .map_err(|_| "ape_size_unavailable".to_owned())?;
    let mut start = 0u64;
    if let Some(id3_size) = id3v2_total_size_at(reader, 0, file_len)? {
        start = id3_size;
    }
    let _ = ape_header_at(reader, start, file_len, false)?;

    let mut footer_offset = file_len.saturating_sub(32);
    if file_len >= 160 {
        reader
            .seek(SeekFrom::Start(file_len - 128))
            .map_err(|_| "ape_id3v1_seek_failed".to_owned())?;
        let mut id3v1 = [0u8; 3];
        if reader.read_exact(&mut id3v1).is_ok() && &id3v1 == b"TAG" {
            footer_offset = file_len - 160;
        }
    }
    let _ = ape_header_at(reader, footer_offset, file_len, true)?;
    Ok(())
}

/// Read metadata from AIFC containers without claiming codec support. The
/// container and metadata chunks are bounded before Lofty parses them; audio
/// properties are disabled so SSND payloads are skipped.
pub(crate) fn parse_aifc<R: Read + Seek>(mut reader: R) -> Attempt {
    let started = Instant::now();
    match validate_aifc_bounds(&mut reader) {
        Ok(true) => {}
        Ok(false) => {
            return Attempt {
                status: AttemptStatus::Unsupported,
                format: "aifc_content_mismatch".to_owned(),
                values: None,
                error: None,
                duration_us: started.elapsed().as_micros(),
            };
        }
        Err(error) => {
            return Attempt {
                duration_us: started.elapsed().as_micros(),
                ..parse_failure("aifc", error)
            };
        }
    }
    let raw_date = raw_id3v2_date_in_iff(&mut reader, b"AIFC");

    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false)
        .max_junk_bytes(MAX_AAC_LEADING_PADDING as usize)
        .parsing_mode(ParsingMode::BestAttempt);
    if let Err(error) = reader.seek(SeekFrom::Start(0)) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("aifc", error.to_string())
        };
    }
    let parsed = AiffFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => parse_failure("aifc", error.to_string()),
        Ok(aifc) => {
            let tagged: TaggedFile = aifc.into();
            let mut values = Values::default();
            merge_aiff_tags(&mut values, &tagged);
            if let Some(date) = raw_date {
                values.date_text = Some(date);
            }
            let has_tags = has_text_metadata(&values);
            Attempt {
                status: if has_tags {
                    AttemptStatus::ParsedWithTags
                } else {
                    AttemptStatus::ParsedWithoutTags
                },
                format: "aifc_metadata".to_owned(),
                values: Some(values),
                error: None,
                duration_us: 0,
            }
        }
    };
    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

/// Parse AAC in ADTS containers. Lofty reads ID3v2/ID3v1 and only probes the
/// first two ADTS frame headers when audio properties are disabled.
pub(crate) fn parse_aac<R: Read + Seek>(mut reader: R) -> Attempt {
    let started = Instant::now();
    match validate_aac_tag_bounds(&mut reader) {
        Ok(true) => {
            return Attempt {
                status: AttemptStatus::Unsupported,
                format: "aac_adif_unsupported".to_owned(),
                values: None,
                error: None,
                duration_us: started.elapsed().as_micros(),
            };
        }
        Ok(false) => {}
        Err(error) => {
            return Attempt {
                duration_us: started.elapsed().as_micros(),
                ..parse_failure("aac_adts", error)
            };
        }
    }
    let raw_date = raw_id3v2_prefix_date(&mut reader, true);

    if let Err(error) = reader.seek(SeekFrom::Start(0)) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("aac_adts", error.to_string())
        };
    }
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false)
        .max_junk_bytes(MAX_AAC_LEADING_PADDING as usize)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = AacFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => parse_failure("aac_adts", error.to_string()),
        Ok(aac) => {
            let tagged: TaggedFile = aac.into();
            let id3v2 = tagged.tag(TagType::Id3v2);
            let tag = id3v2.or_else(|| tagged.tag(TagType::Id3v1));
            let values = tag.map(values_from_tag).unwrap_or_default();
            let mut values = values;
            if id3v2.is_some() {
                if let Some(date) = raw_date {
                    values.date_text = Some(date);
                }
            }
            let has_tags = has_text_metadata(&values);
            Attempt {
                status: if has_tags {
                    AttemptStatus::ParsedWithTags
                } else {
                    AttemptStatus::ParsedWithoutTags
                },
                format: "aac_adts".to_owned(),
                values: Some(values),
                error: None,
                duration_us: 0,
            }
        }
    };
    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

/// Parse Monkey's Audio metadata using bounded APE/ID3 tag sizes. Audio
/// properties are disabled, so the encoded audio payload is not traversed.
pub(crate) fn parse_ape<R: Read + Seek>(mut reader: R) -> Attempt {
    let started = Instant::now();
    if let Err(error) = validate_ape_tag_bounds(&mut reader) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("ape", error)
        };
    }
    let raw_date = raw_id3v2_prefix_date(&mut reader, false);
    if let Err(error) = reader.seek(SeekFrom::Start(0)) {
        return Attempt {
            duration_us: started.elapsed().as_micros(),
            ..parse_failure("ape", error.to_string())
        };
    }
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(false)
        .max_junk_bytes(MAX_AAC_LEADING_PADDING as usize)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = ApeFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => parse_failure("ape", error.to_string()),
        Ok(ape) => {
            let tagged: TaggedFile = ape.into();
            let ape_tag = tagged.tag(TagType::Ape);
            let id3v2 = tagged.tag(TagType::Id3v2);
            let tag = ape_tag.or(id3v2).or_else(|| tagged.tag(TagType::Id3v1));
            let values = tag.map(values_from_tag).unwrap_or_default();
            let mut values = values;
            if ape_tag.is_none() && id3v2.is_some() {
                if let Some(date) = raw_date {
                    values.date_text = Some(date);
                }
            }
            let has_tags = has_text_metadata(&values);
            Attempt {
                status: if has_tags {
                    AttemptStatus::ParsedWithTags
                } else {
                    AttemptStatus::ParsedWithoutTags
                },
                format: "ape".to_owned(),
                values: Some(values),
                error: None,
                duration_us: 0,
            }
        }
    };
    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

pub(crate) fn parse_mp3<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
    let mut reader = reader;
    let raw_date = raw_id3v2_prefix_date(&mut reader, false);
    let _ = reader.seek(SeekFrom::Start(0));
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(true)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = MpegFile::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => Attempt {
            status: AttemptStatus::Failed,
            format: "mp3".to_owned(),
            values: None,
            error: Some(error.to_string().chars().take(512).collect()),
            duration_us: 0,
        },
        Ok(mpeg) => {
            let properties = mpeg.properties();
            if properties.layer() != &Layer::Layer3 {
                let layer = match properties.layer() {
                    Layer::Layer1 => 1,
                    Layer::Layer2 => 2,
                    Layer::Layer3 => unreachable!("Layer III handled above"),
                };
                Attempt {
                    status: AttemptStatus::Unsupported,
                    format: format!("mpeg_layer_{layer}"),
                    values: None,
                    error: None,
                    duration_us: 0,
                }
            } else {
                let mut values = Values {
                    duration_ms: positive_i64(properties.duration().as_millis()),
                    sample_rate: positive_i64(u128::from(properties.sample_rate())),
                    channels: positive_i64(u128::from(properties.channels())),
                    ..Values::default()
                };
                let tagged: lofty::file::TaggedFile = mpeg.into();

                // Select one complete tag source; never merge conflicting fields.
                // ID3v2 is the MP3 primary tag, followed by APE and ID3v1 fallback.
                let id3v2 = tagged.tag(TagType::Id3v2);
                if let Some(tag) = id3v2
                    .or_else(|| tagged.tag(TagType::Ape))
                    .or_else(|| tagged.tag(TagType::Id3v1))
                {
                    values.title = text(tag.get_string(ItemKey::TrackTitle));
                    values.track_artists = strings(tag, ItemKey::TrackArtist);
                    values
                        .track_artists
                        .extend(strings(tag, ItemKey::TrackArtists));
                    values.album_artists = strings(tag, ItemKey::AlbumArtist);
                    values
                        .album_artists
                        .extend(strings(tag, ItemKey::AlbumArtists));
                    values.album = text(tag.get_string(ItemKey::AlbumTitle));
                    values.genre = text(tag.get_string(ItemKey::Genre));
                    values.date_text = text(tag.get_string(ItemKey::RecordingDate));
                    if id3v2.is_some() {
                        values.date_text = raw_date.or_else(|| values.date_text.take());
                    }
                    values.track_number = positive_u32(tag.track());
                    values.disc_number = positive_u32(tag.disk());
                }

                let has_tags = values.title.is_some()
                    || !values.track_artists.is_empty()
                    || !values.album_artists.is_empty()
                    || values.album.is_some()
                    || values.genre.is_some()
                    || values.date_text.is_some()
                    || values.track_number.is_some()
                    || values.disc_number.is_some();
                Attempt {
                    status: if has_tags {
                        AttemptStatus::ParsedWithTags
                    } else {
                        AttemptStatus::ParsedWithoutTags
                    },
                    format: "mp3".to_owned(),
                    values: Some(values),
                    error: None,
                    duration_us: 0,
                }
            }
        }
    };

    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

/// Parse MP4-family audio when Lofty identifies the audio track as AAC or ALAC.
/// Cover art remains disabled and the already-open secure scanner reader is used.
pub(crate) fn parse_mp4<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(true)
        .parsing_mode(ParsingMode::BestAttempt);
    let parsed = Mp4File::read_from(&mut std::io::BufReader::new(reader), options);
    let attempt = match parsed {
        Err(error) => parse_failure("mp4", error.to_string()),
        Ok(mp4) => {
            let properties = mp4.properties();
            let format = match properties.codec() {
                Some(Mp4Codec::AAC) if properties.audio_object_type().is_some() => "mp4_aac",
                // Lofty deliberately guesses AAC if an `mp4a` sample entry
                // lacks a usable decoder descriptor. Do not claim AAC from
                // that guess alone.
                Some(Mp4Codec::AAC) => "mp4_aac_unverified",
                Some(Mp4Codec::ALAC) => "mp4_alac",
                Some(Mp4Codec::MP3) => "mp4_mp3_unsupported",
                Some(Mp4Codec::FLAC) => "mp4_flac_unsupported",
                None => "mp4_codec_unknown",
                _ => "mp4_codec_unknown",
            };
            let supported_codec = matches!(properties.codec(), Some(Mp4Codec::ALAC))
                || (properties.codec() == Some(Mp4Codec::AAC)
                    && properties.audio_object_type().is_some());
            if !supported_codec {
                Attempt {
                    status: AttemptStatus::Unsupported,
                    format: format.to_owned(),
                    values: None,
                    error: None,
                    duration_us: 0,
                }
            } else {
                let mut values = Values {
                    duration_ms: positive_i64(properties.duration().as_millis()),
                    sample_rate: positive_u32(properties.sample_rate()),
                    channels: positive_u32(properties.channels().map(u32::from)),
                    ..Values::default()
                };
                let tagged: TaggedFile = mp4.into();
                if let Some(tag) = tagged.tag(TagType::Mp4Ilst) {
                    values.title = text(tag.get_string(ItemKey::TrackTitle));
                    values.track_artists = strings(tag, ItemKey::TrackArtist);
                    values
                        .track_artists
                        .extend(strings(tag, ItemKey::TrackArtists));
                    values.album_artists = strings(tag, ItemKey::AlbumArtist);
                    values
                        .album_artists
                        .extend(strings(tag, ItemKey::AlbumArtists));
                    values.album = text(tag.get_string(ItemKey::AlbumTitle));
                    values.genre = text(tag.get_string(ItemKey::Genre));
                    values.date_text = text(tag.get_string(ItemKey::RecordingDate));
                    values.track_number = positive_u32(tag.track());
                    values.disc_number = positive_u32(tag.disk());
                }
                let has_tags = has_text_metadata(&values);
                Attempt {
                    status: if has_tags {
                        AttemptStatus::ParsedWithTags
                    } else {
                        AttemptStatus::ParsedWithoutTags
                    },
                    format: format.to_owned(),
                    values: Some(values),
                    error: None,
                    duration_us: 0,
                }
            }
        }
    };
    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

/// Parse supported Ogg codecs by inspecting the stream's identification packet, not its suffix.
/// The reader comes from the secure scanner and is never reopened by pathname.
pub(crate) fn parse_ogg<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
    let options = ParseOptions::new()
        .read_cover_art(false)
        .read_properties(true)
        .parsing_mode(ParsingMode::BestAttempt);
    let probe = Probe::new(std::io::BufReader::new(reader))
        .options(options)
        .guess_file_type();

    let attempt = match probe {
        Err(error) => parse_failure("ogg", error.to_string()),
        Ok(probe) => match probe.file_type() {
            Some(FileType::Vorbis) => parse_ogg_tagged(probe.read(), "ogg_vorbis"),
            Some(FileType::Opus) => parse_ogg_tagged(probe.read(), "ogg_opus"),
            Some(FileType::Speex) => Attempt {
                status: AttemptStatus::Unsupported,
                format: "ogg_speex".to_owned(),
                values: None,
                error: None,
                duration_us: 0,
            },
            Some(other) => Attempt {
                status: AttemptStatus::Unsupported,
                format: format!("ogg_extension_contains_{}", file_type_name(other)),
                values: None,
                error: None,
                duration_us: 0,
            },
            None => parse_failure("ogg", "unrecognized or invalid media stream".to_owned()),
        },
    };

    Attempt {
        duration_us: started.elapsed().as_micros(),
        ..attempt
    }
}

fn parse_ogg_tagged(
    parsed: std::result::Result<TaggedFile, lofty::error::FileParseError>,
    format: &str,
) -> Attempt {
    match parsed {
        Err(error) => parse_failure(format, error.to_string()),
        Ok(tagged) => {
            let properties = tagged.properties();
            // Lofty maps OpusProperties::input_sample_rate() to FileProperties::sample_rate().
            // This is the rate declared in OpusHead, not the codec's fixed 48 kHz decode clock.
            let mut values = Values {
                duration_ms: positive_i64(properties.duration().as_millis()),
                sample_rate: positive_u32(properties.sample_rate()),
                channels: positive_u32(properties.channels().map(u32::from)),
                ..Values::default()
            };

            for tag in tagged.tags() {
                values.title = values
                    .title
                    .or_else(|| text(tag.get_string(ItemKey::TrackTitle)));
                values
                    .track_artists
                    .extend(strings(tag, ItemKey::TrackArtist));
                values
                    .track_artists
                    .extend(strings(tag, ItemKey::TrackArtists));
                values
                    .album_artists
                    .extend(strings(tag, ItemKey::AlbumArtist));
                values
                    .album_artists
                    .extend(strings(tag, ItemKey::AlbumArtists));
                values.album = values
                    .album
                    .or_else(|| text(tag.get_string(ItemKey::AlbumTitle)));
                values.genre = values
                    .genre
                    .or_else(|| text(tag.get_string(ItemKey::Genre)));
                values.date_text = values
                    .date_text
                    .or_else(|| text(tag.get_string(ItemKey::RecordingDate)));
                values.track_number = values.track_number.or_else(|| positive_u32(tag.track()));
                values.disc_number = values.disc_number.or_else(|| positive_u32(tag.disk()));
            }

            let has_tags = has_text_metadata(&values);
            Attempt {
                status: if has_tags {
                    AttemptStatus::ParsedWithTags
                } else {
                    AttemptStatus::ParsedWithoutTags
                },
                format: format.to_owned(),
                values: Some(values),
                error: None,
                duration_us: 0,
            }
        }
    }
}

fn parse_failure(format: &str, error: String) -> Attempt {
    Attempt {
        status: AttemptStatus::Failed,
        format: format.to_owned(),
        values: None,
        error: Some(error.chars().take(512).collect()),
        duration_us: 0,
    }
}

fn has_text_metadata(values: &Values) -> bool {
    values.title.is_some()
        || !values.track_artists.is_empty()
        || !values.album_artists.is_empty()
        || values.album.is_some()
        || values.genre.is_some()
        || values.date_text.is_some()
        || values.track_number.is_some()
        || values.disc_number.is_some()
}

fn file_type_name(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Aac => "aac",
        FileType::Aiff => "aiff",
        FileType::Ape => "ape",
        FileType::Flac => "flac",
        FileType::Mpeg => "mpeg",
        FileType::Mp4 => "mp4",
        FileType::Mpc => "mpc",
        FileType::Opus => "opus",
        FileType::Vorbis => "vorbis",
        FileType::Speex => "speex",
        FileType::Wav => "wav",
        FileType::WavPack => "wavpack",
        FileType::Custom(_) => "custom",
        _ => "other",
    }
}

fn strings(tag: &Tag, key: ItemKey) -> Vec<String> {
    tag.get_strings(key)
        .filter_map(|value| text(Some(value)))
        .collect()
}

fn text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn positive_i64(value: u128) -> Option<i64> {
    i64::try_from(value).ok().filter(|value| *value > 0)
}

fn positive_u32(value: Option<u32>) -> Option<i64> {
    value.and_then(|value| (value > 0).then_some(i64::from(value)))
}

#[cfg(test)]
pub(crate) fn synthetic_flac(comments: &[(&str, &str)]) -> Vec<u8> {
    let source = include_bytes!("fixtures/synthetic.flac");
    let mut offset = 4;
    let mut blocks = Vec::new();
    loop {
        let header = source[offset];
        let block_type = header & 0x7f;
        let length = (usize::from(source[offset + 1]) << 16)
            | (usize::from(source[offset + 2]) << 8)
            | usize::from(source[offset + 3]);
        offset += 4;
        let content = &source[offset..offset + length];
        if block_type != 1 && block_type != 4 {
            blocks.push((block_type, content.to_vec()));
        }
        offset += length;
        if header & 0x80 != 0 {
            break;
        }
    }

    if !comments.is_empty() {
        let vendor = b"sonante-synthetic-fixture";
        let mut content = Vec::new();
        content.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        content.extend_from_slice(vendor);
        content.extend_from_slice(&(comments.len() as u32).to_le_bytes());
        for (key, value) in comments {
            let comment = format!("{key}={value}");
            content.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            content.extend_from_slice(comment.as_bytes());
        }
        blocks.push((4, content));
    }

    let mut result = b"fLaC".to_vec();
    for (index, (block_type, content)) in blocks.iter().enumerate() {
        assert!(*block_type < 0x80 && content.len() < 0x0100_0000);
        let last = index + 1 == blocks.len();
        result.push((if last { 0x80 } else { 0 }) | block_type);
        result.push(((content.len() >> 16) & 0xff) as u8);
        result.push(((content.len() >> 8) & 0xff) as u8);
        result.push((content.len() & 0xff) as u8);
        result.extend_from_slice(content);
    }
    result.extend_from_slice(&source[offset..]);
    result
}

/// Build small tagged fixtures around an ffmpeg-generated silent MP3 stream.
/// Tags are emitted through the pinned Lofty writer, never hand-crafted.
#[cfg(test)]
pub(crate) fn synthetic_mp3(
    tag_sets: &[(lofty::tag::TagType, &[(lofty::tag::ItemKey, &str)])],
    id3v23: bool,
) -> Vec<u8> {
    use lofty::{
        config::WriteOptions,
        tag::{ItemValue, TagExt, TagItem},
    };
    use std::io::Cursor;

    let encoded = include_str!("fixtures/synthetic-silent.mp3.hex");
    let hex = encoded
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let mut bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(hex.len() % 2, 0);
    for (tag_type, items) in tag_sets {
        let mut tag = Tag::new(*tag_type);
        for (key, value) in *items {
            match key {
                lofty::tag::ItemKey::TrackNumber => {
                    if let Some(number) = value.split('/').next().and_then(|part| part.parse().ok())
                    {
                        tag.set_track(number);
                        if let Some(total) =
                            value.split('/').nth(1).and_then(|part| part.parse().ok())
                        {
                            tag.set_track_total(total);
                        }
                    }
                }
                lofty::tag::ItemKey::DiscNumber => {
                    if let Some(number) = value.split('/').next().and_then(|part| part.parse().ok())
                    {
                        tag.set_disk(number);
                        if let Some(total) =
                            value.split('/').nth(1).and_then(|part| part.parse().ok())
                        {
                            tag.set_disk_total(total);
                        }
                    }
                }
                _ => assert!(tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())))),
            }
        }
        let mut options = WriteOptions::new();
        options = options.use_id3v23(id3v23);
        let mut file = Cursor::new(bytes);
        tag.save_to(&mut file, options).unwrap();
        bytes = file.into_inner();
    }
    bytes
}

#[cfg(test)]
pub(crate) fn synthetic_wavpack() -> Vec<u8> {
    let encoded = include_str!("fixtures/synthetic-wavpack.wv.hex");
    let hex = encoded
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// Build Vorbis Comments over a locally encoded synthetic Ogg stream using the pinned Lofty
/// writer. Repeated artist values remain separate comment entries.
#[cfg(test)]
pub(crate) fn synthetic_ogg(source: &[u8], items: &[(lofty::tag::ItemKey, &str)]) -> Vec<u8> {
    use lofty::{
        config::WriteOptions,
        tag::{ItemValue, TagExt, TagItem},
    };
    use std::io::Cursor;

    let mut tag = Tag::new(TagType::VorbisComments);
    for (key, value) in items {
        match key {
            ItemKey::TrackNumber => {
                if let Some(number) = value.split('/').next().and_then(|part| part.parse().ok()) {
                    tag.set_track(number);
                }
            }
            ItemKey::DiscNumber => {
                if let Some(number) = value.split('/').next().and_then(|part| part.parse().ok()) {
                    tag.set_disk(number);
                }
            }
            _ => assert!(tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())))),
        }
    }

    let mut file = Cursor::new(source.to_vec());
    tag.save_to(&mut file, WriteOptions::default()).unwrap();
    file.into_inner()
}

#[cfg(test)]
pub(crate) fn synthetic_mp4(source: &[u8], items: &[(lofty::tag::ItemKey, &str)]) -> Vec<u8> {
    use lofty::{
        config::WriteOptions,
        tag::{ItemValue, TagExt, TagItem},
    };
    use std::io::Cursor;

    let mut tag = Tag::new(TagType::Mp4Ilst);
    for (key, value) in items {
        match key {
            ItemKey::TrackNumber => {
                if let Some(number) = value.split('/').next().and_then(|part| part.parse().ok()) {
                    tag.set_track(number);
                    if let Some(total) = value.split('/').nth(1).and_then(|part| part.parse().ok())
                    {
                        tag.set_track_total(total);
                    }
                }
            }
            ItemKey::DiscNumber => {
                if let Some(number) = value.split('/').next().and_then(|part| part.parse().ok()) {
                    tag.set_disk(number);
                    if let Some(total) = value.split('/').nth(1).and_then(|part| part.parse().ok())
                    {
                        tag.set_disk_total(total);
                    }
                }
            }
            _ => assert!(tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())))),
        }
    }
    let mut file = Cursor::new(source.to_vec());
    tag.save_to(&mut file, WriteOptions::default()).unwrap();
    file.into_inner()
}

/// Build a valid, tiny AIFF PCM fixture with optional ID3v2 and AIFF text chunks.
#[cfg(test)]
pub(crate) fn synthetic_aiff(
    id3_items: &[(lofty::tag::ItemKey, &str)],
    text_chunks: &[(&[u8; 4], &str)],
    aifc: bool,
) -> Vec<u8> {
    use lofty::{
        config::WriteOptions,
        tag::{ItemKey, ItemValue, TagExt, TagItem},
    };

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(8 + data.len() + data.len() % 2);
        result.extend_from_slice(id);
        result.extend_from_slice(&(data.len() as u32).to_be_bytes());
        result.extend_from_slice(data);
        if data.len() % 2 != 0 {
            result.push(0);
        }
        result
    }

    let mut body = if aifc {
        b"AIFC".to_vec()
    } else {
        b"AIFF".to_vec()
    };
    let mut comm = Vec::new();
    comm.extend_from_slice(&2_u16.to_be_bytes()); // stereo
    comm.extend_from_slice(&4_410_u32.to_be_bytes()); // 100 ms at 44.1 kHz
    comm.extend_from_slice(&16_u16.to_be_bytes());
    comm.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]); // 44.1 kHz extended
    if aifc {
        comm.extend_from_slice(b"NONE");
        comm.push(4);
        comm.extend_from_slice(b"NONE");
    }
    body.extend(chunk(b"COMM", &comm));

    let sound = vec![0_u8; 8 + 4_410 * 2 * 2];
    body.extend(chunk(b"SSND", &sound));
    for (id, value) in text_chunks {
        body.extend(chunk(id, value.as_bytes()));
    }

    if !id3_items.is_empty() {
        let mut tag = Tag::new(TagType::Id3v2);
        for (key, value) in id3_items {
            match key {
                ItemKey::TrackNumber => {
                    if let Ok(number) = value.parse() {
                        tag.set_track(number);
                    }
                }
                ItemKey::DiscNumber => {
                    if let Ok(number) = value.parse() {
                        tag.set_disk(number);
                    }
                }
                _ => assert!(tag.push(TagItem::new(*key, ItemValue::Text((*value).to_owned())))),
            }
        }
        let id3: lofty::id3::v2::Id3v2Tag = tag.into();
        let mut bytes = Vec::new();
        id3.dump_to(&mut bytes, WriteOptions::default()).unwrap();
        body.extend(chunk(b"ID3 ", &bytes));
    }

    let mut result = b"FORM".to_vec();
    result.extend_from_slice(&(body.len() as u32).to_be_bytes());
    result.extend(body);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 != 0 {
            out.push(0);
        }
        out
    }

    fn synthetic_id3(items: &[(ItemKey, &str)]) -> Vec<u8> {
        let tag_sets = [(TagType::Id3v2, items)];
        let mp3 = synthetic_mp3(&tag_sets, false);
        assert_eq!(&mp3[..3], b"ID3");
        let size = synchsafe_size(&mp3[6..10]).unwrap();
        mp3[..10 + size].to_vec()
    }

    fn synthetic_adts_frame() -> [u8; 7] {
        [0xff, 0xf1, 0x50, 0x80, 0x00, 0xe0, 0xfc]
    }

    fn synthetic_aac(id3: Option<&[u8]>, frames: usize) -> Vec<u8> {
        let mut bytes = id3.unwrap_or_default().to_vec();
        for _ in 0..frames {
            bytes.extend_from_slice(&synthetic_adts_frame());
        }
        bytes
    }

    fn synthetic_ape(items: &[(&str, &str)]) -> Vec<u8> {
        let mut encoded_items = Vec::new();
        for (key, value) in items {
            encoded_items.extend_from_slice(&(value.len() as u32).to_le_bytes());
            encoded_items.extend_from_slice(&0u32.to_le_bytes());
            encoded_items.extend_from_slice(key.as_bytes());
            encoded_items.push(0);
            encoded_items.extend_from_slice(value.as_bytes());
        }
        let tag_size = encoded_items.len() as u32 + 32;
        let mut bytes = b"MAC ".to_vec();
        bytes.extend_from_slice(&[0; 32]); // audio bytes are never decoded here
        bytes.extend(encoded_items);
        bytes.extend_from_slice(b"APETAGEX");
        bytes.extend_from_slice(&2000u32.to_le_bytes());
        bytes.extend_from_slice(&tag_size.to_le_bytes());
        bytes.extend_from_slice(&(items.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 8]);
        bytes
    }

    fn synthetic_legacy_id3_date(
        version: u8,
        year: Option<&str>,
        day_month: Option<&str>,
    ) -> Vec<u8> {
        let mut body = Vec::new();
        for (id, value) in [(b"TYER", year), (b"TDAT", day_month)] {
            let Some(value) = value else { continue };
            let mut payload = vec![0]; // ISO-8859-1 text encoding
            payload.extend_from_slice(value.as_bytes());
            body.extend_from_slice(id);
            body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            body.extend_from_slice(&[0, 0]);
            body.extend(payload);
        }
        let size = body.len();
        let mut tag = b"ID3".to_vec();
        tag.extend_from_slice(&[version, 0, 0]);
        tag.extend_from_slice(&[
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]);
        tag.extend(body);
        tag
    }

    fn synthetic_dsf(id3: Option<&[u8]>, audio_len: usize) -> Vec<u8> {
        let data_len = 12usize + audio_len;
        let metadata_offset = if id3.is_some() { 80 + data_len } else { 0 };
        let file_len = 80 + data_len + id3.map_or(0, <[u8]>::len);
        let mut bytes = Vec::with_capacity(file_len);
        bytes.extend_from_slice(b"DSD ");
        bytes.extend_from_slice(&28u64.to_le_bytes());
        bytes.extend_from_slice(&(file_len as u64).to_le_bytes());
        bytes.extend_from_slice(&(metadata_offset as u64).to_le_bytes());
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&52u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 40]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u64).to_le_bytes());
        bytes.resize(bytes.len() + audio_len, 0);
        if let Some(id3) = id3 {
            bytes.extend_from_slice(id3);
        }
        bytes
    }

    fn dff_chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(12 + data.len() + data.len() % 2);
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
        bytes.extend_from_slice(data);
        if data.len() % 2 != 0 {
            bytes.push(0);
        }
        bytes
    }

    fn dff_text_chunk(id: &[u8; 4], value: &str) -> Vec<u8> {
        let mut data = (value.len() as u32).to_be_bytes().to_vec();
        data.extend_from_slice(value.as_bytes());
        dff_chunk(id, &data)
    }

    fn synthetic_dff(native: &[(&[u8; 4], &str)], id3: Option<&[u8]>) -> Vec<u8> {
        let mut body = dff_chunk(b"FVER", &0x01050000u32.to_be_bytes());
        body.extend(dff_chunk(b"PROP", b"SND "));
        body.extend(dff_chunk(b"DSD ", &[0; 4]));
        if !native.is_empty() {
            let mut info = Vec::new();
            for (id, value) in native {
                info.extend(dff_text_chunk(id, value));
            }
            body.extend(dff_chunk(b"DIIN", &info));
        }
        if let Some(id3) = id3 {
            body.extend(dff_chunk(b"ID3 ", id3));
        }
        let form_len = 4 + body.len();
        let mut bytes = b"FRM8".to_vec();
        bytes.extend_from_slice(&(form_len as u64).to_be_bytes());
        bytes.extend_from_slice(b"DSD ");
        bytes.extend(body);
        bytes
    }

    fn wav_with_info(fields: &[(&[u8; 4], &str)]) -> Vec<u8> {
        let mut bytes = b"WAVE".to_vec();
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        fmt.extend_from_slice(&2_u16.to_le_bytes());
        fmt.extend_from_slice(&44_100_u32.to_le_bytes());
        fmt.extend_from_slice(&176_400_u32.to_le_bytes());
        fmt.extend_from_slice(&4_u16.to_le_bytes());
        fmt.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend(chunk(b"fmt ", &fmt));

        let mut samples = vec![0_u8; 4_410 * 4];
        // A non-empty data chunk is required for duration/properties.
        samples[0] = 1;
        bytes.extend(chunk(b"data", &samples));

        if !fields.is_empty() {
            let mut info = b"INFO".to_vec();
            for (id, value) in fields {
                let mut text = value.as_bytes().to_vec();
                text.push(0);
                info.extend(chunk(id, &text));
            }
            bytes.extend(chunk(b"LIST", &info));
        }

        let length = bytes.len() as u32 + 4;
        let mut riff = b"RIFF".to_vec();
        riff.extend(length.to_le_bytes());
        riff.extend(bytes);
        riff
    }

    #[test]
    fn aiff_id3_metadata_wins_over_conflicting_text_chunks_and_keeps_properties() {
        let bytes = synthetic_aiff(
            &[
                (ItemKey::TrackTitle, "ID3 title"),
                (ItemKey::TrackArtist, "ID3 artist"),
                (ItemKey::AlbumTitle, "ID3 album"),
                (ItemKey::Genre, "Jazz"),
                (ItemKey::RecordingDate, "2001-03"),
                (ItemKey::TrackNumber, "3"),
                (ItemKey::DiscNumber, "2"),
            ],
            &[(b"NAME", "Chunk title"), (b"AUTH", "Chunk artist")],
            false,
        );
        let attempt = parse_aiff(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "aiff_pcm");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("ID3 title"));
        assert_eq!(values.track_artists, ["ID3 artist"]);
        assert_eq!(values.album.as_deref(), Some("ID3 album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2001-03"));
        assert_eq!(values.track_number, Some(3));
        assert_eq!(values.disc_number, Some(2));
        assert_eq!(values.duration_ms, Some(100));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
    }

    #[test]
    fn aiff_text_chunks_are_fallback_and_unicode_id3_is_preserved() {
        let fallback = parse_aiff(Cursor::new(synthetic_aiff(
            &[],
            &[(b"NAME", "Chunk 東京"), (b"AUTH", "Autor")],
            false,
        )));
        assert_eq!(fallback.status, AttemptStatus::ParsedWithTags);
        let values = fallback.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Chunk 東京"));
        assert_eq!(values.track_artists, ["Autor"]);

        let unicode = parse_aiff(Cursor::new(synthetic_aiff(
            &[(ItemKey::TrackTitle, "Canção 東京")],
            &[],
            false,
        )));
        assert_eq!(
            unicode.values.unwrap().title.as_deref(),
            Some("Canção 東京")
        );
    }

    #[test]
    fn aiff_partial_id3_falls_back_per_field_and_keeps_distinct_artist_items() {
        let bytes = synthetic_aiff(
            &[
                (ItemKey::TrackTitle, "ID3 title"),
                (ItemKey::TrackArtist, "Artist One"),
                (ItemKey::TrackArtist, "Artist Two"),
            ],
            &[(b"AUTH", "Fallback artist"), (b"NAME", "Fallback title")],
            false,
        );
        let attempt = parse_aiff(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("ID3 title"));
        assert_eq!(values.track_artists, ["Artist One", "Artist Two"]);
        assert!(values.album.is_none());
    }

    #[test]
    fn aiff_without_tags_keeps_properties_and_aifc_is_recognized_as_unsupported() {
        let untagged = parse_aiff(Cursor::new(synthetic_aiff(&[], &[], false)));
        assert_eq!(untagged.status, AttemptStatus::ParsedWithoutTags);
        let values = untagged.values.unwrap();
        assert_eq!(values.duration_ms, Some(100));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));

        let aifc = parse_aiff(Cursor::new(synthetic_aiff(&[], &[], true)));
        assert_eq!(aifc.status, AttemptStatus::Unsupported);
        assert_eq!(aifc.format, "aifc_unsupported");
        assert!(aifc.values.is_none());

        let corrupt = parse_aiff(Cursor::new(b"FORM truncated AIFF".to_vec()));
        assert_eq!(corrupt.status, AttemptStatus::Failed);
    }

    #[test]
    fn embedded_front_cover_is_extracted_from_an_opened_aiff_reader_with_a_hard_output_cap() {
        use lofty::{
            config::WriteOptions,
            picture::{MimeType, Picture, PictureType},
            tag::{TagExt, TagType},
        };

        let png = vec![
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00,
            0x00, 0xb5, 0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xda, 0x63, 0xfc, 0xff, 0x1f, 0x00, 0x03, 0x03, 0x02, 0x00, 0xef, 0xa3, 0x3d, 0x50,
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(
            Picture::unchecked(png.clone())
                .mime_type(MimeType::Png)
                .pic_type(PictureType::CoverFront)
                .build(),
        );
        let id3: lofty::id3::v2::Id3v2Tag = tag.into();
        let mut id3_bytes = Vec::new();
        id3.dump_to(&mut id3_bytes, WriteOptions::default())
            .unwrap();

        let mut aiff = synthetic_aiff(&[], &[], false);
        aiff.extend_from_slice(b"ID3 ");
        aiff.extend_from_slice(&(id3_bytes.len() as u32).to_be_bytes());
        aiff.extend_from_slice(&id3_bytes);
        if id3_bytes.len() % 2 != 0 {
            aiff.push(0);
        }
        let form_len = (aiff.len() - 8) as u32;
        aiff[4..8].copy_from_slice(&form_len.to_be_bytes());

        let artwork = extract_embedded_artwork(Cursor::new(aiff.clone()), png.len()).unwrap();
        assert_eq!(artwork.mime_type, "image/png");
        assert_eq!(artwork.bytes, png);
        assert!(extract_embedded_artwork(Cursor::new(aiff), 4).is_none());
    }

    #[test]
    fn embedded_art_extraction_rejects_unrecognized_or_unsupported_file_content() {
        assert!(extract_embedded_artwork(Cursor::new(b"not audio"), 1024).is_none());
        let speex = include_bytes!("fixtures/synthetic-speex.ogg");
        assert!(extract_embedded_artwork(Cursor::new(speex.as_slice()), 1024).is_none());
    }

    #[test]
    fn embedded_art_dimensions_reject_decompression_bombs_and_malformed_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&5000_u32.to_be_bytes());
        png.extend_from_slice(&1_u32.to_be_bytes());
        assert_eq!(embedded_image_mime_type(&png), None);

        let oversized_jpeg = [
            0xff, 0xd8, 0xff, 0xc0, 0x00, 0x07, 0x08, 0x13, 0x89, 0x00, 0x01, 0x00, 0x01,
        ];
        assert_eq!(embedded_image_mime_type(&oversized_jpeg), None);
        assert_eq!(embedded_image_mime_type(b"\x89PNG\r\n\x1a\nshort"), None);
    }

    #[test]
    fn embedded_art_accepts_bounded_jpeg_and_webp_dimensions() {
        let jpeg = [
            0xff, 0xd8, 0xff, 0xc0, 0x00, 0x07, 0x08, 0x00, 0x01, 0x00, 0x01,
        ];
        assert_eq!(embedded_image_mime_type(&jpeg), Some("image/jpeg"));

        let mut webp = vec![0_u8; 30];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        webp[12..16].copy_from_slice(b"VP8X");
        assert_eq!(embedded_image_mime_type(&webp), Some("image/webp"));
        webp[26] = 1; // Width 65,537, rejected before it reaches the WebView.
        assert_eq!(embedded_image_mime_type(&webp), None);
    }

    #[test]
    fn mp4_aac_and_alac_tags_and_properties_are_extracted_without_artwork() {
        let aac = synthetic_mp4(
            include_bytes!("fixtures/synthetic-aac.m4a"),
            &[
                (ItemKey::TrackTitle, "Canção 東京"),
                (ItemKey::TrackArtist, "Track One"),
                (ItemKey::TrackArtist, "Track Two"),
                (ItemKey::AlbumArtist, "Album Artist"),
                (ItemKey::AlbumTitle, "AAC Album"),
                (ItemKey::Genre, "Jazz"),
                (ItemKey::RecordingDate, "2024-06"),
                (ItemKey::TrackNumber, "3/12"),
                (ItemKey::DiscNumber, "2/2"),
            ],
        );
        let attempt = parse_mp4(Cursor::new(aac));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "mp4_aac");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Canção 東京"));
        assert_eq!(values.track_artists, ["Track One", "Track Two"]);
        assert_eq!(values.album_artists, ["Album Artist"]);
        assert_eq!(values.album.as_deref(), Some("AAC Album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2024-06"));
        assert_eq!(values.track_number, Some(3));
        assert_eq!(values.disc_number, Some(2));
        // Lofty's MP4 duration comes from the raw audio mdhd. For AAC this
        // includes the encoder priming interval (ffprobe's edit-list-trimmed
        // presentation duration is 150 ms for this fixture).
        assert_eq!(values.duration_ms, Some(173));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));

        let alac = parse_mp4(Cursor::new(
            include_bytes!("fixtures/synthetic-alac.m4a").to_vec(),
        ));
        assert_eq!(alac.status, AttemptStatus::ParsedWithoutTags);
        assert_eq!(alac.format, "mp4_alac");
        let values = alac.values.unwrap();
        assert_eq!(values.duration_ms, Some(150));
        assert_eq!(values.sample_rate, Some(48_000));
        assert_eq!(values.channels, Some(2));
        assert!(values.title.is_none());
    }

    #[test]
    fn mp4_partial_tags_are_preserved_and_invalid_files_are_failures() {
        let partial = synthetic_mp4(
            include_bytes!("fixtures/synthetic-alac.m4a"),
            &[(ItemKey::TrackTitle, "Partial ALAC")],
        );
        let attempt = parse_mp4(Cursor::new(partial));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "mp4_alac");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Partial ALAC"));
        assert!(values.track_artists.is_empty());
        assert_eq!(values.sample_rate, Some(48_000));

        assert_eq!(
            parse_mp4(Cursor::new(b"not an MP4".to_vec())).status,
            AttemptStatus::Failed
        );
        assert_eq!(
            parse_mp4(Cursor::new(b"\0\0\0\x10ftypM4A ".to_vec())).status,
            AttemptStatus::Failed
        );
    }

    #[test]
    fn parses_pcm_wav_riff_info_and_technical_properties_without_art() {
        let bytes = wav_with_info(&[
            (b"INAM", "Test title"),
            (b"IART", "Test artist"),
            (b"IPRD", "Test album"),
            (b"IGNR", "Jazz"),
            (b"ICRD", "1999-04-03"),
            (b"IPRT", "7"),
        ]);
        let attempt = parse_wav(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "wav_pcm");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Test title"));
        assert_eq!(
            values.track_artists.first().map(String::as_str),
            Some("Test artist")
        );
        assert_eq!(values.album.as_deref(), Some("Test album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("1999-04-03"));
        assert_eq!(values.track_number, Some(7));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert_eq!(values.duration_ms, Some(100));
    }

    #[test]
    fn partial_and_unicode_tags_are_preserved_as_text() {
        let bytes = wav_with_info(&[(b"INAM", "Canção 東京")]);
        let attempt = parse_wav(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(
            attempt.values.unwrap().title.as_deref(),
            Some("Canção 東京")
        );
    }

    #[test]
    fn valid_wav_without_recognized_tags_keeps_technical_properties() {
        let attempt = parse_wav(Cursor::new(wav_with_info(&[])));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithoutTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert_eq!(values.duration_ms, Some(100));
    }

    #[test]
    fn malformed_wav_is_a_recorded_parse_failure() {
        let attempt = parse_wav(Cursor::new(b"not a WAV".to_vec()));
        assert_eq!(attempt.status, AttemptStatus::Failed);
        assert!(attempt.error.is_some());
    }

    #[test]
    fn wavpack_apetag_metadata_and_properties_are_read_without_artwork() {
        let bytes = synthetic_wavpack();
        let attempt = parse_wavpack(Cursor::new(bytes.clone()));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "wavpack");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Fixture Track"));
        assert_eq!(values.track_artists, ["Fixture Artist"]);
        assert_eq!(values.album_artists, ["Fixture Album Artist"]);
        assert_eq!(values.album.as_deref(), Some("Fixture Album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2024"));
        assert_eq!(values.track_number, Some(2));
        assert_eq!(values.disc_number, Some(1));
        assert_eq!(values.sample_rate, Some(8_000));
        assert_eq!(values.channels, Some(1));
        assert_eq!(values.duration_ms, Some(2_000));

        let block_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8;
        let untagged = parse_wavpack(Cursor::new(bytes[..block_size].to_vec()));
        assert_eq!(untagged.status, AttemptStatus::ParsedWithoutTags);
        assert_eq!(untagged.values.unwrap().sample_rate, Some(8_000));

        let mut oversized = bytes;
        let footer = oversized.len() - 32;
        oversized[footer + 12..footer + 16]
            .copy_from_slice(&((MAX_APE_TAG_BYTES as u32) + 1).to_le_bytes());
        assert_eq!(parse_wavpack(Cursor::new(oversized)).status, AttemptStatus::Failed);
        assert_eq!(
            parse_wavpack(Cursor::new(b"wvpk truncated".to_vec())).status,
            AttemptStatus::Failed
        );
    }

    #[test]
    fn flac_vorbis_comments_parse_multiple_artists_and_album_artist_separately() {
        let bytes = synthetic_flac(&[
            ("TITLE", "Synthetic FLAC"),
            ("ARTIST", "Track Artist One"),
            ("ARTIST", "Track Artist Two"),
            ("ALBUMARTIST", "Album Artist One"),
            ("ALBUMARTIST", "Album Artist Two"),
            ("ALBUM", "Synthetic Album"),
            ("GENRE", "Jazz"),
            ("DATE", "2001-03"),
            ("TRACKNUMBER", "3/9"),
            ("DISCNUMBER", "2"),
        ]);
        let attempt = parse_flac(std::io::Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "flac");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Synthetic FLAC"));
        assert_eq!(
            values.track_artists,
            ["Track Artist One", "Track Artist Two"]
        );
        assert_eq!(
            values.album_artists,
            ["Album Artist One", "Album Artist Two"]
        );
        assert_eq!(values.track_artists[0], "Track Artist One");
        assert_eq!(values.album_artists[0], "Album Artist One");
        assert_eq!(values.album.as_deref(), Some("Synthetic Album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2001-03"));
        assert_eq!(values.track_number, Some(3));
        assert_eq!(values.disc_number, Some(2));
        assert_eq!(values.duration_ms, Some(100));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
    }

    #[test]
    fn flac_partial_and_unicode_comments_are_preserved() {
        let bytes = synthetic_flac(&[("TITLE", "Canção 東京")]);
        let attempt = parse_flac(std::io::Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Canção 東京"));
        assert!(values.track_artists.is_empty());
        assert_eq!(values.sample_rate, Some(44_100));
    }

    #[test]
    fn valid_flac_without_vorbis_comments_keeps_properties() {
        let attempt = parse_flac(std::io::Cursor::new(synthetic_flac(&[])));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithoutTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.duration_ms, Some(100));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
    }

    #[test]
    fn corrupt_and_non_flac_payloads_are_not_classified_as_untagged() {
        let corrupt = parse_flac(std::io::Cursor::new(b"fLaC truncated".to_vec()));
        assert_eq!(corrupt.status, AttemptStatus::Failed);
        assert!(corrupt.error.is_some());
        let wrong = parse_flac(std::io::Cursor::new(b"not a flac".to_vec()));
        assert_eq!(wrong.status, AttemptStatus::Failed);
    }

    #[test]
    fn ogg_vorbis_comments_preserve_fields_unicode_and_multiple_artists() {
        let base = include_bytes!("fixtures/synthetic-vorbis.ogg");
        let bytes = synthetic_ogg(
            base,
            &[
                (ItemKey::TrackTitle, "Canção 東京"),
                (ItemKey::TrackArtist, "Artista um"),
                (ItemKey::TrackArtist, "Artista dois"),
                (ItemKey::AlbumArtist, "Artista do álbum"),
                (ItemKey::AlbumArtist, "Convidada"),
                (ItemKey::AlbumTitle, "Disco sintético"),
                (ItemKey::Genre, "Jazz"),
                (ItemKey::RecordingDate, "2024-05"),
                (ItemKey::TrackNumber, "4/8"),
                (ItemKey::DiscNumber, "2"),
            ],
        );

        let attempt = parse_ogg(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "ogg_vorbis");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Canção 東京"));
        assert_eq!(values.track_artists, ["Artista um", "Artista dois"]);
        assert_eq!(values.album_artists, ["Artista do álbum", "Convidada"]);
        assert_eq!(values.album.as_deref(), Some("Disco sintético"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2024-05"));
        assert_eq!(values.track_number, Some(4));
        assert_eq!(values.disc_number, Some(2));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert!((200..=300).contains(&values.duration_ms.unwrap()));
    }

    #[test]
    fn ogg_vorbis_partial_and_untagged_files_keep_only_available_properties() {
        let base = include_bytes!("fixtures/synthetic-vorbis.ogg");
        let partial = parse_ogg(Cursor::new(synthetic_ogg(
            base,
            &[(ItemKey::TrackTitle, "Partial title")],
        )));
        assert_eq!(partial.status, AttemptStatus::ParsedWithTags);
        let values = partial.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Partial title"));
        assert!(values.track_artists.is_empty());

        let untagged = parse_ogg(Cursor::new(base.to_vec()));
        assert_eq!(untagged.status, AttemptStatus::ParsedWithoutTags);
        let values = untagged.values.unwrap();
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert!((200..=300).contains(&values.duration_ms.unwrap()));
    }

    #[test]
    fn opus_comments_and_properties_use_declared_input_rate_not_decode_clock() {
        let base = include_bytes!("fixtures/synthetic-opus.opus");
        let bytes = synthetic_ogg(
            base,
            &[
                (ItemKey::TrackTitle, "Opus sintético"),
                (ItemKey::TrackArtist, "Voz A"),
                (ItemKey::TrackArtist, "Voz B"),
                (ItemKey::AlbumArtist, "Projeto"),
                (ItemKey::AlbumTitle, "Teste"),
                (ItemKey::Genre, "Speech"),
                (ItemKey::RecordingDate, "2025"),
                (ItemKey::TrackNumber, "1"),
                (ItemKey::DiscNumber, "1"),
            ],
        );
        let attempt = parse_ogg(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "ogg_opus");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Opus sintético"));
        assert_eq!(values.track_artists, ["Voz A", "Voz B"]);
        assert_eq!(values.album_artists, ["Projeto"]);
        assert_eq!(values.album.as_deref(), Some("Teste"));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert!((200..=300).contains(&values.duration_ms.unwrap()));
    }

    #[test]
    fn opus_partial_and_untagged_files_remain_distinct() {
        let base = include_bytes!("fixtures/synthetic-opus.opus");
        let partial = parse_ogg(Cursor::new(synthetic_ogg(
            base,
            &[(ItemKey::TrackTitle, "Opus partial")],
        )));
        assert_eq!(partial.status, AttemptStatus::ParsedWithTags);
        assert_eq!(
            partial.values.unwrap().title.as_deref(),
            Some("Opus partial")
        );

        let untagged = parse_ogg(Cursor::new(base.to_vec()));
        assert_eq!(untagged.status, AttemptStatus::ParsedWithoutTags);
        let values = untagged.values.unwrap();
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert!((200..=300).contains(&values.duration_ms.unwrap()));
    }

    #[test]
    fn ogg_probe_rejects_corrupt_streams_and_does_not_claim_speex_or_other_extensions() {
        let corrupt = include_bytes!("fixtures/synthetic-opus.opus")[..45].to_vec();
        assert_eq!(
            parse_ogg(Cursor::new(corrupt)).status,
            AttemptStatus::Failed
        );

        let speex = include_bytes!("fixtures/synthetic-speex.ogg");
        let unsupported = parse_ogg(Cursor::new(speex.to_vec()));
        assert_eq!(unsupported.status, AttemptStatus::Unsupported);
        assert_eq!(unsupported.format, "ogg_speex");

        let wrong_container = synthetic_mp3(&[], false);
        let mismatch = parse_ogg(Cursor::new(wrong_container));
        assert_eq!(mismatch.status, AttemptStatus::Unsupported);
        assert_eq!(mismatch.format, "ogg_extension_contains_mpeg");

        let invalid = parse_ogg(Cursor::new(b"not Ogg or audio".to_vec()));
        assert_eq!(invalid.status, AttemptStatus::Failed);
    }

    #[test]
    fn mp3_id3v23_and_id3v24_are_parsed_from_an_open_reader() {
        for (id3v23, expected_version) in [(true, 3_u8), (false, 4_u8)] {
            let bytes = synthetic_mp3(
                &[(TagType::Id3v2, &[(ItemKey::TrackTitle, "Synthetic title")])],
                id3v23,
            );
            assert_eq!(&bytes[..3], b"ID3");
            assert_eq!(bytes[3], expected_version);
            let attempt = parse_mp3(Cursor::new(bytes));
            assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
            assert_eq!(attempt.format, "mp3");
            assert_eq!(
                attempt.values.unwrap().title.as_deref(),
                Some("Synthetic title")
            );
        }
    }

    #[test]
    fn mp3_id3v1_is_used_when_id3v2_is_absent() {
        let bytes = synthetic_mp3(
            &[(
                TagType::Id3v1,
                &[
                    (ItemKey::TrackTitle, "Legacy title"),
                    (ItemKey::TrackArtist, "Legacy artist"),
                    (ItemKey::AlbumTitle, "Legacy album"),
                ],
            )],
            false,
        );
        let attempt = parse_mp3(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Legacy title"));
        assert_eq!(values.track_artists, ["Legacy artist"]);
        assert_eq!(values.album.as_deref(), Some("Legacy album"));
    }

    #[test]
    fn mp3_uses_one_tag_source_with_id3v2_precedence_over_id3v1() {
        let bytes = synthetic_mp3(
            &[
                (
                    TagType::Id3v1,
                    &[
                        (ItemKey::TrackTitle, "Old title"),
                        (ItemKey::TrackArtist, "Old artist"),
                    ],
                ),
                (TagType::Id3v2, &[(ItemKey::TrackTitle, "Current title")]),
            ],
            false,
        );
        let attempt = parse_mp3(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Current title"));
        assert!(values.track_artists.is_empty());
    }

    #[test]
    fn mp3_uses_lofty_ape_tags_when_id3_tags_are_absent() {
        let bytes = synthetic_mp3(
            &[(
                TagType::Ape,
                &[
                    (ItemKey::TrackTitle, "APE title"),
                    (ItemKey::TrackArtist, "APE artist"),
                ],
            )],
            false,
        );
        let attempt = parse_mp3(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("APE title"));
        assert_eq!(values.track_artists, ["APE artist"]);
    }

    #[test]
    fn mp3_extracts_repeated_artist_values_unicode_and_mpeg_properties() {
        let bytes = synthetic_mp3(
            &[(
                TagType::Id3v2,
                &[
                    (ItemKey::TrackTitle, "Canção 東京"),
                    (ItemKey::TrackArtist, "Track One"),
                    (ItemKey::TrackArtist, "Track Two"),
                    (ItemKey::AlbumArtist, "Album Artist"),
                    (ItemKey::AlbumTitle, "Synthetic Album"),
                    (ItemKey::Genre, "Jazz"),
                    (ItemKey::RecordingDate, "2001-03"),
                    (ItemKey::TrackNumber, "7/14"),
                    (ItemKey::DiscNumber, "2"),
                ],
            )],
            false,
        );
        let attempt = parse_mp3(Cursor::new(bytes));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Canção 東京"));
        assert_eq!(values.track_artists, ["Track One", "Track Two"]);
        assert_eq!(values.album_artists, ["Album Artist"]);
        assert_eq!(values.album.as_deref(), Some("Synthetic Album"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("2001-03"));
        assert_eq!(values.track_number, Some(7));
        assert_eq!(values.disc_number, Some(2));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));
        assert!(values.duration_ms.is_some_and(|duration| duration > 0));
    }

    #[test]
    fn valid_untagged_mp3_keeps_properties_and_invalid_mp3_fails() {
        let untagged = parse_mp3(Cursor::new(synthetic_mp3(&[], false)));
        assert_eq!(untagged.status, AttemptStatus::ParsedWithoutTags);
        let values = untagged.values.unwrap();
        assert!(values.duration_ms.is_some_and(|duration| duration > 0));
        assert_eq!(values.sample_rate, Some(44_100));
        assert_eq!(values.channels, Some(2));

        for invalid in [b"not an mp3".to_vec(), b"ID3 truncated".to_vec()] {
            let attempt = parse_mp3(Cursor::new(invalid));
            assert_eq!(attempt.status, AttemptStatus::Failed);
            assert!(attempt.error.is_some());
        }
    }

    #[test]
    fn parses_dsf_id3_through_header_pointer_and_rejects_invalid_bounds() {
        let mut id3 = synthetic_id3(&[
            (ItemKey::TrackTitle, "Abacab 東京"),
            (ItemKey::TrackArtist, "Genesis"),
            (ItemKey::TrackArtist, "Guest Voice"),
            (ItemKey::AlbumArtist, "Genesis"),
            (ItemKey::AlbumTitle, "Abacab"),
            (ItemKey::Genre, "Other"),
            (ItemKey::RecordingDate, "2025-12-05"),
            (ItemKey::TrackNumber, "1/9"),
            (ItemKey::DiscNumber, "2/3"),
        ]);
        let date = id3
            .windows(10)
            .position(|window| window == b"2025-12-05")
            .unwrap();
        id3[date..date + 10].copy_from_slice(b"2025-22-05");
        let attempt = parse_dsf(Cursor::new(synthetic_dsf(Some(&id3), 128 * 1024)));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Abacab 東京"));
        assert_eq!(values.track_artists, ["Genesis", "Guest Voice"]);
        assert_eq!(values.album_artists, ["Genesis"]);
        assert_eq!(values.album.as_deref(), Some("Abacab"));
        assert_eq!(values.genre.as_deref(), Some("Other"));
        // Lofty rejects the invalid calendar value as a normalized date; it
        // remains a non-fatal tag, while other ID3 fields are retained.
        assert_eq!(values.date_text.as_deref(), Some("2025-22-05"));
        assert_eq!(values.track_number, Some(1));
        assert_eq!(values.disc_number, Some(2));

        let untagged = parse_dsf(Cursor::new(synthetic_dsf(None, 16)));
        assert_eq!(untagged.status, AttemptStatus::ParsedWithoutTags);
        assert!(untagged.values.unwrap().track_artists.is_empty());

        let mut bad_offset = synthetic_dsf(Some(&id3), 0);
        bad_offset[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            parse_dsf(Cursor::new(bad_offset)).status,
            AttemptStatus::Failed
        );

        let mut bad_id3 = id3.clone();
        bad_id3[6] |= 0x80;
        assert_eq!(
            parse_dsf(Cursor::new(synthetic_dsf(Some(&bad_id3), 0))).status,
            AttemptStatus::Failed
        );
        assert_eq!(
            parse_dsf(Cursor::new(b"DSD " as &[u8])).status,
            AttemptStatus::Failed
        );

        let mut oversized_tag = synthetic_dsf(Some(&id3), 0);
        let tag_offset = u64::from_le_bytes(oversized_tag[20..28].try_into().unwrap()) as usize;
        let excessive = (MAX_ID3_BYTES as u32 + 1).min(0x0fff_ffff);
        oversized_tag[tag_offset + 6..tag_offset + 10].copy_from_slice(&[
            ((excessive >> 21) & 0x7f) as u8,
            ((excessive >> 14) & 0x7f) as u8,
            ((excessive >> 7) & 0x7f) as u8,
            (excessive & 0x7f) as u8,
        ]);
        assert_eq!(
            parse_dsf(Cursor::new(oversized_tag)).status,
            AttemptStatus::Failed
        );

        struct CountReads<R> {
            inner: R,
            bytes: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl<R: Read> Read for CountReads<R> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let count = self.inner.read(out)?;
                self.bytes.set(self.bytes.get() + count);
                Ok(count)
            }
        }
        impl<R: Seek> Seek for CountReads<R> {
            fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
                self.inner.seek(from)
            }
        }
        let bytes_read = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = CountReads {
            inner: Cursor::new(synthetic_dsf(Some(&id3), 2 * 1024 * 1024)),
            bytes: bytes_read.clone(),
        };
        let attempt = parse_dsf(counted);
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert!(attempt.duration_us > 0);
        assert!(bytes_read.get() < 4096, "read {} bytes", bytes_read.get());
    }

    #[test]
    fn legacy_id3_dates_keep_year_and_compose_only_valid_ddmm_pairs() {
        let invalid_v23 = synthetic_legacy_id3_date(3, Some("2025"), Some("0522"));
        assert_eq!(raw_id3_date(&invalid_v23).as_deref(), Some("2025"));
        let valid_v23 = synthetic_legacy_id3_date(3, Some("2024"), Some("2902"));
        assert_eq!(raw_id3_date(&valid_v23).as_deref(), Some("2024-02-29"));
        let valid_v24_legacy = synthetic_legacy_id3_date(4, Some("2025"), Some("2205"));
        assert_eq!(
            raw_id3_date(&valid_v24_legacy).as_deref(),
            Some("2025-05-22")
        );
        let partial = synthetic_legacy_id3_date(3, Some("2025"), None);
        assert_eq!(raw_id3_date(&partial).as_deref(), Some("2025"));
        let absent = synthetic_legacy_id3_date(3, None, None);
        assert_eq!(raw_id3_date(&absent), None);
        let day_without_year = synthetic_legacy_id3_date(3, None, Some("2205"));
        assert_eq!(raw_id3_date(&day_without_year), None);

        // Both DSD containers use this same ID3 date interpretation.
        let dsf = parse_dsf(Cursor::new(synthetic_dsf(Some(&invalid_v23), 0)));
        assert_eq!(dsf.values.unwrap().date_text.as_deref(), Some("2025"));
        let dff = parse_dff(Cursor::new(synthetic_dff(&[], Some(&valid_v23))));
        assert_eq!(dff.values.unwrap().date_text.as_deref(), Some("2024-02-29"));
    }

    #[test]
    fn parses_dff_native_fields_and_id3_without_inventing_album_metadata() {
        let native = synthetic_dff(&[(b"DIAR", "Björk"), (b"DITI", "Jóga")], None);
        let parsed = parse_dff(Cursor::new(native));
        assert_eq!(parsed.status, AttemptStatus::ParsedWithTags);
        let values = parsed.values.unwrap();
        assert_eq!(values.track_artists, ["Björk"]);
        assert_eq!(values.title.as_deref(), Some("Jóga"));
        assert!(values.album_artists.is_empty());
        assert!(values.album.is_none());

        let mut id3 = synthetic_id3(&[
            (ItemKey::TrackTitle, "ID3 title"),
            (ItemKey::TrackArtist, "Artist one"),
            (ItemKey::TrackArtist, "Artist two"),
            (ItemKey::AlbumArtist, "Album artist"),
            (ItemKey::AlbumTitle, "Album name"),
            (ItemKey::RecordingDate, "2025-12-05"),
            (ItemKey::TrackNumber, "4"),
        ]);
        // Keep the ID3 frame structure valid while preserving a deliberately
        // non-calendar date string as tag text.
        let date = id3
            .windows(10)
            .position(|window| window == b"2025-12-05")
            .unwrap();
        id3[date..date + 10].copy_from_slice(b"2025-22-05");
        let with_id3 = parse_dff(Cursor::new(synthetic_dff(
            &[(b"DIAR", "Native fallback"), (b"DITI", "Native title")],
            Some(&id3),
        )));
        assert_eq!(with_id3.status, AttemptStatus::ParsedWithTags);
        let values = with_id3.values.unwrap();
        assert_eq!(values.track_artists, ["Artist one", "Artist two"]);
        assert_eq!(values.title.as_deref(), Some("ID3 title"));
        assert_eq!(values.album_artists, ["Album artist"]);
        assert_eq!(values.album.as_deref(), Some("Album name"));
        assert_eq!(values.date_text.as_deref(), Some("2025-22-05"));
        assert_eq!(values.track_number, Some(4));

        assert_eq!(
            parse_dff(Cursor::new(synthetic_dff(&[], None))).status,
            AttemptStatus::ParsedWithoutTags
        );
    }

    #[test]
    fn aac_adts_reads_bounded_id3_metadata_without_scanning_audio_payload() {
        let id3 = synthetic_id3(&[
            (ItemKey::TrackTitle, "Ágætis byrjun"),
            (ItemKey::TrackArtist, "Artist one"),
            (ItemKey::TrackArtist, "Artist two"),
            (ItemKey::AlbumArtist, "Album artist"),
            (ItemKey::AlbumTitle, "Album"),
            (ItemKey::Genre, "Electronic"),
            (ItemKey::RecordingDate, "1999"),
            (ItemKey::TrackNumber, "2/11"),
            (ItemKey::DiscNumber, "1/2"),
        ]);
        let attempt = parse_aac(Cursor::new(synthetic_aac(Some(&id3), 2)));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        assert_eq!(attempt.format, "aac_adts");
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Ágætis byrjun"));
        assert_eq!(values.track_artists, ["Artist one", "Artist two"]);
        assert_eq!(values.album_artists, ["Album artist"]);
        assert_eq!(values.album.as_deref(), Some("Album"));
        assert_eq!(values.genre.as_deref(), Some("Electronic"));
        assert_eq!(values.date_text.as_deref(), Some("1999"));
        assert_eq!(values.track_number, Some(2));
        assert_eq!(values.disc_number, Some(1));

        let partial = synthetic_id3(&[(ItemKey::TrackTitle, "Only title")]);
        assert_eq!(
            parse_aac(Cursor::new(synthetic_aac(Some(&partial), 2)))
                .values
                .unwrap()
                .title
                .as_deref(),
            Some("Only title")
        );
        assert_eq!(
            parse_aac(Cursor::new(synthetic_aac(None, 2))).status,
            AttemptStatus::ParsedWithoutTags
        );

        let mut oversized = id3.clone();
        let size = (MAX_ID3_BYTES as u32 + 1).min(0x0fff_ffff);
        oversized[6..10].copy_from_slice(&[
            ((size >> 21) & 0x7f) as u8,
            ((size >> 14) & 0x7f) as u8,
            ((size >> 7) & 0x7f) as u8,
            (size & 0x7f) as u8,
        ]);
        assert_eq!(
            parse_aac(Cursor::new(synthetic_aac(Some(&oversized), 2))).status,
            AttemptStatus::Failed
        );
        assert_eq!(
            parse_aac(Cursor::new(b"ADIF unsupported payload".to_vec())).status,
            AttemptStatus::Unsupported
        );
        assert_eq!(
            parse_aac(Cursor::new(synthetic_adts_frame()[..6].to_vec())).status,
            AttemptStatus::Failed
        );

        struct CountReads<R> {
            inner: R,
            bytes: std::rc::Rc<std::cell::Cell<usize>>,
        }
        impl<R: Read> Read for CountReads<R> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let count = self.inner.read(out)?;
                self.bytes.set(self.bytes.get() + count);
                Ok(count)
            }
        }
        impl<R: Seek> Seek for CountReads<R> {
            fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
                self.inner.seek(from)
            }
        }
        let mut large_audio = synthetic_aac(Some(&id3), 2);
        large_audio.resize(16 * 1024 * 1024, 0);
        let reads = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = CountReads {
            inner: Cursor::new(large_audio),
            bytes: reads.clone(),
        };
        assert_eq!(parse_aac(counted).status, AttemptStatus::ParsedWithTags);
        assert!(reads.get() < 64 * 1024, "read {} bytes", reads.get());
    }

    #[test]
    fn legacy_id3_date_normalization_is_consistent_between_dsd_and_aac() {
        let invalid = synthetic_legacy_id3_date(3, Some("2025"), Some("0522"));
        let aac = parse_aac(Cursor::new(synthetic_aac(Some(&invalid), 2)));
        assert_eq!(
            aac.values.unwrap().date_text.as_deref(),
            Some("2025"),
            "invalid ID3v2.3 TDAT must not leak as a malformed full date"
        );

        let mut mp3_bytes = invalid.clone();
        mp3_bytes.extend_from_slice(&synthetic_mp3(&[], false));
        let mp3 = parse_mp3(Cursor::new(mp3_bytes));
        assert_eq!(mp3.values.unwrap().date_text.as_deref(), Some("2025"));

        let valid = synthetic_legacy_id3_date(3, Some("2024"), Some("2902"));
        let aac = parse_aac(Cursor::new(synthetic_aac(Some(&valid), 2)));
        assert_eq!(aac.values.unwrap().date_text.as_deref(), Some("2024-02-29"));
    }

    #[test]
    fn mp3_legacy_id3_date_preserves_year_when_tdat_is_invalid() {
        let mut bytes = synthetic_legacy_id3_date(3, Some("2025"), Some("0522"));
        bytes.extend_from_slice(&synthetic_mp3(&[], false));
        let attempt = parse_mp3(Cursor::new(bytes));
        assert_eq!(attempt.values.unwrap().date_text.as_deref(), Some("2025"));
    }

    #[test]
    fn aiff_and_aifc_legacy_id3_dates_keep_year_and_validate_complete_dates() {
        fn with_id3_chunk(mut form: Vec<u8>, id3: &[u8]) -> Vec<u8> {
            form.extend_from_slice(b"ID3 ");
            form.extend_from_slice(&(id3.len() as u32).to_be_bytes());
            form.extend_from_slice(id3);
            if id3.len() % 2 != 0 {
                form.push(0);
            }
            let form_size = (form.len() - 8) as u32;
            form[4..8].copy_from_slice(&form_size.to_be_bytes());
            form
        }

        let invalid = synthetic_legacy_id3_date(3, Some("2025"), Some("0522"));
        let aiff = parse_aiff(Cursor::new(with_id3_chunk(
            synthetic_aiff(&[], &[], false),
            &invalid,
        )));
        assert_eq!(aiff.values.unwrap().date_text.as_deref(), Some("2025"));

        let valid = synthetic_legacy_id3_date(3, Some("2024"), Some("2902"));
        let aifc = parse_aifc(Cursor::new(with_id3_chunk(
            synthetic_aiff(&[], &[], true),
            &valid,
        )));
        assert_eq!(
            aifc.values.unwrap().date_text.as_deref(),
            Some("2024-02-29")
        );
    }

    #[test]
    fn ape_v2_metadata_is_persistable_and_tag_sizes_are_bounded() {
        let bytes = synthetic_ape(&[
            ("Title", "曲名"),
            ("Artist", "Track artist"),
            ("Album Artist", "Album artist"),
            ("Album", "Record"),
            ("Genre", "Jazz"),
            ("Year", "1968"),
            ("Track", "3/10"),
            ("Disc", "2"),
        ]);
        let attempt = parse_ape(Cursor::new(bytes.clone()));
        assert_eq!(attempt.status, AttemptStatus::ParsedWithTags);
        let values = attempt.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("曲名"));
        assert_eq!(values.track_artists, ["Track artist"]);
        assert_eq!(values.album_artists, ["Album artist"]);
        assert_eq!(values.album.as_deref(), Some("Record"));
        assert_eq!(values.genre.as_deref(), Some("Jazz"));
        assert_eq!(values.date_text.as_deref(), Some("1968"));
        assert_eq!(values.track_number, Some(3));
        assert_eq!(values.disc_number, Some(2));

        assert_eq!(
            parse_ape(Cursor::new(synthetic_ape(&[]))).status,
            AttemptStatus::ParsedWithoutTags
        );
        let mut oversized = synthetic_ape(&[("Title", "Too big")]);
        let footer = oversized.len() - 32;
        oversized[footer + 12..footer + 16]
            .copy_from_slice(&((MAX_APE_TAG_BYTES as u32) + 1).to_le_bytes());
        assert_eq!(
            parse_ape(Cursor::new(oversized)).status,
            AttemptStatus::Failed
        );
        assert_eq!(
            parse_ape(Cursor::new(b"MAC truncated".to_vec())).status,
            AttemptStatus::Failed
        );

        let mut id3_only_ape = synthetic_ape(&[]);
        id3_only_ape.truncate(id3_only_ape.len() - 32);
        let legacy_date = synthetic_legacy_id3_date(3, Some("2025"), Some("0522"));
        id3_only_ape.splice(0..0, legacy_date);
        let id3_fallback = parse_ape(Cursor::new(id3_only_ape));
        assert_eq!(
            id3_fallback.values.unwrap().date_text.as_deref(),
            Some("2025")
        );
    }

    #[test]
    fn aifc_reads_tags_only_with_container_bounds_and_keeps_codec_separate() {
        let id3 = synthetic_aiff(
            &[
                (ItemKey::TrackTitle, "タイトル"),
                (ItemKey::TrackArtist, "Track artist"),
                (ItemKey::AlbumArtist, "Album artist"),
                (ItemKey::AlbumTitle, "Album"),
                (ItemKey::Genre, "Classical"),
                (ItemKey::RecordingDate, "1984"),
                (ItemKey::TrackNumber, "4"),
                (ItemKey::DiscNumber, "1"),
            ],
            &[],
            true,
        );
        let parsed = parse_aifc(Cursor::new(id3));
        assert_eq!(parsed.status, AttemptStatus::ParsedWithTags);
        assert_eq!(parsed.format, "aifc_metadata");
        let values = parsed.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("タイトル"));
        assert_eq!(values.track_artists, ["Track artist"]);
        assert_eq!(values.album_artists, ["Album artist"]);
        assert_eq!(values.album.as_deref(), Some("Album"));
        assert_eq!(values.genre.as_deref(), Some("Classical"));
        assert_eq!(values.date_text.as_deref(), Some("1984"));
        assert_eq!(values.track_number, Some(4));
        assert_eq!(values.disc_number, Some(1));
        assert!(values.duration_ms.is_none());

        let mut unknown_codec = synthetic_aiff(
            &[(ItemKey::TrackTitle, "Metadata independent from codec")],
            &[],
            true,
        );
        let codec = unknown_codec
            .windows(4)
            .position(|window| window == b"NONE")
            .unwrap();
        unknown_codec[codec..codec + 4].copy_from_slice(b"ZZZZ");
        assert_eq!(
            parse_aifc(Cursor::new(unknown_codec)).status,
            AttemptStatus::ParsedWithTags
        );

        let text_only = parse_aifc(Cursor::new(synthetic_aiff(
            &[],
            &[(b"NAME", "Piece"), (b"AUTH", "Composer")],
            true,
        )));
        assert_eq!(text_only.status, AttemptStatus::ParsedWithTags);
        let values = text_only.values.unwrap();
        assert_eq!(values.title.as_deref(), Some("Piece"));
        assert_eq!(values.track_artists, ["Composer"]);
        assert!(values.album.is_none());

        assert_eq!(
            parse_aifc(Cursor::new(synthetic_aiff(&[], &[], true))).status,
            AttemptStatus::ParsedWithoutTags
        );
        assert_eq!(
            parse_aifc(Cursor::new(synthetic_aiff(&[], &[], false))).status,
            AttemptStatus::Unsupported
        );
        let mut malformed = synthetic_aiff(&[], &[], true);
        malformed[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            parse_aifc(Cursor::new(malformed)).status,
            AttemptStatus::Failed
        );

        let too_long = "x".repeat(MAX_AIFF_TEXT_BYTES as usize + 1);
        let excessive_text = synthetic_aiff(&[], &[(b"NAME", &too_long)], true);
        assert_eq!(
            parse_aifc(Cursor::new(excessive_text)).status,
            AttemptStatus::Failed
        );
    }

    #[test]
    fn dff_skips_unknown_chunks_and_rejects_malformed_or_excessive_chunks() {
        let mut bytes = synthetic_dff(&[(b"DIAR", "Artist")], None);
        // Unknown top-level chunk is accepted and skipped without interpretation.
        let unknown = dff_chunk(b"XTRA", &[9; 101]);
        let old_form_size = u64::from_be_bytes(bytes[4..12].try_into().unwrap());
        bytes.extend_from_slice(&unknown);
        bytes[4..12].copy_from_slice(&(old_form_size + unknown.len() as u64).to_be_bytes());
        assert_eq!(
            parse_dff(Cursor::new(bytes)).status,
            AttemptStatus::ParsedWithTags
        );

        let mut malformed = synthetic_dff(&[], None);
        malformed[4..12].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            parse_dff(Cursor::new(malformed)).status,
            AttemptStatus::Failed
        );

        let mut malformed_child = synthetic_dff(&[(b"DIAR", "Artist")], None);
        let artist_chunk = malformed_child
            .windows(4)
            .position(|window| window == b"DIAR")
            .unwrap();
        malformed_child[artist_chunk + 4..artist_chunk + 12]
            .copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            parse_dff(Cursor::new(malformed_child)).status,
            AttemptStatus::Failed
        );

        let mut excessive = b"FRM8".to_vec();
        let mut body = b"DSD ".to_vec();
        body.extend(dff_chunk(b"FVER", &0x01050000u32.to_be_bytes()));
        body.extend(dff_chunk(b"PROP", b"SND "));
        for _ in 0..MAX_DFF_CHUNKS + 1 {
            body.extend(dff_chunk(b"XTRA", &[]));
        }
        body.extend(dff_chunk(b"DSD ", &[0; 4]));
        excessive.extend_from_slice(&(body.len() as u64).to_be_bytes());
        excessive.extend(body);
        assert_eq!(
            parse_dff(Cursor::new(excessive)).status,
            AttemptStatus::Failed
        );
    }

    #[test]
    fn parser_versions_are_scoped_to_format_families() {
        assert_eq!(parser_version_for_extension("mp3"), MP3_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("MP3"), MP3_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("wav"), PARSER_VERSION);
        assert_eq!(parser_version_for_extension("flac"), PARSER_VERSION);
        assert_eq!(parser_version_for_extension("ogg"), OGG_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("oga"), OGG_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("opus"), OGG_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("OPUS"), OGG_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("aif"), AIFF_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("AIFF"), AIFF_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("aifc"), AIFC_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("aac"), AAC_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("ape"), APE_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("wv"), WAVPACK_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("WV"), WAVPACK_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("wma"), PARSER_VERSION);
        assert!(MP3_PARSER_VERSION > 3);
        assert!(AIFF_PARSER_VERSION > 1);
        assert!(DSF_PARSER_VERSION > PARSER_VERSION);
        assert!(DFF_PARSER_VERSION > PARSER_VERSION);
        assert!(AIFC_PARSER_VERSION > 3);
        assert!(AAC_PARSER_VERSION > 3);
        assert!(APE_PARSER_VERSION > 3);
        assert!(WAVPACK_PARSER_VERSION > PARSER_VERSION);
        assert_eq!(parser_version_for_extension("m4a"), MP4_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("MP4"), MP4_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("dsf"), DSF_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("DSF"), DSF_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("dff"), DFF_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("DFF"), DFF_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("ogg"), OGG_PARSER_VERSION);
    }
}
