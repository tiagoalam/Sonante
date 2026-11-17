//! WAV PCM, AIFF PCM, FLAC, MP3, Ogg Vorbis/Opus, and MP4 AAC/ALAC metadata extraction. Files are opened by the secure
//! scanner before they reach this module; this module never opens paths itself.

use lofty::{
    config::{ParseOptions, ParsingMode},
    file::{AudioFile, FileType, TaggedFile, TaggedFileExt},
    flac::FlacFile,
    iff::aiff::AiffFile,
    iff::wav::{WavFile, WavFormat},
    mp4::{Mp4Codec, Mp4File},
    mpeg::{Layer, MpegFile},
    probe::Probe,
    tag::{Accessor, ItemKey, Tag, TagType},
};
use std::{
    io::{Read, Seek, SeekFrom},
    time::Instant,
};

pub(crate) const PARSER_VERSION: i64 = 2;
pub(crate) const MP3_PARSER_VERSION: i64 = 3;
pub(crate) const OGG_PARSER_VERSION: i64 = 1;
pub(crate) const AIFF_PARSER_VERSION: i64 = 1;
pub(crate) const MP4_PARSER_VERSION: i64 = 1;

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
    } else {
        PARSER_VERSION
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

/// Parse classic uncompressed AIFF. AIFF-C is deliberately recognized but not
/// treated as PCM: its compression variants need a separate support policy.
pub(crate) fn parse_aiff<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
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
                let id3 = tagged.tag(TagType::Id3v2);
                let text_chunks = tagged.tag(TagType::AiffText);

                // ID3v2 wins per field; AIFF NAME/AUTH chunks fill only fields
                // absent from ID3v2. Values from conflicting sources are never
                // concatenated. AIFF text chunks do not define album/date/etc.
                values.title = id3
                    .and_then(|tag| text(tag.get_string(ItemKey::TrackTitle)))
                    .or_else(|| {
                        text_chunks.and_then(|tag| text(tag.get_string(ItemKey::TrackTitle)))
                    });
                values.track_artists = id3
                    .map(|tag| {
                        let mut artists = strings(tag, ItemKey::TrackArtist);
                        artists.extend(strings(tag, ItemKey::TrackArtists));
                        artists
                    })
                    .filter(|artists| !artists.is_empty())
                    .or_else(|| {
                        text_chunks
                            .and_then(|tag| strings(tag, ItemKey::TrackArtist).into_iter().next())
                            .map(|artist| vec![artist])
                    })
                    .unwrap_or_default();
                if let Some(tag) = id3 {
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

pub(crate) fn parse_mp3<R: Read + Seek>(reader: R) -> Attempt {
    let started = Instant::now();
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
                if let Some(tag) = tagged
                    .tag(TagType::Id3v2)
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
        assert_eq!(parser_version_for_extension("m4a"), MP4_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("MP4"), MP4_PARSER_VERSION);
        assert_eq!(parser_version_for_extension("ogg"), OGG_PARSER_VERSION);
    }
}
