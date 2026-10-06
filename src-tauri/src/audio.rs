use crate::plex::{contains_plex_token, legacy_plex_image_ref, PlexImageRef};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaLocator {
    Local {
        uri: String,
    },
    Plex {
        server_id: String,
        part_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rating_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurrentMedia {
    Local {
        uri: String,
        queue_index: usize,
    },
    Plex {
        server_id: String,
        part_key: String,
        queue_index: usize,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeBackend {
    AlsaHardware,
    #[serde(rename = "pipewire")]
    PipeWire,
    MpdSoftware,
    Unavailable,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct VolumeStatus {
    pub value: u32,
    pub muted: bool,
    pub writable: bool,
    pub available: bool,
    pub backend: VolumeBackend,
}

impl VolumeStatus {
    fn from_mpd(value: i32) -> Self {
        let available = (0..=100).contains(&value);
        let value = if available { value as u32 } else { 0 };
        Self {
            value,
            muted: available && value == 0,
            writable: available,
            available,
            backend: if available {
                VolumeBackend::MpdSoftware
            } else {
                VolumeBackend::Unavailable
            },
        }
    }

    pub(crate) fn pipewire(value: u32, muted: bool) -> Self {
        Self {
            value,
            muted,
            writable: true,
            available: true,
            backend: VolumeBackend::PipeWire,
        }
    }

    pub(crate) fn pipewire_unavailable() -> Self {
        Self {
            value: 0,
            muted: false,
            writable: false,
            available: false,
            backend: VolumeBackend::Unavailable,
        }
    }

    pub(crate) fn identify_backend(&mut self, backend: VolumeBackend) {
        self.backend = if self.available {
            backend
        } else {
            VolumeBackend::Unavailable
        };
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub thumb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plex_image: Option<PlexImageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_locator: Option<MediaLocator>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uri: String,
    #[serde(default)]
    pub duration: Option<f64>,
}

const QUEUE_CACHE_VERSION: u8 = 2;

#[derive(Serialize)]
struct PersistedQueueRef<'a> {
    version: u8,
    tracks: Vec<PersistedTrackRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resume: Option<&'a PlaybackResumeSnapshot>,
}

#[derive(Serialize)]
struct PersistedTrackRef<'a> {
    title: &'a str,
    artist: &'a str,
    album: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    plex_image: Option<&'a PlexImageRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_locator: Option<&'a MediaLocator>,
    #[serde(skip_serializing_if = "str::is_empty")]
    uri: &'a str,
    duration: Option<f64>,
}

#[derive(Deserialize)]
struct PersistedQueue {
    version: u8,
    tracks: Vec<PersistedTrack>,
    #[serde(default)]
    resume: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct PersistedTrack {
    title: String,
    artist: String,
    album: String,
    #[serde(default)]
    plex_image: Option<PlexImageRef>,
    #[serde(default)]
    media_locator: Option<MediaLocator>,
    #[serde(default)]
    uri: String,
    #[serde(default)]
    duration: Option<f64>,
}

impl From<PersistedTrack> for TrackMetadata {
    fn from(track: PersistedTrack) -> Self {
        Self {
            title: track.title,
            artist: track.artist,
            album: track.album,
            thumb: None,
            plex_image: track.plex_image,
            media_locator: track.media_locator,
            uri: track.uri,
            duration: track.duration,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalItem {
    pub item_type: String,
    pub path: String,
    pub name: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbum {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<String>,
    pub folder_path: String,
    pub source_id: Option<String>,
    pub track_count: usize,
    pub discs: Vec<LocalAlbumDisc>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbumDisc {
    pub number: u32,
    pub label: String,
    pub folder_path: String,
    pub track_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlaybackStatus {
    pub state: String,
    pub elapsed: f64,
    pub duration: f64,
    pub audio_format: String,
    pub current_media: Option<CurrentMedia>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub thumb: Option<String>,
    pub plex_image: Option<PlexImageRef>,
    pub volume: VolumeStatus,
    pub is_updating: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MpdProbeFailure {
    SocketUnavailable,
    ProtocolUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Stopped,
    Paused,
    Playing,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct PlaybackResumeSnapshot {
    queue_index: usize,
    elapsed: f64,
    state: PlaybackState,
}

impl PlaybackResumeSnapshot {
    fn valid_for(&self, queue_len: usize) -> bool {
        self.queue_index < queue_len
            && self.elapsed.is_finite()
            && self.elapsed >= 0.0
            && self.state != PlaybackState::Stopped
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupQueueRestore {
    Pending,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SeekPlaybackStatus {
    state: PlaybackState,
    elapsed: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceSwitchSnapshot {
    pub queue_index: usize,
    pub elapsed: f64,
    pub state: PlaybackState,
}

#[derive(Debug)]
pub struct DeviceSwitchPreparationError {
    pub cause: String,
    pub snapshot: Option<DeviceSwitchSnapshot>,
}

fn get_queue_cache_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        thread_local! {
            static TEST_CACHE_PATH: PathBuf = std::env::temp_dir()
                .join(format!("sonante-queue-test-{}-{:?}", std::process::id(), std::thread::current().id()))
                .join("sonante/queue_cache.json");
        }
        return Some(TEST_CACHE_PATH.with(Clone::clone));
    }
    #[cfg(not(test))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| Path::new(&h).join(".config")))
            .ok()?;
        Some(base.join("sonante").join("queue_cache.json"))
    }
}

const MAX_LOCAL_COVER_BYTES: usize = 8 * 1024 * 1024;
const MAX_EMBEDDED_COVER_TRACK_PROBES: usize = 3;

struct PictureChunk {
    total_size: usize,
    mime: Option<String>,
    bytes: Vec<u8>,
}

enum PictureResponse {
    Missing,
    TooLarge,
    Chunk(PictureChunk),
}

pub fn find_folder_cover_path(dir: &Path) -> Option<String> {
    let candidate_names = [
        "cover.jpg", "cover.jpeg", "cover.png",
        "Folder.jpg", "folder.jpg", "folder.jpeg", "folder.png",
        "front.jpg", "Front.jpg", "front.png", "Front.png",
        "albumart.jpg", "AlbumArtSmall.jpg"
    ];

    for name in &candidate_names {
        let p = dir.join(name);
        if p.is_file() {
            if let Ok(bytes) = std::fs::read(&p) {
                if bytes.len() <= MAX_LOCAL_COVER_BYTES {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("jpeg").to_lowercase();
                    let mime = if ext == "png" { "image/png" } else { "image/jpeg" };
                    let b64 = BASE64.encode(&bytes);
                    return Some(format!("data:{};base64,{}", mime, b64));
                }
            }
        }
    }
    None
}

fn check_hdmi_connected(card_num: &str, dev_id: &str) -> Option<String> {
    let card_dir = format!("/proc/asound/card{}", card_num);
    let Ok(entries) = std::fs::read_dir(&card_dir) else {
        return None;
    };

    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.starts_with("eld#") && (fname.ends_with(&format!(".{}", dev_id)) || fname.contains(&format!("#{}.", dev_id))) {
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                let is_valid = content.lines().any(|l| {
                    let t = l.trim();
                    t == "eld_valid\t1" || t == "eld_valid 1" || t.starts_with("eld_valid 1")
                });
                if is_valid {
                    let mut monitor = String::new();
                    for line in content.lines() {
                        if line.starts_with("monitor_name") {
                            let parts: Vec<&str> = line.split(&['\t', ' '][..]).collect();
                            if parts.len() > 1 {
                                monitor = parts[1..].join(" ").trim().to_string();
                            }
                        }
                    }
                    return Some(if !monitor.is_empty() {
                        format!("Monitor/TV [{}]", monitor)
                    } else {
                        format!("Saída HDMI/DP {}", dev_id)
                    });
                }
            }
        }
    }
    None
}

pub fn list_audio_devices() -> Vec<AudioDevice> {
    let mut dacs = Vec::new();
    let mut onboard = Vec::new();
    let mut hdmi_devs = Vec::new();

    if let Ok(output) = Command::new("aplay").arg("-l").output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if line.starts_with("card ") && line.contains("device ") {
                let colon_pos = match line.find(':') {
                    Some(p) => p,
                    None => continue,
                };
                let card_num = line[5..colon_pos].trim();

                let device_pos = match line.find(", device ") {
                    Some(p) => p,
                    None => continue,
                };

                let card_spec = line[colon_pos + 1..device_pos].trim();
                let (card_id, card_label) = if let Some(bracket_start) = card_spec.find('[') {
                    let id = card_spec[..bracket_start].trim();
                    let label = card_spec[bracket_start + 1..card_spec.len().saturating_sub(1)].trim();
                    (id, label)
                } else {
                    (card_spec, card_spec)
                };

                let after_dev = &line[device_pos + 9..];
                let (dev_id, dev_label) = if let Some(colon_dev) = after_dev.find(':') {
                    let d_id = after_dev[..colon_dev].trim();
                    let rest = after_dev[colon_dev + 1..].trim();
                    let d_label = if let (Some(b_start), Some(b_end)) = (rest.find('['), rest.rfind(']')) {
                        rest[b_start + 1..b_end].trim()
                    } else {
                        rest
                    };
                    (d_id, d_label)
                } else {
                    ("0", "Default")
                };

                let hw_id = format!("hw:CARD={},DEV={}", card_id, dev_id);

                let is_usb_dac = card_id.to_uppercase().contains("USB")
                    || card_label.to_uppercase().contains("USB")
                    || dev_label.to_uppercase().contains("USB")
                    || card_id.to_uppercase().contains("R2R")
                    || card_label.to_uppercase().contains("DAC");

                let is_hdmi = card_id.to_uppercase().contains("HDMI")
                    || card_label.to_uppercase().contains("HDMI")
                    || dev_label.to_uppercase().contains("HDMI")
                    || dev_label.to_uppercase().contains("DISPLAYPORT")
                    || dev_label.to_uppercase().contains("DP");

                if is_usb_dac {
                    dacs.push(AudioDevice {
                        id: hw_id,
                        name: format!("{} — DAC USB (hw:CARD={},DEV={})", card_label, card_id, dev_id),
                    });
                } else if is_hdmi {
                    if let Some(display_name) = check_hdmi_connected(card_num, dev_id) {
                        hdmi_devs.push(AudioDevice {
                            id: hw_id,
                            name: format!("{} — {} (hw:CARD={},DEV={})", card_label, display_name, card_id, dev_id),
                        });
                    }
                } else {
                    onboard.push(AudioDevice {
                        id: hw_id,
                        name: format!("{} — {} (hw:CARD={},DEV={})", card_label, dev_label, card_id, dev_id),
                    });
                }
            }
        }
    }

    let mut devices = Vec::new();
    devices.extend(dacs);
    devices.extend(onboard);
    devices.extend(hdmi_devs);

    devices
}

pub struct AudioEngine {
    socket_path: String,
    music_dir: String,
    queue: Vec<TrackMetadata>,
    restored_resume: Option<PlaybackResumeSnapshot>,
    startup_restore: StartupQueueRestore,
}

const LOCAL_ALBUM_PAGE_SIZE: usize = 500;

#[derive(Default)]
struct AlbumCollector {
    title: String,
    year: Option<String>,
    track_count: usize,
    folder_path: String,
    album_artist: Option<(String, String)>,
    album_artist_consistent: bool,
    artists: HashMap<String, (String, usize)>,
    discs: Vec<LocalAlbumDisc>,
}

#[derive(Default)]
struct LocalAlbumCollection {
    physical: HashMap<String, AlbumCollector>,
    cue: HashMap<String, AlbumCollector>,
}

struct VirtualCueTrackPath {
    cue_directory: String,
    physical_folder: String,
}

impl AlbumCollector {
    fn merge_title_variant(&mut self, other: Self) {
        self.track_count += other.track_count;
        for other_disc in other.discs {
            if let Some(disc) = self
                .discs
                .iter_mut()
                .find(|disc| disc.folder_path == other_disc.folder_path)
            {
                disc.track_count += other_disc.track_count;
            } else {
                self.discs.push(other_disc);
            }
        }
        if self.year.is_none() {
            self.year = other.year;
        }
        if !other.album_artist_consistent
            || self.album_artist.as_ref().map(|(name, _)| name)
                != other.album_artist.as_ref().map(|(name, _)| name)
        {
            self.album_artist_consistent = false;
        }
        if let (Some((_, display)), Some((_, other_display))) =
            (&mut self.album_artist, other.album_artist)
        {
            if other_display > *display {
                *display = other_display;
            }
        }
        for (key, (display, count)) in other.artists {
            let entry = self
                .artists
                .entry(key)
                .or_insert_with(|| (display.clone(), 0));
            entry.1 += count;
            if display > entry.0 {
                entry.0 = display;
            }
        }
    }

    fn merge_disc(&mut self, other: Self) {
        self.track_count += other.track_count;
        if self.year.is_none() {
            self.year = other.year;
        }
        if !other.album_artist_consistent
            || self.album_artist.as_ref().map(|(name, _)| name)
                != other.album_artist.as_ref().map(|(name, _)| name)
        {
            self.album_artist_consistent = false;
        }
        if let (Some((_, display)), Some((_, other_display))) =
            (&mut self.album_artist, other.album_artist)
        {
            if other_display > *display {
                *display = other_display;
            }
        }
        for (key, (display, count)) in other.artists {
            let entry = self
                .artists
                .entry(key)
                .or_insert_with(|| (display.clone(), 0));
            entry.1 += count;
            if display > entry.0 {
                entry.0 = display;
            }
        }
        self.discs.extend(other.discs);
    }

    fn add_artist_metadata(&mut self, artist: Option<String>, album_artist: Option<String>) {
        let album_artist = album_artist
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if let Some(name) = album_artist {
            let normalized = name.to_lowercase();
            match &mut self.album_artist {
                Some((existing, display)) if *existing == normalized => {
                    if name > display.as_str() {
                        *display = name.to_string();
                    }
                }
                Some(_) => self.album_artist_consistent = false,
                None => self.album_artist = Some((normalized, name.to_string())),
            }
        } else {
            self.album_artist_consistent = false;
        }

        if let Some(name) = artist
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let entry = self
                .artists
                .entry(name.to_lowercase())
                .or_insert_with(|| (name.to_string(), 0));
            entry.1 += 1;
            if name > entry.0.as_str() {
                entry.0 = name.to_string();
            }
        }
    }

    fn display_artist(&self) -> String {
        if self.album_artist_consistent {
            if let Some((_, display)) = &self.album_artist {
                return display.clone();
            }
        }
        if self.artists.len() == 1 {
            return self.artists.values().next().unwrap().0.clone();
        }
        if let Some((display, count)) = self.artists.values().max_by_key(|(_, count)| count) {
            if *count >= self.track_count - self.track_count / 5 {
                return display.clone();
            }
            return "Various Artists".to_string();
        }
        "Artista Desconhecido".to_string()
    }
}

impl AudioEngine {
    fn virtual_cue_track_path(file: &str) -> Option<VirtualCueTrackPath> {
        let path = Path::new(file);
        if path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return None;
        }

        let leaf = path.file_name()?.to_str()?;
        let track_number = leaf.strip_prefix("track")?;
        if track_number.is_empty() || !track_number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        let cue_directory = path.parent()?;
        let cue_name = cue_directory.file_name()?.to_str()?;
        if !cue_name.to_ascii_lowercase().ends_with(".cue") {
            return None;
        }

        let physical_folder = cue_directory.parent()?;
        if physical_folder.as_os_str().is_empty() {
            return None;
        }

        Some(VirtualCueTrackPath {
            cue_directory: cue_directory.to_string_lossy().into_owned(),
            physical_folder: physical_folder.to_string_lossy().into_owned(),
        })
    }

    fn disc_folder_number(name: &str) -> Option<u32> {
        let lower = name.to_ascii_lowercase();
        let suffix = ["disc", "disk", "cd"]
            .iter()
            .find_map(|prefix| lower.strip_prefix(prefix))?;
        let suffix = if matches!(suffix.as_bytes().first(), Some(b' ' | b'-' | b'_')) {
            &suffix[1..]
        } else {
            suffix
        };
        let digit_count = suffix.bytes().take_while(u8::is_ascii_digit).count();
        if digit_count == 0 {
            return None;
        }
        let (digits, remainder) = suffix.split_at(digit_count);
        let number = digits.parse::<u32>().ok().filter(|number| *number > 0)?;
        if remainder.is_empty() {
            return Some(number);
        }
        let separated = remainder.starts_with(char::is_whitespace)
            || remainder.starts_with(['(', '[', '-', '–', '—', ':']);
        if separated && remainder.chars().any(char::is_alphabetic) {
            Some(number)
        } else {
            None
        }
    }

    fn multidisc_base_title(album_title: &str, disc_number: u32) -> Option<String> {
        let title = album_title.trim();
        if let Some(without_close) = title.strip_suffix(')') {
            if let Some(open_index) = without_close.rfind('(') {
                let base = without_close[..open_index].trim_end();
                let marker = without_close[open_index + 1..].trim();
                if !base.is_empty() && !marker.is_empty() {
                    let expanded = format!("{base} {marker}");
                    match Self::multidisc_base_title(&expanded, disc_number) {
                        None => return None,
                        Some(parsed) if parsed == base => return Some(parsed),
                        _ => {}
                    }
                }
            }
        }

        for (index, ch) in title.char_indices().rev() {
            if !ch.is_whitespace() {
                continue;
            }
            let suffix = title[index..].trim_start();
            let lower = suffix.to_ascii_lowercase();
            let Some(after_prefix) = ["disco", "disc", "disk", "cd"]
                .iter()
                .find_map(|prefix| lower.strip_prefix(prefix))
            else {
                continue;
            };
            let after_prefix = if matches!(
                after_prefix.as_bytes().first(),
                Some(b' ' | b'.' | b'-' | b'_')
            ) {
                &after_prefix[1..]
            } else {
                after_prefix
            };
            let digit_count = after_prefix.bytes().take_while(u8::is_ascii_digit).count();
            if digit_count == 0 {
                continue;
            }
            let (digits, remainder) = after_prefix.split_at(digit_count);
            let descriptor = remainder.trim_start();
            let valid_descriptor = if descriptor.is_empty() {
                true
            } else if let Some(inner) = descriptor
                .strip_prefix('(')
                .and_then(|text| text.strip_suffix(')'))
            {
                !inner.trim().is_empty()
            } else if let Some(inner) = descriptor
                .strip_prefix('[')
                .and_then(|text| text.strip_suffix(']'))
            {
                !inner.trim().is_empty()
            } else if let Some(inner) = descriptor.strip_prefix(['-', '–', '—', ':']) {
                !inner.trim().is_empty()
            } else {
                false
            };
            if !valid_descriptor {
                continue;
            }
            let number = digits.parse::<u32>().ok()?;
            if number != disc_number || number == 0 {
                return None;
            }
            let base = title[..index].trim_end();
            let base = match base.char_indices().next_back() {
                Some((separator_index, ',' | '-' | '–' | '—' | ':')) => {
                    base[..separator_index].trim_end()
                }
                _ => base,
            };
            return (!base.is_empty()).then(|| base.to_string());
        }
        Some(title.to_string())
    }

    fn local_album_id(folder: &str, title: &str) -> String {
        format!(
            "{}:{}:{}",
            folder.len(),
            folder,
            title.trim().to_lowercase()
        )
    }

    fn consolidate_dominant_album_titles(map: &mut HashMap<String, AlbumCollector>) {
        let mut folders: HashMap<String, Vec<String>> = HashMap::new();
        for (id, album) in map.iter() {
            folders
                .entry(album.folder_path.clone())
                .or_default()
                .push(id.clone());
        }
        for (_, mut ids) in folders {
            if ids.len() < 2 {
                continue;
            }
            ids.sort();
            let total: usize = ids.iter().map(|id| map[id].track_count).sum();
            let Some(dominant_id) = ids
                .iter()
                .find(|id| map[*id].track_count >= total - total / 5)
                .cloned()
            else {
                continue;
            };
            let mut dominant = map.remove(&dominant_id).expect("dominant album must exist");
            for id in ids {
                if id != dominant_id {
                    dominant
                        .merge_title_variant(map.remove(&id).expect("album variant must exist"));
                }
            }
            map.insert(dominant_id, dominant);
        }
    }

    fn consolidate_local_album_discs(map: &mut HashMap<String, AlbumCollector>) {
        let mut candidates: HashMap<String, Vec<(u32, String, Option<String>)>> = HashMap::new();
        for (id, album) in map.iter() {
            let folder = Path::new(&album.folder_path);
            let Some((root, label)) = folder.parent().zip(folder.file_name()) else {
                continue;
            };
            let Some(label) = label.to_str() else {
                continue;
            };
            let Some(number) = Self::disc_folder_number(label) else {
                continue;
            };
            let root = root.to_string_lossy().to_string();
            if root.is_empty() {
                continue;
            }
            candidates.entry(root).or_default().push((
                number,
                id.clone(),
                Self::multidisc_base_title(&album.title, number),
            ));
        }

        for (root, mut discs) in candidates {
            if discs.len() < 2 {
                continue;
            }
            discs.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            if discs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                continue;
            }
            let Some(title) = discs[0].2.clone() else {
                continue;
            };
            let normalized_title = title.trim().to_lowercase();
            if discs.iter().any(|(_, _, base)| {
                base.as_ref().map(|value| value.trim().to_lowercase())
                    != Some(normalized_title.clone())
            }) {
                continue;
            }
            let consolidated_id = Self::local_album_id(&root, &title);
            if map.contains_key(&consolidated_id) {
                continue;
            }
            let mut albums = discs
                .into_iter()
                .map(|(_, id, _)| map.remove(&id).expect("disc candidate must exist"));
            let mut merged = albums.next().expect("multiple disc candidates");
            merged.title = title;
            for album in albums {
                merged.merge_disc(album);
            }
            merged.discs.sort_by(|a, b| {
                a.number
                    .cmp(&b.number)
                    .then_with(|| a.folder_path.cmp(&b.folder_path))
            });
            map.insert(consolidated_id, merged);
        }
    }

    fn consistent_queue_index(
        status_index: Option<usize>,
        current_song_index: Option<usize>,
    ) -> Option<usize> {
        match (status_index, current_song_index) {
            (Some(status), Some(current)) if status == current => Some(status),
            (Some(_), Some(_)) => None,
            (Some(status), None) => Some(status),
            (None, Some(current)) => Some(current),
            (None, None) => None,
        }
    }

    fn current_media_for_queue_index(&self, queue_index: Option<usize>) -> Option<CurrentMedia> {
        let queue_index = queue_index?;
        let track = self.queue.get(queue_index)?;

        match &track.media_locator {
            Some(MediaLocator::Plex {
                server_id,
                part_key,
                ..
            }) if !server_id.is_empty() && !part_key.is_empty() => Some(CurrentMedia::Plex {
                server_id: server_id.clone(),
                part_key: part_key.clone(),
                queue_index,
            }),
            Some(MediaLocator::Local { uri }) if !uri.is_empty() => Some(CurrentMedia::Local {
                uri: uri.clone(),
                queue_index,
            }),
            None if !track.uri.is_empty() => Some(CurrentMedia::Local {
                uri: track.uri.clone(),
                queue_index,
            }),
            _ => None,
        }
    }

    pub fn local_album_socket_path(&self) -> String {
        self.socket_path.clone()
    }

    fn local_album_windows(total: usize) -> Vec<(usize, usize)> {
        let mut windows = Vec::new();
        let mut start = 0;
        while start < total {
            let end = start.saturating_add(LOCAL_ALBUM_PAGE_SIZE).min(total);
            windows.push((start, end));
            start = end;
        }
        windows
    }

    fn parse_local_album_count(lines: &[String]) -> Result<usize, String> {
        let mut songs = None;
        for line in lines {
            if let Some(value) = line.strip_prefix("songs: ") {
                if songs.is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("Contagem de músicas inválida na resposta do MPD.".to_string());
                }
                songs = Some(value.parse::<usize>().map_err(|_| {
                    "Contagem de músicas excede o limite suportado.".to_string()
                })?);
            }
        }
        songs.ok_or_else(|| "Resposta count do MPD não contém songs.".to_string())
    }

    pub fn get_local_albums_at(
        socket_path: &str,
        valid_source_ids: &HashSet<String>,
    ) -> Result<Vec<LocalAlbum>, String> {
        let status = Self::send_command_to_socket(socket_path, "status")
            .map_err(|e| format!("Falha ao verificar atualização da biblioteca: {e}"))?;
        if status.iter().any(|line| line.starts_with("updating_db: ")) {
            return Err("Biblioteca local em atualização no MPD; tente novamente após a atualização.".to_string());
        }
        let count = Self::send_command_to_socket(socket_path, "count \"(base '')\"")
            .map_err(|e| format!("Falha ao contar músicas da biblioteca local: {e}"))?;
        let total = Self::parse_local_album_count(&count)?;
        let mut collection = LocalAlbumCollection::default();
        for (start, end) in Self::local_album_windows(total) {
            let command = format!("find \"(base '')\" window {start}:{end}");
            let lines = Self::send_command_to_socket(socket_path, &command)
                .map_err(|e| format!("Falha ao ler página {start}:{end} da biblioteca local: {e}"))?;
            let found = Self::collect_local_album_page(&mut collection, lines)
                .map_err(|e| format!("Página {start}:{end} inválida: {e}"))?;
            if found != end - start {
                return Err(format!(
                    "Página {start}:{end} inconsistente: esperadas {} músicas, recebidas {found}.",
                    end - start
                ));
            }
        }
        Ok(Self::finalize_local_album_collection(
            collection,
            valid_source_ids,
        ))
    }

    fn finalize_local_album_collection(
        collection: LocalAlbumCollection,
        valid_source_ids: &HashSet<String>,
    ) -> Vec<LocalAlbum> {
        let mut map = Self::canonical_local_album_collectors(collection);
        Self::consolidate_dominant_album_titles(&mut map);
        Self::consolidate_local_album_discs(&mut map);

        let mut albums: Vec<LocalAlbum> = map
            .into_iter()
            .filter(|(_, col)| col.track_count > 0 && !col.folder_path.is_empty())
            .map(|(key, col)| {
                let artist = col.display_artist();
                let source_id = Self::local_album_source_id(&col.discs, valid_source_ids);
                LocalAlbum {
                    id: key,
                    title: col.title,
                    artist,
                    year: col.year,
                    folder_path: col.folder_path,
                    source_id,
                    track_count: col.track_count,
                    discs: col.discs,
                }
            })
            .collect();

        albums.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        albums
    }

    fn local_album_source_id(
        discs: &[LocalAlbumDisc],
        valid_source_ids: &HashSet<String>,
    ) -> Option<String> {
        let mut resolved = discs.iter().map(|disc| {
            let mut components = Path::new(&disc.folder_path).components();
            let first = match components.next() {
                Some(std::path::Component::Normal(name)) => name.to_str(),
                _ => None,
            }?;
            if components.any(|component| !matches!(component, std::path::Component::Normal(_))) {
                return None;
            }
            valid_source_ids.contains(first).then(|| first.to_string())
        });
        let first = resolved.next()??;
        resolved
            .all(|source| source.as_deref() == Some(first.as_str()))
            .then_some(first)
    }

    fn canonical_local_album_collectors(
        collection: LocalAlbumCollection,
    ) -> HashMap<String, AlbumCollector> {
        let mut canonical = collection.physical;
        for (key, cue) in collection.cue {
            match canonical.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    if entry.get().track_count == 1
                        && cue.track_count >= 2
                        && cue.discs.len() == 1
                    {
                        entry.insert(cue);
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(cue);
                }
            }
        }
        canonical
    }

    fn collect_local_album_page(
        collection: &mut LocalAlbumCollection,
        lines: Vec<String>,
    ) -> Result<usize, String> {
        let mut cur_file = String::new();
        let mut cur_album = None;
        let mut cur_artist = None;
        let mut cur_album_artist = None;
        let mut cur_date = None;

        let commit_track = |collection: &mut LocalAlbumCollection,
                            file: &str,
                            album: Option<String>,
                            artist: Option<String>,
                            album_artist: Option<String>,
                            date: Option<String>| {
            if file.is_empty() {
                return;
            }
            let path = Path::new(file);
            let cue_path = Self::virtual_cue_track_path(file);
            let is_cue = cue_path.is_some();
            let physical_folder = cue_path
                .as_ref()
                .map(|cue| cue.physical_folder.clone())
                .unwrap_or_else(|| {
                    path.parent()
                        .map(|directory| directory.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            let folder_name = Path::new(&physical_folder)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Álbum Desconhecido")
                .to_string();

            let (disc_path, disc_label) = cue_path
                .map(|cue| {
                    let label = Path::new(&cue.cue_directory)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("CUE")
                        .to_string();
                    (cue.cue_directory, label)
                })
                .unwrap_or_else(|| (physical_folder.clone(), folder_name.clone()));
            let title = album.unwrap_or(folder_name);
            let key = Self::local_album_id(&physical_folder, &title);
            let map = if is_cue {
                &mut collection.cue
            } else {
                &mut collection.physical
            };

            let entry = map.entry(key).or_insert_with(|| AlbumCollector {
                title: title.clone(),
                year: date.clone(),
                track_count: 0,
                folder_path: physical_folder.clone(),
                album_artist: None,
                album_artist_consistent: true,
                artists: HashMap::new(),
                discs: Vec::new(),
            });

            if title > entry.title {
                entry.title = title;
            }
            entry.track_count += 1;
            if let Some(disc) = entry
                .discs
                .iter_mut()
                .find(|disc| disc.folder_path == disc_path)
            {
                disc.track_count += 1;
            } else {
                entry.discs.push(LocalAlbumDisc {
                    number: Self::disc_folder_number(&disc_label).unwrap_or(0),
                    label: disc_label,
                    folder_path: disc_path,
                    track_count: 1,
                });
            }
            entry.add_artist_metadata(artist, album_artist);
            if entry.year.is_none() && date.is_some() {
                entry.year = date;
            }
        };

        let mut found = 0;
        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "file" => {
                        if v.is_empty() {
                            return Err("entrada file vazia".to_string());
                        }
                        commit_track(
                            collection,
                            &cur_file,
                            cur_album.take(),
                            cur_artist.take(),
                            cur_album_artist.take(),
                            cur_date.take(),
                        );
                        cur_file = v.to_string();
                        found += 1;
                    }
                    "Album" if !cur_file.is_empty() => cur_album = Some(v.to_string()),
                    "Artist" if !cur_file.is_empty() => cur_artist = Some(v.to_string()),
                    "AlbumArtist" if !cur_file.is_empty() => cur_album_artist = Some(v.to_string()),
                    "Date" if !cur_file.is_empty() => cur_date = Some(v.chars().take(4).collect::<String>()),
                    _ => {}
                }
            }
        }

        commit_track(
            collection,
            &cur_file,
            cur_album,
            cur_artist,
            cur_album_artist,
            cur_date,
        );

        Ok(found)
    }

    pub fn rescan_library(&self) -> Result<(), String> {
        Self::rescan_library_at(&self.socket_path)
    }

    pub fn rescan_library_at(socket_path: &str) -> Result<(), String> {
        Self::send_command_to_socket(socket_path, "rescan").map(|_| ())
    }

    pub fn update_library(socket_path: &str) -> Result<(), String> {
        Self::send_command_to_socket(socket_path, "update").map(|_| ())
    }

    #[cfg(test)]
    fn encode_queue_cache(queue: &[TrackMetadata]) -> Result<Vec<u8>, String> {
        Self::encode_queue_cache_with_resume(queue, None)
    }

    fn encode_queue_cache_with_resume(
        queue: &[TrackMetadata],
        resume: Option<&PlaybackResumeSnapshot>,
    ) -> Result<Vec<u8>, String> {
        if queue.iter().any(|track| match &track.media_locator {
            Some(MediaLocator::Plex { .. }) => false,
            Some(MediaLocator::Local { uri }) => {
                Self::contains_plex_token(uri) || Self::contains_plex_token(&track.uri)
            }
            None => Self::contains_plex_token(&track.uri),
        }) {
            return Err("Cache da fila contém uma URI autenticada inválida.".to_string());
        }
        let tracks = queue
            .iter()
            .map(|track| PersistedTrackRef {
                title: &track.title,
                artist: &track.artist,
                album: &track.album,
                plex_image: track.plex_image.as_ref(),
                media_locator: track.media_locator.as_ref(),
                uri: if matches!(track.media_locator, Some(MediaLocator::Plex { .. })) {
                    ""
                } else {
                    &track.uri
                },
                duration: track.duration,
            })
            .collect();
        serde_json::to_vec(&PersistedQueueRef {
            version: QUEUE_CACHE_VERSION,
            tracks,
            resume: resume.filter(|cursor| cursor.valid_for(queue.len())),
        })
        .map_err(|e| format!("Falha ao serializar o cache da fila: {}", e))
    }

    fn save_queue_cache(&self) -> Result<(), String> {
        let path = get_queue_cache_path()
            .ok_or_else(|| "Não foi possível determinar o caminho do cache da fila.".to_string())?;
        let json = Self::encode_queue_cache_with_resume(&self.queue, self.restored_resume.as_ref())?;
        crate::persistence::atomic_write_private(&path, &json, "queue_cache.json")
    }

    fn migrate_loaded_queue(queue: Vec<TrackMetadata>) -> (Vec<TrackMetadata>, bool) {
        let mut changed = false;
        let migrated = queue
            .into_iter()
            .filter_map(|mut track| {
                changed |= Self::sanitize_track_artwork(&mut track);
                changed |= track.thumb.take().is_some();
                match &track.media_locator {
                    Some(MediaLocator::Plex {
                        server_id,
                        part_key,
                        ..
                    }) if !server_id.trim().is_empty() && !part_key.trim().is_empty() => {
                        // A referência estável é a fonte de verdade; nunca mantenha uma URI
                        // autenticada que possa ter vindo de uma versão intermediária do cache.
                        changed |= !track.uri.is_empty();
                        track.uri.clear();
                        Some(track)
                    }
                    Some(MediaLocator::Plex { .. }) => {
                        changed = true;
                        None
                    }
                    Some(MediaLocator::Local { uri })
                        if !Self::contains_plex_token(uri)
                            && !Self::contains_plex_token(&track.uri) =>
                    {
                        Some(track)
                    }
                    Some(MediaLocator::Local { .. }) => {
                        changed = true;
                        None
                    }
                    None if !Self::contains_plex_token(&track.uri) => Some(track),
                    None => {
                        changed = true;
                        None
                    }
                }
            })
            .collect();
        (migrated, changed)
    }

    fn sanitize_track_artwork(track: &mut TrackMetadata) -> bool {
        match &track.media_locator {
            Some(MediaLocator::Plex { server_id, .. }) => {
                let mut changed = false;
                if track.plex_image.as_ref().is_some_and(|image| {
                    !image.is_valid() || image.server_id != *server_id
                }) {
                    track.plex_image = None;
                    changed = true;
                }
                if let Some(legacy_thumb) = track.thumb.take() {
                    if track.plex_image.is_none() && contains_plex_token(&legacy_thumb) {
                        track.plex_image = legacy_plex_image_ref(&legacy_thumb, Some(server_id));
                    }
                    changed = true;
                }
                changed
            }
            _ => {
                let mut changed = track.plex_image.take().is_some();
                if track.thumb.as_deref().is_some_and(contains_plex_token) {
                    track.thumb = None;
                    changed = true;
                }
                changed
            }
        }
    }

    pub(crate) fn contains_plex_token(uri: &str) -> bool {
        uri.split_once('?').is_some_and(|(_, query)| {
            query.split('&').any(|field| {
                field
                    .split_once('=')
                    .map(|(name, _)| name.eq_ignore_ascii_case("X-Plex-Token"))
                    .unwrap_or(false)
            })
        })
    }

    fn sanitize_mpd_error(error: String, playback_uris: &[String]) -> String {
        if !playback_uris
            .iter()
            .any(|uri| Self::contains_plex_token(uri))
        {
            return error;
        }
        if error.starts_with("ACK [") {
            if let Some(end) = error.find('}') {
                return format!("O MPD recusou a mídia Plex ({}).", &error[..=end]);
            }
        }
        "Falha ao enviar a mídia Plex ao MPD.".to_string()
    }

    #[cfg(test)]
    fn decode_queue_cache<R: BufRead>(reader: R) -> Result<(Vec<TrackMetadata>, bool), String> {
        let (queue, _, changed) = Self::decode_queue_cache_with_resume(reader)?;
        Ok((queue, changed))
    }

    fn decode_queue_cache_with_resume<R: BufRead>(mut reader: R) -> Result<(Vec<TrackMetadata>, Option<PlaybackResumeSnapshot>, bool), String> {
        let first = loop {
            let available = reader
                .fill_buf()
                .map_err(|_| "Falha ao ler cache da fila.".to_string())?;
            if available.is_empty() {
                return Err("Cache da fila vazio ou inválido.".to_string());
            }
            let whitespace = available
                .iter()
                .take_while(|byte| byte.is_ascii_whitespace())
                .count();
            if whitespace == available.len() {
                reader.consume(whitespace);
                continue;
            }
            let first = available[whitespace];
            reader.consume(whitespace);
            break first;
        };
        match first {
            b'[' => {
                let queue: Vec<TrackMetadata> = serde_json::from_reader(reader)
                    .map_err(|_| "Cache antigo da fila inválido.".to_string())?;
                let (queue, _) = Self::migrate_loaded_queue(queue);
                Ok((queue, None, true))
            }
            b'{' => {
                let persisted: PersistedQueue = serde_json::from_reader(reader)
                    .map_err(|_| "Cache v2 da fila inválido.".to_string())?;
                if persisted.version != QUEUE_CACHE_VERSION {
                    return Err("Versão do cache da fila não suportada.".to_string());
                }
                let (queue, changed) = Self::migrate_loaded_queue(
                    persisted
                        .tracks
                        .into_iter()
                        .map(TrackMetadata::from)
                        .collect(),
                );
                let resume = persisted.resume
                    .and_then(|value| serde_json::from_value::<PlaybackResumeSnapshot>(value).ok())
                    .filter(|cursor| cursor.valid_for(queue.len()));
                Ok((queue, resume, changed))
            }
            _ => Err("Cache da fila inválido.".to_string()),
        }
    }

    fn load_queue_cache() -> (Vec<TrackMetadata>, Option<PlaybackResumeSnapshot>) {
        if let Some(p) = get_queue_cache_path() {
            if let Err(error) =
                crate::persistence::prepare_private_file_for_load(&p, "queue_cache.json")
            {
                eprintln!("[Persistência] {}", error);
            }
            if p.exists() {
                match std::fs::File::open(&p) {
                    Ok(file) => match Self::decode_queue_cache_with_resume(BufReader::new(file)) {
                        Ok((queue, resume, changed)) => {
                            if changed {
                                match Self::encode_queue_cache_with_resume(&queue, resume.as_ref()) {
                                    Ok(json) => {
                                        if let Err(error) = crate::persistence::atomic_write_private(
                                            &p,
                                            &json,
                                            "queue_cache.json",
                                        ) {
                                            eprintln!("[Persistência] {}", error);
                                        }
                                    }
                                    Err(error) => eprintln!("[Persistência] {}", error),
                                }
                            }
                            return (queue, resume);
                        }
                        Err(error) => eprintln!("[Persistência] {}", error),
                    },
                    Err(error) => {
                        eprintln!("[Persistência] Falha ao abrir cache da fila: {}", error)
                    }
                }
            }
        }
        (Vec::new(), None)
    }

    pub fn new(socket_path: &str, music_dir: &str) -> Self {
        let (queue, restored_resume) = Self::load_queue_cache();
        let startup_restore = if queue.is_empty() {
            StartupQueueRestore::Ready
        } else {
            StartupQueueRestore::Pending
        };
        Self {
            socket_path: socket_path.to_string(),
            music_dir: music_dir.to_string(),
            queue,
            restored_resume,
            startup_restore,
        }
    }

    pub fn set_music_dir(&mut self, music_dir: &str) {
        self.music_dir = music_dir.to_string();
    }

    pub fn resolve_cover(&self, path_str: &str) -> Option<String> {
        let p = Path::new(path_str);
        let full_path = if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new(&self.music_dir).join(p)
        };

        if full_path.is_dir() {
            find_folder_cover_path(&full_path)
        } else if let Some(parent) = full_path.parent() {
            find_folder_cover_path(parent)
        } else {
            None
        }
    }

    pub fn set_volume(&self, volume: u32) -> Result<(), String> {
        let clamped = volume.min(100);
        self.send_command(&format!("setvol {}", clamped)).map(|_| ())
    }

    pub(crate) fn probe_mpd(&self) -> Result<(), MpdProbeFailure> {
        let stream = UnixStream::connect(&self.socket_path)
            .map_err(|_| MpdProbeFailure::SocketUnavailable)?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|_| MpdProbeFailure::ProtocolUnavailable)?;
        let mut greeting = String::new();
        let greeting_bytes = BufReader::new(stream)
            .read_line(&mut greeting)
            .map_err(|_| MpdProbeFailure::ProtocolUnavailable)?;
        if greeting_bytes == 0 || !greeting.starts_with("OK MPD ") || !greeting.ends_with('\n') {
            return Err(MpdProbeFailure::ProtocolUnavailable);
        }
        Ok(())
    }

    fn send_command(&self, command: &str) -> Result<Vec<String>, String> {
        Self::send_command_to_socket(&self.socket_path, command)
    }

    fn send_command_to_socket(socket_path: &str, command: &str) -> Result<Vec<String>, String> {
        let mut stream = UnixStream::connect(socket_path)
            .map_err(|e| format!("Falha ao conectar no socket MPD: {}", e))?;

        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| format!("Falha ao configurar timeout de leitura do MPD: {}", e))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| format!("Falha ao configurar timeout de escrita do MPD: {}", e))?;

        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut welcome = String::new();
        let welcome_bytes = reader
            .read_line(&mut welcome)
            .map_err(|e| format!("Falha ao ler handshake do MPD: {}", e))?;
        if welcome_bytes == 0 || !welcome.starts_with("OK MPD ") || !welcome.ends_with('\n') {
            return Err("Handshake inválido ou incompleto recebido do MPD.".to_string());
        }

        let cmd = format!("{}\n", command);
        stream
            .write_all(cmd.as_bytes())
            .map_err(|e| format!("Falha ao enviar comando ao MPD: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("Falha ao concluir envio do comando ao MPD: {}", e))?;

        Self::read_mpd_response(&mut reader)
    }

    pub fn get_local_cover_at(
        socket_path: &str,
        library_dir: &Path,
        path: &str,
    ) -> Result<Option<String>, String> {
        let relative = Path::new(path);
        let full_path = if relative.is_absolute() {
            relative.to_path_buf()
        } else {
            library_dir.join(relative)
        };
        let local_cover = if full_path.is_dir() {
            find_folder_cover_path(&full_path)
        } else {
            full_path.parent().and_then(find_folder_cover_path)
        };
        if local_cover.is_some() {
            return Ok(local_cover);
        }
        if relative.is_absolute() || path.is_empty() {
            return Ok(None);
        }
        Self::embedded_cover_for_folder(socket_path, path)
    }

    fn embedded_cover_for_folder(
        socket_path: &str,
        folder: &str,
    ) -> Result<Option<String>, String> {
        let command = format!("lsinfo {}", Self::quote_mpd_argument(folder)?);
        let lines = Self::send_command_to_socket(socket_path, &command)?;
        let tracks = lines
            .iter()
            .filter_map(|line| line.strip_prefix("file: "))
            .filter(|uri| Path::new(uri).parent() == Some(Path::new(folder)))
            .take(MAX_EMBEDDED_COVER_TRACK_PROBES);
        for uri in tracks {
            if let Some(cover) = Self::embedded_cover_for_track(socket_path, uri)? {
                return Ok(Some(cover));
            }
        }
        Ok(None)
    }

    fn embedded_cover_for_track(socket_path: &str, uri: &str) -> Result<Option<String>, String> {
        let mut image = Vec::new();
        let mut expected_size = None;
        let mut mime = None;
        loop {
            let response = Self::read_picture_chunk(socket_path, uri, image.len())?;
            let chunk = match response {
                PictureResponse::Missing | PictureResponse::TooLarge => return Ok(None),
                PictureResponse::Chunk(chunk) => chunk,
            };
            if let Some(size) = expected_size {
                if size != chunk.total_size {
                    return Err("Tamanho da imagem MPD mudou entre chunks".into());
                }
            } else {
                expected_size = Some(chunk.total_size);
                image.reserve(chunk.total_size);
            }
            if let Some(kind) = chunk.mime {
                if mime.as_ref().is_some_and(|previous| previous != &kind) {
                    return Err("Tipo da imagem MPD mudou entre chunks".into());
                }
                mime = Some(kind);
            }
            if chunk.bytes.is_empty() {
                return Err("Chunk vazio em imagem MPD incompleta".into());
            }
            image.extend_from_slice(&chunk.bytes);
            if image.len() == chunk.total_size {
                let kind = match Self::safe_picture_mime(mime.as_deref(), &image) {
                    Some(kind) => kind,
                    None => return Ok(None),
                };
                return Ok(Some(format!("data:{kind};base64,{}", BASE64.encode(image))));
            }
        }
    }

    fn safe_picture_mime(reported: Option<&str>, bytes: &[u8]) -> Option<&'static str> {
        if let Some(kind) = reported {
            return match kind {
                "image/jpeg" => Some("image/jpeg"),
                "image/png" => Some("image/png"),
                "image/webp" => Some("image/webp"),
                _ => None,
            };
        }
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some("image/jpeg")
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some("image/webp")
        } else {
            None
        }
    }

    fn read_picture_chunk(
        socket_path: &str,
        uri: &str,
        offset: usize,
    ) -> Result<PictureResponse, String> {
        let stream = UnixStream::connect(socket_path)
            .map_err(|e| format!("Falha ao conectar para readpicture: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| format!("Falha ao configurar leitura de readpicture: {e}"))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| format!("Falha ao configurar escrita de readpicture: {e}"))?;
        let mut reader = BufReader::new(stream);
        let welcome = Self::read_picture_line(&mut reader)?;
        if !welcome.starts_with("OK MPD ") {
            return Err("Handshake inválido de readpicture".into());
        }
        let command = format!("readpicture {} {offset}\n", Self::quote_mpd_argument(uri)?);
        reader
            .get_mut()
            .write_all(command.as_bytes())
            .map_err(|e| format!("Falha ao enviar readpicture: {e}"))?;
        reader
            .get_mut()
            .flush()
            .map_err(|e| format!("Falha ao concluir readpicture: {e}"))?;

        let mut total_size = None;
        let mut mime = None;
        loop {
            let line = Self::read_picture_line(&mut reader)?;
            if line == "OK" {
                return if total_size.unwrap_or(0) == 0 {
                    Ok(PictureResponse::Missing)
                } else {
                    Err("Resposta readpicture sem bloco binário".into())
                };
            }
            if line.starts_with("ACK") {
                return Ok(PictureResponse::Missing);
            }
            if let Some(value) = line.strip_prefix("size: ") {
                let size = value
                    .parse::<usize>()
                    .map_err(|_| "Tamanho inválido de readpicture".to_string())?;
                if size > MAX_LOCAL_COVER_BYTES {
                    return Ok(PictureResponse::TooLarge);
                }
                total_size = Some(size);
            } else if let Some(value) = line.strip_prefix("type: ") {
                mime = Some(value.to_ascii_lowercase());
            } else if let Some(value) = line.strip_prefix("binary: ") {
                let length = value
                    .parse::<usize>()
                    .map_err(|_| "Tamanho binário inválido de readpicture".to_string())?;
                let size = total_size.ok_or("Resposta readpicture sem tamanho total")?;
                if length > MAX_LOCAL_COVER_BYTES
                    || offset.checked_add(length).is_none_or(|end| end > size)
                {
                    return Err("Bloco binário excede o tamanho declarado".into());
                }
                let mut bytes = vec![0; length];
                reader
                    .read_exact(&mut bytes)
                    .map_err(|e| format!("Bloco binário readpicture incompleto: {e}"))?;
                let mut separator = [0];
                reader
                    .read_exact(&mut separator)
                    .map_err(|e| format!("Separador binário readpicture ausente: {e}"))?;
                if separator != [b'\n'] {
                    return Err("Separador binário readpicture inválido".into());
                }
                match Self::read_picture_line(&mut reader)?.as_str() {
                    "OK" => {
                        return Ok(PictureResponse::Chunk(PictureChunk {
                            total_size: size,
                            mime,
                            bytes,
                        }))
                    }
                    value if value.starts_with("ACK") => return Ok(PictureResponse::Missing),
                    _ => return Err("Resposta final inválida de readpicture".into()),
                }
            }
        }
    }

    fn read_picture_line<R: BufRead>(reader: &mut R) -> Result<String, String> {
        const MAX_PICTURE_LINE_BYTES: usize = 4096;
        let mut line = Vec::new();
        loop {
            let available = reader
                .fill_buf()
                .map_err(|e| format!("Falha ao ler cabeçalho readpicture: {e}"))?;
            if available.is_empty() {
                return Err("Conexão readpicture encerrada antes da resposta final".into());
            }
            let count = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(available.len());
            if line.len() + count > MAX_PICTURE_LINE_BYTES {
                return Err("Cabeçalho readpicture excede limite".into());
            }
            line.extend_from_slice(&available[..count]);
            reader.consume(count);
            if line.last() == Some(&b'\n') {
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return String::from_utf8(line)
                    .map_err(|_| "Cabeçalho readpicture não é UTF-8".into());
            }
        }
    }

    fn read_mpd_response<R: BufRead>(reader: &mut R) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|e| format!("Falha ao ler resposta do MPD: {}", e))?;
            if bytes == 0 {
                return Err("Conexão MPD encerrada antes da resposta final.".to_string());
            }
            let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
            if trimmed == "OK" {
                return Ok(lines);
            }
            if trimmed.starts_with("ACK") {
                return Err(trimmed);
            }
            lines.push(trimmed);
        }
    }

    fn capture_playback_snapshot(&self) -> Result<Option<PlaybackResumeSnapshot>, String> {
        if self.queue.is_empty() {
            return Ok(None);
        }
        let lines = self.send_command("status")?;
        let mut state = None;
        let mut queue_index = None;
        let mut elapsed = 0.0;
        for line in lines {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "state" => state = Some(Self::parse_playback_state(value)?),
                    "song" => queue_index = Some(Self::parse_mpd_number("song", value)?),
                    "elapsed" => elapsed = Self::parse_mpd_number("elapsed", value)?,
                    _ => {}
                }
            }
        }
        let state = state.ok_or_else(|| "Status MPD sem estado de reprodução.".to_string())?;
        if state == PlaybackState::Stopped {
            return Ok(self.restored_resume.clone());
        }
        let snapshot = PlaybackResumeSnapshot {
            queue_index: queue_index.ok_or_else(|| "Status MPD sem índice da faixa ativa.".to_string())?,
            elapsed,
            state,
        };
        if !snapshot.valid_for(self.queue.len()) {
            return Err("Status MPD contém cursor fora da fila ou posição inválida.".to_string());
        }
        Ok(Some(snapshot))
    }

    pub fn persist_shutdown_resume(&mut self) -> Result<(), String> {
        let captured = self.capture_playback_snapshot()?;
        if captured == self.restored_resume {
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.restored_resume, captured);
        if let Err(error) = self.save_queue_cache() {
            self.restored_resume = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn startup_queue_pending(&self) -> bool {
        self.startup_restore == StartupQueueRestore::Pending
    }

    pub fn fail_startup_queue_restore(&mut self) {
        if self.startup_queue_pending() {
            self.startup_restore = StartupQueueRestore::Failed;
        }
    }

    pub fn restore_startup_queue(&mut self, playback_uris: &[String]) -> Result<(), String> {
        if !self.startup_queue_pending() {
            return Ok(());
        }
        let commands = match self.device_switch_queue_restore_commands(None, playback_uris) {
            Ok(commands) => commands,
            Err(error) => {
                self.startup_restore = StartupQueueRestore::Failed;
                return Err(error);
            }
        };
        match self.send_command(&commands) {
            Ok(_) => {
                self.startup_restore = StartupQueueRestore::Ready;
                Ok(())
            }
            Err(error) => {
                self.startup_restore = StartupQueueRestore::Failed;
                Err(Self::sanitize_mpd_error(error, playback_uris))
            }
        }
    }

    /// Prepara a troca de saída: captura a posição e para a reprodução sem destruir a fila
    pub fn prepare_device_switch(
        &mut self,
    ) -> Result<Option<DeviceSwitchSnapshot>, DeviceSwitchPreparationError> {
        let before_stop = |cause| DeviceSwitchPreparationError {
            cause,
            snapshot: None,
        };
        let status_lines = self.send_command("status").map_err(before_stop)?;
        let mut state = None;
        let mut elapsed = 0.0;
        let mut queue_index = 0;
        for line in status_lines {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "state" => {
                        state = Some(Self::parse_playback_state(value).map_err(before_stop)?)
                    }
                    "elapsed" => {
                        elapsed = value.parse::<f64>().map_err(|e| {
                            before_stop(format!(
                                "Posição inválida retornada pelo MPD ({}): {}",
                                value, e
                            ))
                        })?;
                    }
                    "song" => {
                        queue_index = value.parse::<usize>().map_err(|e| {
                            before_stop(format!(
                                "Índice inválido retornado pelo MPD ({}): {}",
                                value, e
                            ))
                        })?;
                    }
                    _ => {}
                }
            }
        }
        let state = state.ok_or_else(|| {
            before_stop("Resposta de status do MPD não contém o estado de reprodução.".to_string())
        })?;
        let snapshot = (!self.queue.is_empty()).then_some(DeviceSwitchSnapshot {
            queue_index,
            elapsed,
            state,
        });

        if state != PlaybackState::Stopped {
            self.send_command("stop")
                .map_err(|cause| DeviceSwitchPreparationError { cause, snapshot })?;
        }

        Ok(snapshot)
    }

    /// Restaura fila, faixa e posição quando possível; Playing/Paused terminam pausados e Stopped permanece parado.
    pub fn restore_after_device_switch(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        playback_uris: &[String],
    ) -> Result<(), String> {
        if self.queue.is_empty() {
            return Ok(());
        }

        let mut send_command = |command: &str| self.send_command(command);
        self.restore_after_device_switch_with(
            saved_state,
            playback_uris,
            &mut send_command,
            &mut std::thread::sleep,
        )
    }

    fn restore_after_device_switch_with<F, S>(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        playback_uris: &[String],
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<(), String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        let restore_queue =
            self.device_switch_queue_restore_commands(saved_state, playback_uris)?;
        send_command(&restore_queue)
            .map_err(|error| Self::sanitize_mpd_error(error, playback_uris))?;

        let Some(snapshot) = saved_state else {
            return Ok(());
        };
        if snapshot.state == PlaybackState::Stopped {
            return Ok(());
        }

        if snapshot.elapsed > 0.5 {
            Self::restore_position(snapshot.elapsed, send_command, sleep)?;
        }

        send_command("pause 1")?;

        Ok(())
    }

    fn device_switch_queue_restore_commands(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        playback_uris: &[String],
    ) -> Result<String, String> {
        if playback_uris.len() != self.queue.len() {
            return Err(format!(
                "Quantidade de URIs resolvidas ({}) difere da fila lógica ({}).",
                playback_uris.len(),
                self.queue.len()
            ));
        }
        let mut batch = String::from("command_list_begin\nclear\n");
        for uri in playback_uris {
            batch.push_str(&format!("add {}\n", Self::quote_mpd_argument(uri)?));
        }
        if let Some(snapshot) = saved_state {
            // `add` preserva a posição na fila, mas não seleciona uma faixa atual no MPD.
            // Para Stopped isso é intencional: selecionar exigiria play/seek e poderia abrir o DAC.
            if snapshot.state != PlaybackState::Stopped {
                let target_index = snapshot.queue_index.min(self.queue.len().saturating_sub(1));
                batch.push_str(&format!("play {}\n", target_index));
            }
        }
        batch.push_str("command_list_end");

        Ok(batch)
    }

    fn restore_position<F, S>(
        elapsed: f64,
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<(), String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        const ATTEMPTS: usize = 3;
        const RETRY_DELAY: Duration = Duration::from_millis(75);
        let command = format!("seekcur {:.1}", elapsed);

        for attempt in 0..ATTEMPTS {
            match send_command(&command) {
                Ok(_) => return Ok(()),
                Err(error) if Self::is_not_seekable_ack(&error) => {
                    if attempt + 1 < ATTEMPTS {
                        sleep(RETRY_DELAY);
                    }
                }
                Err(error) => return Err(error),
            }
        }

        eprintln!(
            "[Audio] Aviso: o MPD manteve a faixa ativa, mas recusou restaurar a posição temporal (Not seekable)."
        );
        Ok(())
    }

    fn is_not_seekable_ack(error: &str) -> bool {
        let Some((prefix, message)) = error.rsplit_once('}') else {
            return false;
        };
        error.starts_with("ACK [")
            && prefix.ends_with("{seekcur")
            && message.trim() == "Not seekable"
    }

    fn quote_mpd_argument(value: &str) -> Result<String, String> {
        if value.contains(['\0', '\r', '\n']) {
            return Err(
                "Argumento textual inválido para o MPD: NUL e quebras de linha não são permitidos."
                    .to_string(),
            );
        }
        Ok(format!(
            "\"{}\"",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ))
    }

    fn parse_playback_state(state: &str) -> Result<PlaybackState, String> {
        match state {
            "stop" => Ok(PlaybackState::Stopped),
            "pause" => Ok(PlaybackState::Paused),
            "play" => Ok(PlaybackState::Playing),
            value => Err(format!(
                "Estado de reprodução inválido retornado pelo MPD: {}",
                value
            )),
        }
    }

    fn parse_mpd_number<T>(field: &str, value: &str) -> Result<T, String>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        value.parse::<T>().map_err(|e| {
            format!(
                "Valor inválido para o campo {} retornado pelo MPD: {}",
                field, e
            )
        })
    }

    pub fn play_tracks(
        &mut self,
        mut tracks: Vec<TrackMetadata>,
        playback_uris: Vec<String>,
        start_index: usize,
    ) -> Result<(), String> {
        if tracks.is_empty() {
            return Err("Não é possível iniciar uma fila vazia.".to_string());
        }
        if start_index >= tracks.len() {
            return Err(format!(
                "Índice inicial fora da fila: {} para {} faixa(s).",
                start_index,
                tracks.len()
            ));
        }
        if playback_uris.len() != tracks.len() {
            return Err(format!(
                "Quantidade de URIs resolvidas ({}) difere da fila lógica ({}).",
                playback_uris.len(),
                tracks.len()
            ));
        }

        for (track, playback_uri) in tracks.iter_mut().zip(&playback_uris) {
            Self::sanitize_track_artwork(track);
            if track.thumb.is_none() {
                track.thumb = self.resolve_cover(playback_uri);
            }
        }

        let mut batch = String::from("command_list_begin\nclear\n");
        for uri in &playback_uris {
            batch.push_str(&format!("add {}\n", Self::quote_mpd_argument(uri)?));
        }
        batch.push_str(&format!("play {}\ncommand_list_end", start_index));

        self.send_command(&batch)
            .map_err(|error| Self::sanitize_mpd_error(error, &playback_uris))?;
        self.queue = tracks;
        self.restored_resume = None;
        self.startup_restore = StartupQueueRestore::Ready;
        self.save_queue_cache()
    }

    pub fn play_uris(&mut self, uris: Vec<String>, start_index: usize) -> Result<(), String> {
        if uris.iter().any(|uri| Self::contains_plex_token(uri)) {
            return Err(
                "URLs Plex autenticadas não são aceitas como identidade persistente da faixa."
                    .to_string(),
            );
        }
        let playback_uris = uris.clone();
        let tracks = uris
            .into_iter()
            .map(|u| {
                let clean_u = u.split('?').next().unwrap_or(&u);
                let filename = clean_u.split('/').last().unwrap_or("").to_string();
                TrackMetadata {
                    title: filename,
                    artist: "".to_string(),
                    album: "".to_string(),
                    thumb: None,
                    plex_image: None,
                    media_locator: None,
                    uri: u,
                    duration: None,
                }
            })
            .collect();
        self.play_tracks(tracks, playback_uris, start_index)
    }

    fn ensure_startup_queue_ready(&self) -> Result<(), String> {
        match self.startup_restore {
            StartupQueueRestore::Ready => Ok(()),
            StartupQueueRestore::Pending => Err("A fila ainda está sendo restaurada no MPD.".to_string()),
            StartupQueueRestore::Failed => Err("A fila não pôde ser restaurada no MPD; selecione uma faixa novamente.".to_string()),
        }
    }

    fn observed_mpd_playback_state(&self) -> Result<PlaybackState, String> {
        self.send_command("status")?
            .into_iter()
            .find_map(|line| line.strip_prefix("state: ").map(str::to_string))
            .ok_or_else(|| "Status MPD sem estado de reprodução após retomar a faixa.".to_string())
            .and_then(|state| Self::parse_playback_state(&state))
    }

    pub fn toggle_play_pause(&mut self) -> Result<(), String> {
        self.ensure_startup_queue_ready()?;
        if let Some(snapshot) = self.restored_resume {
            if !snapshot.valid_for(self.queue.len()) {
                return Err("Cursor de retomada inválido para a fila atual.".to_string());
            }
            self.send_command(&format!("play {}", snapshot.queue_index))?;
            self.restored_resume = None;
            if snapshot.elapsed > 0.0 {
                let mut send = |command: &str| self.send_command(command);
                Self::restore_position(snapshot.elapsed, &mut send, &mut std::thread::sleep)?;
            }
            return match self.observed_mpd_playback_state()? {
                PlaybackState::Playing => Ok(()),
                PlaybackState::Paused => self.send_command("pause 0").map(|_| ()),
                PlaybackState::Stopped => {
                    Err("O MPD parou após a tentativa de retomar a faixa.".to_string())
                }
            };
        }
        let status = self.get_status()?;
        match status.state.as_str() {
            "play" => self.send_command("pause 1").map(|_| ()),
            "pause" => self.send_command("pause 0").map(|_| ()),
            _ => self.send_command("play").map(|_| ()),
        }
    }

    pub fn next(&mut self) -> Result<(), String> {
        self.ensure_startup_queue_ready()?;
        self.send_command("next")?;
        self.restored_resume = None;
        Ok(())
    }

    pub fn previous(&mut self) -> Result<(), String> {
        self.ensure_startup_queue_ready()?;
        self.send_command("previous")?;
        self.restored_resume = None;
        Ok(())
    }

    fn parse_seek_status(lines: Vec<String>) -> Result<SeekPlaybackStatus, String> {
        let mut state = None;
        let mut elapsed = None;
        for line in lines {
            if let Some((key, value)) = line.split_once(": ") {
                match key {
                    "state" => state = Some(Self::parse_playback_state(value)?),
                    "elapsed" => elapsed = Some(Self::parse_mpd_number("elapsed", value)?),
                    _ => {}
                }
            }
        }
        Ok(SeekPlaybackStatus {
            state: state.ok_or_else(|| "Status MPD sem estado durante seek.".to_string())?,
            elapsed: elapsed.unwrap_or(0.0),
        })
    }

    fn seek_with<F, S>(
        &mut self,
        seconds: f64,
        is_shared: bool,
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<(), String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err("Posição de seek inválida.".to_string());
        }
        self.ensure_startup_queue_ready()?;
        if let Some(mut snapshot) = self.restored_resume {
            if (snapshot.elapsed - seconds).abs() <= 0.05 {
                return Ok(());
            }
            let previous = snapshot;
            snapshot.elapsed = seconds;
            self.restored_resume = Some(snapshot);
            if let Err(error) = self.save_queue_cache() {
                self.restored_resume = Some(previous);
                return Err(error);
            }
            return Ok(());
        }

        let pre_state = if is_shared {
            Some(Self::parse_seek_status(send_command("status")?)?.state)
        } else {
            None
        };
        send_command(&format!("seekcur {:.1}", seconds))?;
        if pre_state != Some(PlaybackState::Playing) {
            return Ok(());
        }

        const SAMPLE_DELAY: Duration = Duration::from_millis(80);
        const MIN_PROGRESS: f64 = 0.02;
        const TARGET_TOLERANCE: f64 = 2.0;
        sleep(SAMPLE_DELAY);
        let first = Self::parse_seek_status(send_command("status")?)?;
        sleep(SAMPLE_DELAY);
        let second = Self::parse_seek_status(send_command("status")?)?;
        if first.state == PlaybackState::Stopped || second.state == PlaybackState::Stopped {
            return Err("O MPD parou inesperadamente após o seek Shared.".to_string());
        }
        if first.state != PlaybackState::Playing || second.state != PlaybackState::Playing {
            return Err("O MPD deixou o estado de reprodução durante o seek Shared.".to_string());
        }
        let near_target = (first.elapsed - seconds).abs() <= TARGET_TOLERANCE
            && (second.elapsed - seconds).abs() <= TARGET_TOLERANCE;
        if !near_target || second.elapsed - first.elapsed >= MIN_PROGRESS {
            return Ok(());
        }

        send_command("pause 1")
            .map_err(|error| format!("Falha ao pausar para recuperar o seek Shared: {}", error))?;
        send_command("pause 0")
            .map_err(|error| format!("Falha ao retomar após o seek Shared: {}", error))?;
        let recovery_first = Self::parse_seek_status(send_command("status")?)?;
        sleep(SAMPLE_DELAY);
        let recovery_second = Self::parse_seek_status(send_command("status")?)?;
        if recovery_first.state != PlaybackState::Playing
            || recovery_second.state != PlaybackState::Playing
            || recovery_second.elapsed - recovery_first.elapsed < MIN_PROGRESS
        {
            return Err(
                "A saída Shared permaneceu congelada após uma tentativa de pause/resume."
                    .to_string(),
            );
        }
        Ok(())
    }

    pub fn seek(&mut self, seconds: f64, is_shared: bool) -> Result<(), String> {
        let socket_path = self.socket_path.clone();
        let mut send_command = |command: &str| Self::send_command_to_socket(&socket_path, command);
        self.seek_with(seconds, is_shared, &mut send_command, &mut std::thread::sleep)
    }

    pub fn get_queue(&self) -> Vec<TrackMetadata> {
        self.queue.clone()
    }

    pub fn play_index(&mut self, index: usize) -> Result<(), String> {
        self.ensure_startup_queue_ready()?;
        if index >= self.queue.len() {
            return Err(format!(
                "Índice fora da fila: {} para {} faixa(s).",
                index,
                self.queue.len()
            ));
        }
        self.send_command(&format!("play {}", index))?;
        self.restored_resume = None;
        Ok(())
    }

    pub fn clear_queue(&mut self) -> Result<(), String> {
        self.send_command("clear")?;
        self.queue.clear();
        self.restored_resume = None;
        self.startup_restore = StartupQueueRestore::Ready;
        self.save_queue_cache()
    }

    pub fn list_directory(&self, path: &str) -> Result<Vec<LocalItem>, String> {
        let cmd = if path.trim().is_empty() {
            "lsinfo".to_string()
        } else {
            format!("lsinfo {}", Self::quote_mpd_argument(path)?)
        };

        let lines = self.send_command(&cmd)?;
        let mut items = Vec::new();
        let mut current_item: Option<LocalItem> = None;

        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "directory" => {
                        if let Some(item) = current_item.take() {
                            items.push(item);
                        }
                        let name = v.split('/').last().unwrap_or(v).to_string();
                        current_item = Some(LocalItem {
                            item_type: "directory".to_string(),
                            path: v.to_string(),
                            name,
                            title: None,
                            artist: None,
                            album: None,
                            duration: None,
                        });
                    }
                    "file" => {
                        if let Some(item) = current_item.take() {
                            items.push(item);
                        }
                        let name = v.split('/').last().unwrap_or(v).to_string();
                        current_item = Some(LocalItem {
                            item_type: "file".to_string(),
                            path: v.to_string(),
                            name,
                            title: None,
                            artist: None,
                            album: None,
                            duration: None,
                        });
                    }
                    "Title" => {
                        if let Some(ref mut item) = current_item {
                            item.title = Some(v.to_string());
                        }
                    }
                    "Artist" => {
                        if let Some(ref mut item) = current_item {
                            item.artist = Some(v.to_string());
                        }
                    }
                    "Album" => {
                        if let Some(ref mut item) = current_item {
                            item.album = Some(v.to_string());
                        }
                    }
                    "duration" | "Time" | "time" | "Duration" => {
                        if let Some(ref mut item) = current_item {
                            if item.duration.is_none() {
                                let clean = v.trim();
                                if let Ok(secs) = clean.parse::<f64>() {
                                    item.duration = Some(secs);
                                } else if let Some((m, s)) = clean.split_once(':') {
                                    if let (Ok(m_val), Ok(s_val)) = (m.trim().parse::<f64>(), s.trim().parse::<f64>()) {
                                        item.duration = Some(m_val * 60.0 + s_val);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(item) = current_item {
            items.push(item);
        }

        Ok(items)
    }

    pub fn local_media_exists(&self, uri: &str) -> Result<bool, String> {
        let command = format!("find file {}", Self::quote_mpd_argument(uri)?);
        let lines = self.send_command(&command)?;
        Ok(lines.iter().any(|line| {
            line.strip_prefix("file: ")
                .is_some_and(|candidate| candidate == uri)
        }))
    }

    pub fn get_status(&self) -> Result<PlaybackStatus, String> {
        let lines = self.send_command("status")?;

        let mut state = None;
        let mut elapsed = 0.0;
        let mut duration = 0.0;
        let mut audio_format = String::new();
        let mut song_index: Option<usize> = None;
        let mut volume: i32 = 100;
        let mut is_updating = false;

        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "state" => {
                        Self::parse_playback_state(v)?;
                        state = Some(v.to_string());
                    }
                    "elapsed" => elapsed = Self::parse_mpd_number("elapsed", v)?,
                    "duration" => duration = Self::parse_mpd_number("duration", v)?,
                    "audio" => audio_format = v.to_string(),
                    "song" => song_index = Some(Self::parse_mpd_number("song", v)?),
                    "volume" => volume = Self::parse_mpd_number("volume", v)?,
                    "updating_db" => is_updating = true,
                    _ => {}
                }
            }
        }
        let state = state.ok_or_else(|| {
            "Resposta de status do MPD não contém o estado de reprodução.".to_string()
        })?;

        // Se o MPD estiver parado e sem nenhuma faixa ativa, retorna estado neutro e limpo
        if state == "stop" && song_index.is_none() {
            if self.startup_restore == StartupQueueRestore::Ready {
                if let Some(resume) = self.restored_resume.as_ref().filter(|resume| resume.valid_for(self.queue.len())) {
                    let track = &self.queue[resume.queue_index];
                    let local_uri = match &track.media_locator {
                        Some(MediaLocator::Local { uri }) => Some(uri.as_str()),
                        Some(MediaLocator::Plex { .. }) => None,
                        None if !track.uri.starts_with("http://") && !track.uri.starts_with("https://") => Some(track.uri.as_str()),
                        None => None,
                    };
                    return Ok(PlaybackStatus {
                        state: "pause".to_string(),
                        elapsed: resume.elapsed,
                        duration: track.duration.unwrap_or(0.0),
                        audio_format,
                        current_media: self.current_media_for_queue_index(Some(resume.queue_index)),
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        album: track.album.clone(),
                        thumb: track.thumb.clone().or_else(|| local_uri.and_then(|uri| self.resolve_cover(uri))),
                        plex_image: track.plex_image.clone(),
                        volume: VolumeStatus::from_mpd(volume),
                        is_updating,
                    });
                }
            }
            return Ok(PlaybackStatus {
                state,
                elapsed: 0.0,
                duration: 0.0,
                audio_format,
                current_media: None,
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                thumb: None,
                plex_image: None,
                volume: VolumeStatus::from_mpd(volume),
                is_updating,
            });
        }

        let song_lines = self.send_command("currentsong")?;
        let mut current_file = String::new();
        let mut current_song_index = None;
        let mut tag_title = String::new();
        let mut tag_artist = String::new();
        let mut tag_album = String::new();

        for line in song_lines {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "file" => current_file = v.to_string(),
                    "Title" => tag_title = v.to_string(),
                    "Artist" => tag_artist = v.to_string(),
                    "Album" => tag_album = v.to_string(),
                    "Pos" => {
                        current_song_index = Some(Self::parse_mpd_number("Pos", v)?);
                    }
                    _ => {}
                }
            }
        }

        let current_media_queue_index =
            Self::consistent_queue_index(song_index, current_song_index);
        if song_index.is_none() {
            song_index = current_song_index;
        }

        let mut title = String::new();
        let mut artist = String::new();
        let mut album = String::new();
        let mut thumb = None;
        let mut plex_image = None;

        if let Some(idx) = song_index {
            if let Some(track) = self.queue.get(idx) {
                title = track.title.clone();
                artist = track.artist.clone();
                album = track.album.clone();
                thumb = track.thumb.clone();
                plex_image = track.plex_image.clone();
                if duration <= 0.0 {
                    if let Some(d) = track.duration {
                        duration = d;
                    }
                }
            }
        }

        if (title.is_empty() || thumb.is_none()) && !current_file.is_empty() {
            if let Some(track) = self.queue.iter().find(|t| t.uri == current_file) {
                if title.is_empty() {
                    title = track.title.clone();
                }
                if artist.is_empty() {
                    artist = track.artist.clone();
                }
                if album.is_empty() {
                    album = track.album.clone();
                }
                if thumb.is_none() {
                    thumb = track.thumb.clone();
                }
                if plex_image.is_none() {
                    plex_image = track.plex_image.clone();
                }
                if duration <= 0.0 {
                    if let Some(d) = track.duration {
                        duration = d;
                    }
                }
            }
        }

        if title.is_empty() {
            title = if !tag_title.is_empty() {
                tag_title
            } else if !current_file.is_empty() {
                let clean = current_file.split('?').next().unwrap_or(&current_file);
                clean.split('/').last().unwrap_or(clean).to_string()
            } else {
                String::new()
            };
        }

        if artist.is_empty() && !tag_artist.is_empty() {
            artist = tag_artist;
        }
        if album.is_empty() && !tag_album.is_empty() {
            album = tag_album;
        }

        if thumb.is_none() && !current_file.is_empty() {
            thumb = self.resolve_cover(&current_file);
        }

        let current_media = self.current_media_for_queue_index(current_media_queue_index);

        Ok(PlaybackStatus {
            state,
            elapsed,
            duration,
            audio_format,
            current_media,
            title,
            artist,
            album,
            thumb,
            plex_image,
            volume: VolumeStatus::from_mpd(volume),
            is_updating,
        })
    }
}

pub struct AudioState(pub Mutex<AudioEngine>);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    static ALBUM_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn seek_status(state: &str, elapsed: f64) -> Result<Vec<String>, String> {
        Ok(vec![
            format!("state: {state}"),
            format!("elapsed: {elapsed}"),
        ])
    }

    fn scripted_seek(
        engine: &mut AudioEngine,
        seconds: f64,
        is_shared: bool,
        responses: Vec<Result<Vec<String>, String>>,
    ) -> (Result<(), String>, Vec<String>, Vec<Duration>) {
        let mut responses = VecDeque::from(responses);
        let mut commands = Vec::new();
        let mut sleeps = Vec::new();
        let result = engine.seek_with(
            seconds,
            is_shared,
            &mut |command| {
                commands.push(command.to_string());
                responses.pop_front().expect("resposta MPD simulada ausente")
            },
            &mut |duration| sleeps.push(duration),
        );
        assert!(responses.is_empty(), "respostas MPD simuladas não consumidas");
        (result, commands, sleeps)
    }

    fn album_test_lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    fn collect_album_tracks(
        tracks: &[(&str, Option<&str>, Option<&str>, Option<&str>)],
    ) -> HashMap<String, AlbumCollector> {
        let mut lines = Vec::new();
        for (file, album, artist, album_artist) in tracks {
            lines.push(format!("file: {file}"));
            if let Some(value) = album {
                lines.push(format!("Album: {value}"));
            }
            if let Some(value) = artist {
                lines.push(format!("Artist: {value}"));
            }
            if let Some(value) = album_artist {
                lines.push(format!("AlbumArtist: {value}"));
            }
        }
        let mut collection = LocalAlbumCollection::default();
        assert_eq!(
            AudioEngine::collect_local_album_page(&mut collection, lines).unwrap(),
            tracks.len()
        );
        AudioEngine::canonical_local_album_collectors(collection)
    }

    fn album_id(folder: &str, title: &str) -> String {
        format!(
            "{}:{}:{}",
            folder.len(),
            folder,
            title.trim().to_lowercase()
        )
    }

    fn collect_album_pages(pages: Vec<Vec<String>>) -> LocalAlbumCollection {
        let mut collection = LocalAlbumCollection::default();
        for page in pages {
            AudioEngine::collect_local_album_page(&mut collection, page).unwrap();
        }
        collection
    }

    fn cue_fixture_lines(
        folder: &str,
        album: &str,
        physical_tracks: usize,
        cue_tracks: usize,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        for index in 1..=physical_tracks {
            lines.push(format!("file: {folder}/{index:02}.flac"));
            lines.push(format!("Album: {album}"));
        }
        for index in 1..=cue_tracks {
            lines.push(format!("file: {folder}/image.cue/track{index:04}"));
            lines.push(format!("Album: {album}"));
        }
        lines
    }

    fn resolved_album_tracks(
        tracks: &[(&str, Option<&str>, Option<&str>, Option<&str>)],
    ) -> HashMap<String, AlbumCollector> {
        let mut map = collect_album_tracks(tracks);
        AudioEngine::consolidate_dominant_album_titles(&mut map);
        map
    }

    fn consolidated_album_tracks(
        tracks: &[(&str, Option<&str>, Option<&str>, Option<&str>)],
    ) -> HashMap<String, AlbumCollector> {
        let mut map = resolved_album_tracks(tracks);
        AudioEngine::consolidate_local_album_discs(&mut map);
        map
    }

    #[test]
    fn disc_folder_names_are_strict_and_positive() {
        for (name, number) in [
            ("CD1", 1),
            ("CD01", 1),
            ("CD 2", 2),
            ("Disc-3", 3),
            ("disk_04", 4),
            ("disc999", 999),
            ("CD 2 (Dub Wise)", 2),
            ("CD 2 - Dub Wise", 2),
            ("CD 2 – Dub Wise", 2),
            ("CD 2 — Dub Wise", 2),
            ("CD 2: Dub Wise", 2),
            ("Disc 3 (Bonus)", 3),
            ("Disk 4 [Live]", 4),
            ("CD1 Extra", 1),
        ] {
            assert_eq!(AudioEngine::disc_folder_number(name), Some(number));
        }
        for name in [
            "CD Collection",
            "Disco Music",
            "The Compact Disc",
            "CD Singles",
            "Discography",
            "CD",
            "Disc",
            "Disk",
            "CD0",
            "CD 0",
            "Disc 0",
            "Disc-",
            "CD  1",
            "CD2Something",
            "CD-1-2",
            "CD4294967296",
        ] {
            assert_eq!(AudioEngine::disc_folder_number(name), None, "{name}");
        }
    }

    #[test]
    fn virtual_cue_tracks_are_detected_strictly() {
        for (path, expected) in [
            ("Artist/Album/album.cue/track0001", true),
            ("Artist/Album/ALBUM.CUE/track12", true),
            ("Artist/Album/album.cue/track", false),
            ("Artist/Album/album.cue/foo0001", false),
            ("Artist/Album/album.cue-not/track0001", false),
            ("Artist/Album/track0001.flac", false),
            ("Artist.cue/Album/track0001.flac", false),
        ] {
            assert_eq!(
                AudioEngine::virtual_cue_track_path(path).is_some(),
                expected,
                "{path}"
            );
        }

        let cue = AudioEngine::virtual_cue_track_path(
            "Brasilidades/Artist/Album/album.cue/track0004",
        )
        .unwrap();
        assert_eq!(cue.cue_directory, "Brasilidades/Artist/Album/album.cue");
        assert_eq!(cue.physical_folder, "Brasilidades/Artist/Album");
    }

    #[test]
    fn physical_tracks_win_over_a_duplicate_cue_album() {
        let collection = collect_album_pages(vec![cue_fixture_lines(
            "FLAC/Donovan/Album", "Donovan Album", 20, 20,
        )]);
        let albums = AudioEngine::finalize_local_album_collection(
            collection,
            &HashSet::from(["FLAC".to_string()]),
        );

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 20);
        assert_eq!(albums[0].folder_path, "FLAC/Donovan/Album");
        assert_eq!(albums[0].discs.len(), 1);
        assert_eq!(albums[0].discs[0].folder_path, "FLAC/Donovan/Album");
        assert_eq!(albums[0].source_id.as_deref(), Some("FLAC"));
    }

    #[test]
    fn single_image_cue_wins_without_becoming_a_one_track_album() {
        let collection = collect_album_pages(vec![cue_fixture_lines(
            "Music/Artist/Album", "Album", 1, 10,
        )]);
        let albums = AudioEngine::finalize_local_album_collection(
            collection,
            &HashSet::from(["Music".to_string()]),
        );

        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 10);
        assert_eq!(albums[0].folder_path, "Music/Artist/Album");
        assert_eq!(albums[0].discs.len(), 1);
        assert_eq!(
            albums[0].discs[0].folder_path,
            "Music/Artist/Album/image.cue"
        );
        assert_eq!(albums[0].discs[0].track_count, 10);
        assert_eq!(albums[0].source_id.as_deref(), Some("Music"));
    }

    #[test]
    fn one_physical_and_one_cue_track_choose_physical() {
        let collection = collect_album_pages(vec![cue_fixture_lines(
            "Music/Artist/Album", "Album", 1, 1,
        )]);
        let map = AudioEngine::canonical_local_album_collectors(collection);
        let album = &map[&album_id("Music/Artist/Album", "Album")];
        assert_eq!(album.track_count, 1);
        assert_eq!(album.discs[0].folder_path, "Music/Artist/Album");
    }

    #[test]
    fn ambiguous_multiple_cue_directories_prefer_physical() {
        let lines = album_test_lines(&[
            "file: Music/Artist/Album/image.flac",
            "Album: Album",
            "file: Music/Artist/Album/first.cue/track0001",
            "Album: Album",
            "file: Music/Artist/Album/second.cue/track0001",
            "Album: Album",
        ]);
        let map = AudioEngine::canonical_local_album_collectors(collect_album_pages(vec![lines]));
        let album = &map[&album_id("Music/Artist/Album", "Album")];
        assert_eq!(album.track_count, 1);
        assert_eq!(album.discs[0].folder_path, "Music/Artist/Album");
    }

    #[test]
    fn cue_only_and_physical_only_albums_remain_available() {
        let cue = AudioEngine::canonical_local_album_collectors(collect_album_pages(vec![
            cue_fixture_lines("Music/Artist/Cue", "Cue", 0, 4),
        ]));
        assert_eq!(cue[&album_id("Music/Artist/Cue", "Cue")].track_count, 4);
        assert_eq!(
            cue[&album_id("Music/Artist/Cue", "Cue")].discs[0].folder_path,
            "Music/Artist/Cue/image.cue"
        );

        let physical = AudioEngine::canonical_local_album_collectors(collect_album_pages(vec![
            cue_fixture_lines("Music/Artist/Physical", "Physical", 3, 0),
        ]));
        assert_eq!(
            physical[&album_id("Music/Artist/Physical", "Physical")].track_count,
            3
        );
        assert_eq!(
            physical[&album_id("Music/Artist/Physical", "Physical")].discs[0].folder_path,
            "Music/Artist/Physical"
        );
    }

    #[test]
    fn cue_choice_is_global_across_collection_pages() {
        let mut first_page = cue_fixture_lines("Music/Artist/Album", "Album", 1, 0);
        for index in 0..499 {
            first_page.push(format!("file: Music/Filler/{index:04}.flac"));
            first_page.push("Album: Filler".to_string());
        }
        let cue_page = cue_fixture_lines("Music/Artist/Album", "Album", 0, 10);
        let map = AudioEngine::canonical_local_album_collectors(collect_album_pages(vec![
            first_page, cue_page,
        ]));
        let album = &map[&album_id("Music/Artist/Album", "Album")];
        assert_eq!(album.track_count, 10);
        assert_eq!(
            album.discs[0].folder_path,
            "Music/Artist/Album/image.cue"
        );
    }

    #[test]
    fn different_album_titles_in_one_folder_are_not_canonicalized_together() {
        let mut lines = cue_fixture_lines("Music/Artist/Folder", "Physical Album", 2, 0);
        lines.extend(cue_fixture_lines("Music/Artist/Folder", "Cue Album", 0, 2));
        let albums = AudioEngine::finalize_local_album_collection(
            collect_album_pages(vec![lines]),
            &HashSet::new(),
        );
        assert_eq!(albums.len(), 2);
        assert_eq!(albums.iter().map(|album| album.track_count).sum::<usize>(), 4);
    }

    #[test]
    fn multidisc_title_suffix_matches_its_folder_number() {
        for (title, number) in [
            ("Album CD1", 1),
            ("Album CD 1", 1),
            ("Album CD.1", 1),
            ("Album CD-1", 1),
            ("Album CD_1", 1),
            ("Album Disc1", 1),
            ("Album Disc 1", 1),
            ("Album Disc.1", 1),
            ("Album Disk1", 1),
            ("Album Disk 1", 1),
            ("Album Disc-2", 2),
            ("Album Disk_03", 3),
            ("Album Disco 1", 1),
            ("Album Disco2", 2),
            ("Album Disco-1", 1),
            ("Album Disco_1", 1),
            ("Album Disco.1", 1),
            ("Album, Disco 1", 1),
            ("Album - Disco 1", 1),
            ("Album – Disco 2", 2),
            ("Album — Disco 2", 2),
            ("Album: Disco 1", 1),
            ("Album (CD1)", 1),
            ("Album (CD 2)", 2),
            ("Album (CD.1)", 1),
            ("Album (CD-1)", 1),
            ("Album (CD_1)", 1),
            ("Album (Disc 1)", 1),
            ("Album (Disk 2)", 2),
            ("Album (Disco 1)", 1),
            ("Album (cD1)", 1),
            ("Album CD.2 (Dub Wise)", 2),
            ("Album CD 2 - Bonus", 2),
            ("Album Disc 3 (Live)", 3),
            ("Album Disk 4 [Bonus]", 4),
        ] {
            assert_eq!(
                AudioEngine::multidisc_base_title(title, number).as_deref(),
                Some("Album"),
                "{title}"
            );
        }
        assert_eq!(AudioEngine::multidisc_base_title("Album CD1", 2), None);
        assert_eq!(AudioEngine::multidisc_base_title("Album Disco 2", 1), None);
        assert_eq!(AudioEngine::multidisc_base_title("Album (CD2)", 1), None);
        assert_eq!(
            AudioEngine::multidisc_base_title("CD Collection", 1).as_deref(),
            Some("CD Collection")
        );
        assert_eq!(
            AudioEngine::multidisc_base_title("Compact Disc", 1).as_deref(),
            Some("Compact Disc")
        );
        for title in ["Disco Music", "Disco Inferno", "Album Discography"] {
            assert_eq!(
                AudioEngine::multidisc_base_title(title, 1).as_deref(),
                Some(title),
                "{title}"
            );
        }
        for title in [
            "Album (CD1) Remaster",
            "History of Compact Disc (Documentary)",
            "Disco Music (Live)",
            "Discography (CD Collection)",
        ] {
            assert_eq!(
                AudioEngine::multidisc_base_title(title, 1).as_deref(),
                Some(title),
                "{title}"
            );
        }
    }

    #[test]
    fn parenthesized_cd_suffix_consolidates_real_albums() {
        for (root, title, first_path, second_path) in [
            (
                "Brasilidades/Egberto Gismonti/Saudações",
                "Saudações",
                "Brasilidades/Egberto Gismonti/Saudações/CD1/a.flac",
                "Brasilidades/Egberto Gismonti/Saudações/CD2/b.flac",
            ),
            (
                "Jazz/Paco de Lucia & John McLaughlin/Live At Montreux 1987",
                "Paco And John Live At Montreux 1987",
                "Jazz/Paco de Lucia & John McLaughlin/Live At Montreux 1987/CD1/a.flac",
                "Jazz/Paco de Lucia & John McLaughlin/Live At Montreux 1987/CD2/b.flac",
            ),
        ] {
            let first_title = format!("{title} (CD1)");
            let second_title = format!("{title} (CD2)");
            let map = consolidated_album_tracks(&[
                (first_path, Some(first_title.as_str()), None, None),
                (second_path, Some(second_title.as_str()), None, None),
            ]);

            assert_eq!(map.len(), 1, "{title}");
            let album = &map[&album_id(root, title)];
            assert_eq!(album.title, title);
            assert_eq!(album.track_count, 2);
            assert_eq!(album.discs.len(), 2);
            assert_eq!(
                album
                    .discs
                    .iter()
                    .map(|disc| (disc.number, disc.label.as_str()))
                    .collect::<Vec<_>>(),
                vec![(1, "CD1"), (2, "CD2")]
            );
        }
    }

    #[test]
    fn parenthesized_disc_tags_in_the_same_folder_remain_separate() {
        let map = consolidated_album_tracks(&[
            (
                "Artist/Black Is Our Colour/one.flac",
                Some("Black Is Our Colour (Disc 1)"),
                None,
                None,
            ),
            (
                "Artist/Black Is Our Colour/two.flac",
                Some("Black Is Our Colour (Disc 2)"),
                None,
                None,
            ),
        ]);

        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&album_id(
            "Artist/Black Is Our Colour",
            "Black Is Our Colour (Disc 1)"
        )));
        assert!(map.contains_key(&album_id(
            "Artist/Black Is Our Colour",
            "Black Is Our Colour (Disc 2)"
        )));
    }

    #[test]
    fn portuguese_disco_suffix_consolidates_the_real_cd_layout() {
        let map = consolidated_album_tracks(&[
            (
                "Brasilidades/Legião Urbana/Música P_Acampamentos/CD1/a.flac",
                Some("Música P/Acampamentos, Disco 1"),
                Some("Legião Urbana"),
                None,
            ),
            (
                "Brasilidades/Legião Urbana/Música P_Acampamentos/CD2/b.flac",
                Some("Música P/Acampamentos, Disco 2"),
                Some("Legião Urbana"),
                None,
            ),
        ]);
        let root = "Brasilidades/Legião Urbana/Música P_Acampamentos";
        assert_eq!(map.len(), 1);
        let album = &map[&album_id(root, "Música P/Acampamentos")];
        assert_eq!(album.title, "Música P/Acampamentos");
        assert_eq!(album.track_count, 2);
        assert_eq!(album.discs.len(), 2);
        assert_eq!(
            album
                .discs
                .iter()
                .map(|disc| (disc.number, disc.label.as_str(), disc.folder_path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    1,
                    "CD1",
                    "Brasilidades/Legião Urbana/Música P_Acampamentos/CD1"
                ),
                (
                    2,
                    "CD2",
                    "Brasilidades/Legião Urbana/Música P_Acampamentos/CD2"
                ),
            ]
        );
    }

    #[test]
    fn multi_disc_album_uses_root_identity_and_first_disc_folder() {
        let map = consolidated_album_tracks(&[
            (
                "Artist/Album/CD2/track.flac",
                Some("Album"),
                Some("Guest"),
                None,
            ),
            (
                "Artist/Album/CD1/track.flac",
                Some("Album"),
                Some("Main"),
                None,
            ),
            (
                "Artist/Album/CD1/track2.flac",
                Some("Album"),
                Some("Main"),
                None,
            ),
        ]);
        assert_eq!(map.len(), 1);
        let album = &map[&album_id("Artist/Album", "Album")];
        assert_eq!(album.folder_path, "Artist/Album/CD1");
        assert_eq!(album.track_count, 3);
        assert_eq!(
            album
                .discs
                .iter()
                .map(|disc| (
                    disc.number,
                    disc.label.as_str(),
                    disc.folder_path.as_str(),
                    disc.track_count
                ))
                .collect::<Vec<_>>(),
            vec![
                (1, "CD1", "Artist/Album/CD1", 2),
                (2, "CD2", "Artist/Album/CD2", 1),
            ]
        );
        assert_eq!(album.display_artist(), "Various Artists");
    }

    #[test]
    fn labeled_disc_folders_consolidate_and_preserve_labels() {
        let map = consolidated_album_tracks(&[
            (
                "Artist/Album/CD 2 (Dub Wise)/two.flac",
                Some("Album"),
                None,
                None,
            ),
            ("Artist/Album/CD 1/one.flac", Some("Album"), None, None),
        ]);
        assert_eq!(map.len(), 1);
        let album = &map[&album_id("Artist/Album", "Album")];
        assert_eq!(album.track_count, 2);
        assert_eq!(album.discs.len(), 2);
        assert_eq!(
            album
                .discs
                .iter()
                .map(|disc| (disc.number, disc.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "CD 1"), (2, "CD 2 (Dub Wise)")]
        );
    }

    #[test]
    fn album_tag_disc_suffixes_consolidate_the_real_labeled_disc_case() {
        let map = consolidated_album_tracks(&[
            (
                "Reggae/Clinton Fearon/03 - What A System - 1999 (2 CD)/CD 2 (Dub Wise)/two.flac",
                Some("\" What A System \" CD.2 (Dub Wise)"),
                None,
                Some("Clinton Fearon & The Boogie Brown Band"),
            ),
            (
                "Reggae/Clinton Fearon/03 - What A System - 1999 (2 CD)/CD 1/one.flac",
                Some("\" What A System \" CD.1"),
                None,
                Some("Clinton Fearon & The Boogie Brown Band"),
            ),
        ]);
        let root = "Reggae/Clinton Fearon/03 - What A System - 1999 (2 CD)";
        assert_eq!(map.len(), 1);
        let album = &map[&album_id(root, "\" What A System \"")];
        assert_eq!(album.title, "\" What A System \"");
        assert_eq!(album.track_count, 2);
        assert_eq!(
            album.display_artist(),
            "Clinton Fearon & The Boogie Brown Band"
        );
        assert_eq!(album.folder_path, format!("{root}/CD 1"));
        assert_eq!(album.discs.len(), 2);
        assert_eq!(
            album
                .discs
                .iter()
                .map(|disc| (disc.number, disc.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "CD 1"), (2, "CD 2 (Dub Wise)")]
        );
    }

    #[test]
    fn unsuffixed_and_matching_suffixed_album_tags_consolidate() {
        let map = consolidated_album_tracks(&[
            ("Root/Album/CD1/one.flac", Some("Album Name"), None, None),
            (
                "Root/Album/CD2/two.flac",
                Some("Album Name CD2"),
                None,
                None,
            ),
        ]);
        assert_eq!(map.len(), 1);
        let album = &map[&album_id("Root/Album", "Album Name")];
        assert_eq!(album.title, "Album Name");
        assert_eq!(album.track_count, 2);
    }

    #[test]
    fn multidisc_title_suffixes_do_not_merge_ambiguous_albums() {
        for tracks in [
            [
                ("Root/CD1/one.flac", Some("First Album CD1"), None, None),
                ("Root/CD2/two.flac", Some("Second Album CD2"), None, None),
            ],
            [
                ("Root/CD1/one.flac", Some("Album Name CD1"), None, None),
                ("Root/CD2/two.flac", Some("Album Name CD3"), None, None),
            ],
            [
                ("Root A/CD1/one.flac", Some("Album Name CD1"), None, None),
                ("Root B/CD2/two.flac", Some("Album Name CD2"), None, None),
            ],
            [
                ("Root/First/one.flac", Some("Album Name CD1"), None, None),
                ("Root/Second/two.flac", Some("Album Name CD2"), None, None),
            ],
        ] {
            assert_eq!(consolidated_album_tracks(&tracks).len(), 2);
        }
    }

    #[test]
    fn multi_disc_consolidation_rejects_single_and_ambiguous_folders() {
        let map = consolidated_album_tracks(&[
            ("Artist/Album/CD1/one.flac", Some("Album"), None, None),
            ("Artist/Other/CD2/two.flac", Some("Album"), None, None),
            (
                "Artist/Album/CD2/three.flac",
                Some("Other title"),
                None,
                None,
            ),
            ("Artist/Album/Regular/four.flac", Some("Album"), None, None),
        ]);
        assert_eq!(map.len(), 4);
        assert!(map.contains_key(&album_id("Artist/Album/CD1", "Album")));
        assert!(map.contains_key(&album_id("Artist/Other/CD2", "Album")));
        assert!(map.contains_key(&album_id("Artist/Album/CD2", "Other title")));
        assert!(map.contains_key(&album_id("Artist/Album/Regular", "Album")));

        let duplicate_number = consolidated_album_tracks(&[
            ("Artist/Album/CD1/one.flac", Some("Album"), None, None),
            ("Artist/Album/Disc01/two.flac", Some("Album"), None, None),
        ]);
        assert_eq!(duplicate_number.len(), 2);

        let normal =
            consolidated_album_tracks(&[("Artist/Album/one.flac", Some("Album"), None, None)]);
        let album = &normal[&album_id("Artist/Album", "Album")];
        assert_eq!(album.discs.len(), 1);
        assert_eq!(album.discs[0].folder_path, "Artist/Album");
        assert_eq!(album.discs[0].track_count, 1);
    }

    #[test]
    fn multi_disc_artist_and_album_artist_use_all_tracks() {
        let mut tracks = Vec::new();
        for index in 0..10 {
            tracks.push((
                if index == 0 {
                    "Artist/Album/CD1/one.flac"
                } else {
                    "Artist/Album/CD1/other.flac"
                },
                Some("Album"),
                Some("Big Youth"),
                None,
            ));
        }
        for index in 0..10 {
            tracks.push((
                "Artist/Album/CD2/track.flac",
                Some("Album"),
                Some(if index < 8 { "Big Youth" } else { "Guest" }),
                None,
            ));
        }
        let map = consolidated_album_tracks(&tracks);
        let album = &map[&album_id("Artist/Album", "Album")];
        assert_eq!(album.track_count, 20);
        assert_eq!(album.display_artist(), "Big Youth");

        let map = consolidated_album_tracks(&[
            (
                "Artist/Album/CD1/one.flac",
                Some("Album"),
                Some("A"),
                Some("The Group"),
            ),
            (
                "Artist/Album/CD2/two.flac",
                Some("Album"),
                Some("B"),
                Some("the group"),
            ),
        ]);
        assert_eq!(
            map[&album_id("Artist/Album", "Album")].display_artist(),
            "the group"
        );

        let map = consolidated_album_tracks(&[
            (
                "Artist/Album/CD1/one.flac",
                Some("Album"),
                Some("A"),
                Some("The Group"),
            ),
            ("Artist/Album/CD2/two.flac", Some("Album"), Some("B"), None),
        ]);
        assert_eq!(
            map[&album_id("Artist/Album", "Album")].display_artist(),
            "Various Artists"
        );
    }

    #[test]
    fn multi_disc_album_survives_page_boundary() {
        let mut collection = LocalAlbumCollection::default();
        let mut first_page = Vec::new();
        for index in 0..500 {
            first_page.push(format!("file: Artist/Album/CD1/{index}.flac"));
            first_page.push("Album: Album".to_string());
            first_page.push("Artist: Big Youth".to_string());
        }
        assert_eq!(
            AudioEngine::collect_local_album_page(&mut collection, first_page).unwrap(),
            500
        );
        assert_eq!(
            AudioEngine::collect_local_album_page(
                &mut collection,
                album_test_lines(&[
                    "file: Artist/Album/CD2/one.flac",
                    "Album: Album",
                    "Artist: Guest",
                ])
            )
            .unwrap(),
            1
        );
        let mut map = AudioEngine::canonical_local_album_collectors(collection);
        AudioEngine::consolidate_local_album_discs(&mut map);
        assert_eq!(map.len(), 1);
        let album = &map[&album_id("Artist/Album", "Album")];
        assert_eq!(album.track_count, 501);
        assert_eq!(album.discs.len(), 2);
        assert_eq!(album.display_artist(), "Big Youth");
    }

    fn source_test_disc(folder_path: &str) -> LocalAlbumDisc {
        LocalAlbumDisc {
            number: 0,
            label: "Disc".to_string(),
            folder_path: folder_path.to_string(),
            track_count: 1,
        }
    }

    #[test]
    fn local_album_source_uses_only_a_valid_virtual_root_component() {
        let valid = HashSet::from([
            "Reggae".to_string(),
            "Brasilidades".to_string(),
            "Music (2)".to_string(),
        ]);
        for (path, expected) in [
            ("Reggae/Artist/Album", Some("Reggae")),
            ("Brasilidades/Album", Some("Brasilidades")),
            ("Music (2)/Album", Some("Music (2)")),
            ("Reggae", Some("Reggae")),
            ("Unknown/Album", None),
            ("", None),
            ("../Reggae/Album", None),
            ("Reggae/../Album", None),
            ("/Reggae/Album", None),
        ] {
            assert_eq!(
                AudioEngine::local_album_source_id(&[source_test_disc(path)], &valid).as_deref(),
                expected,
                "unexpected source for {path:?}"
            );
        }
    }

    #[test]
    fn local_album_source_requires_all_discs_to_share_the_same_source() {
        let valid = HashSet::from(["Reggae".to_string(), "Jazz".to_string()]);
        assert_eq!(
            AudioEngine::local_album_source_id(
                &[
                    source_test_disc("Reggae/Album/CD1"),
                    source_test_disc("Reggae/Album/CD2"),
                ],
                &valid,
            )
            .as_deref(),
            Some("Reggae")
        );
        assert_eq!(
            AudioEngine::local_album_source_id(
                &[
                    source_test_disc("Reggae/Album/CD1"),
                    source_test_disc("Jazz/Album/CD2"),
                ],
                &valid,
            ),
            None
        );
    }

    #[test]
    fn paginated_album_query_returns_one_multi_disc_album() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base '')\"", "songs: 2\nOK\n"),
            ("find \"(base '')\" window 0:2", "file: Artist/Album/CD2/two.flac\nAlbum: Album\nArtist: B\nfile: Artist/Album/CD1/one.flac\nAlbum: Album\nArtist: A\nOK\n"),
        ]);
        let albums = AudioEngine::get_local_albums_at(
            &path,
            &HashSet::from(["Artist".to_string()]),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, album_id("Artist/Album", "Album"));
        assert_eq!(albums[0].folder_path, "Artist/Album/CD1");
        assert_eq!(albums[0].source_id.as_deref(), Some("Artist"));
        assert_eq!(albums[0].track_count, 2);
        assert_eq!(
            albums[0]
                .discs
                .iter()
                .map(|disc| disc.number)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    fn fake_album_mpd(responses: Vec<(&'static str, &'static str)>) -> (String, thread::JoinHandle<()>) {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("sonante-albums-{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            for (expected, response) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.23.5\n").unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut command).unwrap();
                assert_eq!(command, format!("{expected}\n"));
                stream.write_all(response.as_bytes()).unwrap();
            }
            std::fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    fn fake_startup_restore_mpd(expected: String) -> (String, thread::JoinHandle<()>) {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("sonante-resume-{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.23.5\n").unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut actual = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                actual.push_str(&line);
                if line == "command_list_end\n" { break; }
            }
            assert_eq!(actual.trim_end(), expected);
            stream.write_all(b"OK\n").unwrap();
            std::fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    fn cover_test_dir() -> PathBuf {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sonante-cover-{}-{id}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    fn fake_cover_mpd(responses: Vec<(String, Vec<u8>)>) -> (String, thread::JoinHandle<()>) {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("sonante-cover-{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            for (expected, response) in responses {
                let started = std::time::Instant::now();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && started.elapsed() < Duration::from_secs(3) =>
                        {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("Conexão MPD simulada ausente: {error}"),
                    }
                };
                stream.write_all(b"OK MPD 0.23.5\n").unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut command)
                    .unwrap();
                assert_eq!(command, format!("{expected}\n"));
                stream.write_all(&response).unwrap();
            }
            std::fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    fn picture_response(size: usize, mime: Option<&str>, bytes: &[u8]) -> Vec<u8> {
        let mut response = format!("size: {size}\n").into_bytes();
        if let Some(kind) = mime {
            response.extend_from_slice(format!("type: {kind}\n").as_bytes());
        }
        response.extend_from_slice(format!("binary: {}\n", bytes.len()).as_bytes());
        response.extend_from_slice(bytes);
        response.extend_from_slice(b"\nOK\n");
        response
    }

    fn lsinfo_files(folder: &str, names: &[&str]) -> Vec<u8> {
        let mut response = String::new();
        for name in names {
            response.push_str(&format!("file: {folder}/{name}\n"));
        }
        response.push_str("OK\n");
        response.into_bytes()
    }

    #[test]
    fn local_cover_has_priority_without_contacting_mpd() {
        let dir = cover_test_dir();
        let folder = dir.join("Music");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("cover.jpg"), b"local cover").unwrap();
        let cover =
            AudioEngine::get_local_cover_at("/nonexistent/mpd.socket", &dir, "Music").unwrap();
        assert_eq!(
            cover,
            Some(format!(
                "data:image/jpeg;base64,{}",
                BASE64.encode(b"local cover")
            ))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn embedded_jpeg_png_and_webp_are_returned_as_safe_data_uris() {
        for (kind, bytes) in [
            ("image/jpeg", b"\xff\xd8\xffJPEG".as_slice()),
            ("image/png", b"\x89PNG\r\n\x1a\nPNG".as_slice()),
            ("image/webp", b"RIFF1234WEBPdata".as_slice()),
        ] {
            let dir = cover_test_dir();
            let (socket, server) = fake_cover_mpd(vec![
                (
                    "lsinfo \"Music\"".into(),
                    lsinfo_files("Music", &["one.flac"]),
                ),
                (
                    "readpicture \"Music/one.flac\" 0".into(),
                    picture_response(bytes.len(), Some(kind), bytes),
                ),
            ]);
            let cover = AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap();
            assert_eq!(
                cover,
                Some(format!("data:{kind};base64,{}", BASE64.encode(bytes)))
            );
            server.join().unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn empty_picture_and_ack_try_the_next_direct_track() {
        let dir = cover_test_dir();
        let jpeg = b"\xff\xd8\xffnext";
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music\"".into(),
                lsinfo_files("Music", &["one.flac", "two.flac"]),
            ),
            ("readpicture \"Music/one.flac\" 0".into(), b"OK\n".to_vec()),
            (
                "readpicture \"Music/two.flac\" 0".into(),
                picture_response(jpeg.len(), Some("image/jpeg"), jpeg),
            ),
        ]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            Some(format!("data:image/jpeg;base64,{}", BASE64.encode(jpeg)))
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();

        let dir = cover_test_dir();
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music\"".into(),
                lsinfo_files("Music", &["one.flac"]),
            ),
            (
                "readpicture \"Music/one.flac\" 0".into(),
                b"ACK [5@0] {readpicture} unsupported\n".to_vec(),
            ),
        ]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            None
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn embedded_cover_probes_at_most_three_direct_tracks() {
        let dir = cover_test_dir();
        let mut responses = vec![(
            "lsinfo \"Music\"".into(),
            lsinfo_files("Music", &["1.flac", "2.flac", "3.flac", "4.flac"]),
        )];
        for name in ["1.flac", "2.flac", "3.flac"] {
            responses.push((format!("readpicture \"Music/{name}\" 0"), b"OK\n".to_vec()));
        }
        let (socket, server) = fake_cover_mpd(responses);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            None
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_picture_is_rejected_before_binary_download() {
        let dir = cover_test_dir();
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music\"".into(),
                lsinfo_files("Music", &["one.flac"]),
            ),
            (
                "readpicture \"Music/one.flac\" 0".into(),
                format!("size: {}\n", MAX_LOCAL_COVER_BYTES + 1).into_bytes(),
            ),
        ]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            None
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn picture_chunks_are_joined_at_exact_offsets() {
        let dir = cover_test_dir();
        let jpeg = b"\xff\xd8\xff12345";
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music\"".into(),
                lsinfo_files("Music", &["one.flac"]),
            ),
            (
                "readpicture \"Music/one.flac\" 0".into(),
                picture_response(jpeg.len(), Some("image/jpeg"), &jpeg[..3]),
            ),
            (
                "readpicture \"Music/one.flac\" 3".into(),
                picture_response(jpeg.len(), Some("image/jpeg"), &jpeg[3..]),
            ),
        ]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            Some(format!("data:image/jpeg;base64,{}", BASE64.encode(jpeg)))
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_or_truncated_picture_fails_without_panicking() {
        for response in [
            b"size: 2\nbinary: 3\n".to_vec(),
            b"size: 4\nbinary: 4\n\xff\xd8".to_vec(),
            b"size: 999999999999999999999999999999\n".to_vec(),
        ] {
            let dir = cover_test_dir();
            let (socket, server) = fake_cover_mpd(vec![
                (
                    "lsinfo \"Music\"".into(),
                    lsinfo_files("Music", &["one.flac"]),
                ),
                ("readpicture \"Music/one.flac\" 0".into(), response),
            ]);
            assert!(AudioEngine::get_local_cover_at(&socket, &dir, "Music").is_err());
            server.join().unwrap();
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn unknown_mime_is_not_embedded_in_data_uri() {
        let dir = cover_test_dir();
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music\"".into(),
                lsinfo_files("Music", &["one.flac"]),
            ),
            (
                "readpicture \"Music/one.flac\" 0".into(),
                picture_response(4, Some("image/svg+xml"), b"<svg"),
            ),
        ]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            None
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_mime_uses_known_image_signature_only() {
        let dir = cover_test_dir();
        let jpeg = b"\xff\xd8\xffimage";
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music\"".into(),
                lsinfo_files("Music", &["one.flac"]),
            ),
            (
                "readpicture \"Music/one.flac\" 0".into(),
                picture_response(jpeg.len(), None, jpeg),
            ),
        ]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            Some(format!("data:image/jpeg;base64,{}", BASE64.encode(jpeg)))
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn picture_uri_with_spaces_and_quotes_is_escaped() {
        let dir = cover_test_dir();
        let folder = "Music \"Mix\"";
        let jpeg = b"\xff\xd8\xffyes";
        let (socket, server) = fake_cover_mpd(vec![
            (
                "lsinfo \"Music \\\"Mix\\\"\"".into(),
                lsinfo_files(folder, &["01 song.flac"]),
            ),
            (
                "readpicture \"Music \\\"Mix\\\"/01 song.flac\" 0".into(),
                picture_response(jpeg.len(), Some("image/jpeg"), jpeg),
            ),
        ]);
        assert!(AudioEngine::get_local_cover_at(&socket, &dir, folder)
            .unwrap()
            .is_some());
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn folder_without_direct_tracks_has_no_embedded_cover() {
        let dir = cover_test_dir();
        let (socket, server) = fake_cover_mpd(vec![(
            "lsinfo \"Music\"".into(),
            b"directory: Music/Sub\nfile: Music/Sub/deep.flac\nOK\n".to_vec(),
        )]);
        assert_eq!(
            AudioEngine::get_local_cover_at(&socket, &dir, "Music").unwrap(),
            None
        );
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn library_update_and_rescan_send_distinct_mpd_commands() {
        for command in ["update", "rescan"] {
            let (path, server) = fake_album_mpd(vec![(command, "updating_db: 1\nOK\n")]);
            let audio = AudioEngine {
                socket_path: path,
                music_dir: String::new(),
                queue: Vec::new(),
                restored_resume: None,
                startup_restore: StartupQueueRestore::Ready,
            };
            if command == "update" {
                AudioEngine::update_library(&audio.socket_path).unwrap();
            } else {
                audio.rescan_library().unwrap();
            }
            server.join().unwrap();
        }

        let (path, server) = fake_album_mpd(vec![("update", "ACK [5@0] {update} failure\n")]);
        let audio = AudioEngine {
            socket_path: path,
            music_dir: String::new(),
            queue: Vec::new(),
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        };
        assert!(AudioEngine::update_library(&audio.socket_path).is_err());
        server.join().unwrap();
    }

    #[test]
    fn local_album_windows_cover_exact_count() {
        for (count, expected) in [
            (0, vec![]),
            (1, vec![(0, 1)]),
            (500, vec![(0, 500)]),
            (501, vec![(0, 500), (500, 501)]),
            (1234, vec![(0, 500), (500, 1000), (1000, 1234)]),
        ] {
            assert_eq!(AudioEngine::local_album_windows(count), expected);
        }
    }

    #[test]
    fn local_album_count_rejects_missing_invalid_and_overflow() {
        assert_eq!(AudioEngine::parse_local_album_count(&album_test_lines(&["songs: 0"])).unwrap(), 0);
        for lines in [
            album_test_lines(&[]),
            album_test_lines(&["playtime: 1"]),
            album_test_lines(&["songs: nope"]),
            album_test_lines(&["songs: -1"]),
            album_test_lines(&["songs: 999999999999999999999999999999999999999999"]),
            album_test_lines(&["songs: 1", "songs: 2"]),
        ] {
            assert!(AudioEngine::parse_local_album_count(&lines).is_err());
        }
    }

    #[test]
    fn local_album_pages_preserve_aggregation_and_reset_tags() {
        let mut collection = LocalAlbumCollection::default();
        assert_eq!(AudioEngine::collect_local_album_page(&mut collection, album_test_lines(&[
            "file: Música/Disco/primeira.flac", "Album: Mesmo", "Artist: Faixa", "AlbumArtist: Álbum", "Date: 2024-03-01",
            "file: Música/Sem Album/primeira.flac",
        ])).unwrap(), 2);
        assert_eq!(AudioEngine::collect_local_album_page(&mut collection, album_test_lines(&[
            "file: Música/Disco/segunda.flac", "Album: Mesmo", "Artist: Faixa", "AlbumArtist: Álbum",
            "file: Música/Sem Album/segunda.flac",
        ])).unwrap(), 2);
        let map = AudioEngine::canonical_local_album_collectors(collection);
        let shared = &map[&album_id("Música/Disco", "Mesmo")];
        assert_eq!(shared.track_count, 2);
        assert_eq!(shared.display_artist(), "Álbum");
        assert_eq!(shared.year.as_deref(), Some("2024"));
        assert_eq!(shared.folder_path, "Música/Disco");
        let fallback = &map[&album_id("Música/Sem Album", "Sem Album")];
        assert_eq!(fallback.track_count, 2);
        assert_eq!(fallback.title, "Sem Album");
        assert_eq!(fallback.year, None);
        assert_eq!(fallback.display_artist(), "Artista Desconhecido");
    }

    #[test]
    fn local_album_crossing_500_song_boundary_counts_both_tracks() {
        let mut collection = LocalAlbumCollection::default();
        let mut first_page = Vec::new();
        for index in 0..500 {
            first_page.push(format!("file: Disco/faixa-{index}.flac"));
            first_page.push("Album: Mesmo".to_string());
        }
        assert_eq!(AudioEngine::collect_local_album_page(&mut collection, first_page).unwrap(), 500);
        assert_eq!(AudioEngine::collect_local_album_page(&mut collection, album_test_lines(&[
            "file: Disco/faixa-500.flac", "Album: Mesmo",
        ])).unwrap(), 1);
        let map = AudioEngine::canonical_local_album_collectors(collection);
        assert_eq!(map[&album_id("Disco", "Mesmo")].track_count, 501);
    }

    #[test]
    fn local_album_identity_uses_exact_folder_and_normalized_title() {
        let map = collect_album_tracks(&[
            (
                "Pasta A/faixa 1.flac",
                Some(" Greatest Hits "),
                Some("Artist A"),
                None,
            ),
            (
                "Pasta A/faixa 2.flac",
                Some("greatest hits"),
                Some("Artist A"),
                None,
            ),
            (
                "Pasta B/faixa 1.flac",
                Some("Greatest Hits"),
                Some("Artist A"),
                None,
            ),
            (
                "pasta A/faixa 1.flac",
                Some("Greatest Hits"),
                Some("Artist A"),
                None,
            ),
            (
                "Pasta A/faixa 3.flac",
                Some("Second Album"),
                Some("Artist A"),
                None,
            ),
            ("Pasta A/faixa 4.flac", None, Some("Artist A"), None),
        ]);
        assert_eq!(map.len(), 5);
        assert_eq!(map[&album_id("Pasta A", "Greatest Hits")].track_count, 2);
        assert_eq!(
            map[&album_id("Pasta A", "Greatest Hits")].folder_path,
            "Pasta A"
        );
        assert!(map.contains_key(&album_id("Pasta B", "Greatest Hits")));
        assert!(map.contains_key(&album_id("pasta A", "Greatest Hits")));
        assert!(map.contains_key(&album_id("Pasta A", "Second Album")));
        assert_eq!(map[&album_id("Pasta A", "Pasta A")].title, "Pasta A");
        assert_eq!(
            map[&album_id("Pasta A", "Greatest Hits")].display_artist(),
            "Artist A"
        );
    }

    #[test]
    fn dominant_album_title_absorbs_lobao_tag_outlier() {
        let mut tracks = vec![(
            "Lobão/Acústico/guest.flac",
            Some("Acústico MTV"),
            None,
            None,
        )];
        tracks.extend(
            std::iter::repeat((
                "Lobão/Acústico/track.flac",
                Some("Acústico Lobão"),
                None,
                None,
            ))
            .take(17),
        );
        let map = resolved_album_tracks(&tracks);
        assert_eq!(map.len(), 1);
        let album = &map[&album_id("Lobão/Acústico", "Acústico Lobão")];
        assert_eq!(album.title, "Acústico Lobão");
        assert_eq!(album.folder_path, "Lobão/Acústico");
        assert_eq!(album.track_count, 18);
        assert_eq!(album.discs.len(), 1);
        assert_eq!(album.discs[0].track_count, 18);
    }

    #[test]
    fn normalized_album_title_display_is_independent_of_track_order() {
        let tracks = [
            ("Folder/one.flac", Some("Album"), None, None),
            ("Folder/two.flac", Some("album"), None, None),
        ];
        let first = resolved_album_tracks(&tracks);
        let reversed = resolved_album_tracks(&[tracks[1], tracks[0]]);
        let id = album_id("Folder", "Album");
        assert_eq!(first[&id].title, reversed[&id].title);
        assert_eq!(first[&id].track_count, 2);
    }

    #[test]
    fn dominant_album_title_uses_all_tracks_and_exact_eighty_percent() {
        for (a, b, c, expected_groups) in [
            (8, 2, 0, 1),
            (7, 3, 0, 2),
            (5, 5, 0, 2),
            (16, 3, 1, 1),
            (15, 3, 2, 3),
        ] {
            let mut tracks = Vec::new();
            for (title, count) in [("Album A", a), ("Album B", b), ("Album C", c)] {
                tracks.extend(
                    std::iter::repeat(("Artist/Folder/track.flac", Some(title), None, None))
                        .take(count),
                );
            }
            let map = resolved_album_tracks(&tracks);
            assert_eq!(map.len(), expected_groups, "{a}/{b}/{c}");
            if expected_groups == 1 {
                let album = &map[&album_id("Artist/Folder", "Album A")];
                assert_eq!(album.title, "Album A");
                assert_eq!(album.track_count, a + b + c);
            } else {
                assert_eq!(map[&album_id("Artist/Folder", "Album A")].track_count, a);
                assert_eq!(map[&album_id("Artist/Folder", "Album B")].track_count, b);
                if c > 0 {
                    assert_eq!(map[&album_id("Artist/Folder", "Album C")].track_count, c);
                }
            }
        }
    }

    #[test]
    fn dominant_album_title_stays_within_exact_folder_and_uses_missing_album_fallback() {
        let mut tracks = Vec::new();
        tracks.extend(
            std::iter::repeat(("Folder A/track.flac", Some("Album A"), None, None)).take(8),
        );
        tracks.extend(std::iter::repeat(("Folder A/other.flac", None, None, None)).take(2));
        tracks.push(("Folder B/track.flac", Some("Album A"), None, None));
        let map = resolved_album_tracks(&tracks);
        assert_eq!(map.len(), 2);
        assert_eq!(map[&album_id("Folder A", "Album A")].track_count, 10);
        assert_eq!(map[&album_id("Folder B", "Album A")].track_count, 1);

        let map = resolved_album_tracks(&[("Folder C/track.flac", None, None, None)]);
        assert_eq!(map[&album_id("Folder C", "Folder C")].title, "Folder C");
    }

    #[test]
    fn dominant_album_title_merges_artist_and_album_artist_statistics() {
        for (guest_album_artist, expected) in [(Some("Album Band"), "Album Band"), (None, "Main")] {
            let mut tracks = Vec::new();
            tracks.extend(
                std::iter::repeat((
                    "Artist/Folder/track.flac",
                    Some("Album A"),
                    Some("Main"),
                    Some("Album Band"),
                ))
                .take(8),
            );
            tracks.extend(
                std::iter::repeat((
                    "Artist/Folder/guest.flac",
                    Some("Album B"),
                    Some("Guest"),
                    guest_album_artist,
                ))
                .take(2),
            );
            let map = resolved_album_tracks(&tracks);
            assert_eq!(
                map[&album_id("Artist/Folder", "Album A")].display_artist(),
                expected
            );
        }
    }

    #[test]
    fn dominant_album_title_counts_across_page_boundary() {
        let mut collection = LocalAlbumCollection::default();
        let mut first_page = Vec::new();
        for index in 0..500 {
            first_page.push(format!("file: Artist/Folder/{index}.flac"));
            first_page.push(format!(
                "Album: {}",
                if index < 400 { "Album A" } else { "Album B" }
            ));
        }
        assert_eq!(
            AudioEngine::collect_local_album_page(&mut collection, first_page).unwrap(),
            500
        );
        assert_eq!(
            AudioEngine::collect_local_album_page(
                &mut collection,
                album_test_lines(&["file: Artist/Folder/500.flac", "Album: Album A",])
            )
            .unwrap(),
            1
        );
        let mut map = AudioEngine::canonical_local_album_collectors(collection);
        AudioEngine::consolidate_dominant_album_titles(&mut map);
        assert_eq!(map.len(), 1);
        assert_eq!(map[&album_id("Artist/Folder", "Album A")].track_count, 501);
    }

    #[test]
    fn local_album_artist_frequency_selects_dominant_or_various() {
        for (dominant, guest, expected) in [
            (18, 2, "Big Youth"),
            (10, 10, "Various Artists"),
            (4, 1, "Big Youth"),
            (3, 1, "Various Artists"),
        ] {
            let mut tracks = Vec::new();
            for _ in 0..dominant {
                tracks.push((
                    "Disco/faixa.flac",
                    Some("Progress"),
                    Some("Big Youth"),
                    None,
                ));
            }
            for index in 0..guest {
                let artist = if index == 0 {
                    "Big Youth & Ark Angels"
                } else {
                    "Ark Angels"
                };
                tracks.push(("Disco/convidada.flac", Some("Progress"), Some(artist), None));
            }
            let map = collect_album_tracks(&tracks);
            assert_eq!(map.len(), 1);
            let album = &map[&album_id("Disco", "Progress")];
            assert_eq!(album.track_count, dominant + guest);
            assert_eq!(album.display_artist(), expected);
        }
    }

    #[test]
    fn local_album_artist_requires_consistent_album_artist_on_every_track() {
        for (second_album_artist, expected) in [
            (Some(" THE GROUP "), "The Group"),
            (None, "Track Artist"),
            (Some("Other Group"), "Track Artist"),
        ] {
            let map = collect_album_tracks(&[
                (
                    "Disco/um.flac",
                    Some("Album"),
                    Some("Track Artist"),
                    Some("The Group"),
                ),
                (
                    "Disco/dois.flac",
                    Some("Album"),
                    Some("Track Artist"),
                    second_album_artist,
                ),
            ]);
            assert_eq!(map[&album_id("Disco", "Album")].display_artist(), expected);
        }
        let map = collect_album_tracks(&[
            ("Disco/um.flac", Some("Album"), None, None),
            ("Disco/dois.flac", Some("Album"), Some(" "), Some(" ")),
        ]);
        assert_eq!(
            map[&album_id("Disco", "Album")].display_artist(),
            "Artista Desconhecido"
        );
    }

    #[test]
    fn local_album_artist_display_is_independent_of_track_order() {
        let first = collect_album_tracks(&[
            ("Disco/um.flac", Some("Album"), Some("BIG YOUTH"), None),
            ("Disco/dois.flac", Some("Album"), Some("Big Youth"), None),
        ]);
        let reversed = collect_album_tracks(&[
            ("Disco/dois.flac", Some("Album"), Some("Big Youth"), None),
            ("Disco/um.flac", Some("Album"), Some("BIG YOUTH"), None),
        ]);
        assert_eq!(
            first[&album_id("Disco", "Album")].display_artist(),
            "Big Youth"
        );
        assert_eq!(
            first[&album_id("Disco", "Album")].display_artist(),
            reversed[&album_id("Disco", "Album")].display_artist()
        );
    }

    #[test]
    fn local_album_artist_stats_cross_page_boundary() {
        let mut collection = LocalAlbumCollection::default();
        let mut first_page = Vec::new();
        for index in 0..500 {
            first_page.push(format!("file: Disco/faixa-{index}.flac"));
            first_page.push("Album: Progress".to_string());
            first_page.push(format!(
                "Artist: {}",
                if index < 400 { "Big Youth" } else { "Guest" }
            ));
        }
        assert_eq!(
            AudioEngine::collect_local_album_page(&mut collection, first_page).unwrap(),
            500
        );
        assert_eq!(
            collection.physical[&album_id("Disco", "Progress")].display_artist(),
            "Big Youth"
        );
        assert_eq!(
            AudioEngine::collect_local_album_page(
                &mut collection,
                album_test_lines(&[
                    "file: Disco/faixa-500.flac",
                    "Album: Progress",
                    "Artist: Guest",
                ])
            )
            .unwrap(),
            1
        );
        let map = AudioEngine::canonical_local_album_collectors(collection);
        assert_eq!(map.len(), 1);
        assert_eq!(map[&album_id("Disco", "Progress")].track_count, 501);
        assert_eq!(
            map[&album_id("Disco", "Progress")].display_artist(),
            "Various Artists"
        );
    }

    #[test]
    fn local_album_query_uses_count_and_window_and_handles_empty_library() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base '')\"", "songs: 1\nplaytime: 20\nOK\n"),
            ("find \"(base '')\" window 0:1", "file: Raiz/Disco/faixa.flac\nAlbum: Disco\nOK\n"),
        ]);
        let albums = AudioEngine::get_local_albums_at(
            &path,
            &HashSet::from(["Raiz".to_string()]),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, album_id("Raiz/Disco", "Disco"));
        assert_eq!(albums[0].folder_path, "Raiz/Disco");
        assert_eq!(albums[0].source_id.as_deref(), Some("Raiz"));
        assert_eq!(albums[0].track_count, 1);

        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base '')\"", "songs: 0\nOK\n"),
        ]);
        assert!(AudioEngine::get_local_albums_at(&path, &HashSet::new())
            .unwrap()
            .is_empty());
        server.join().unwrap();
    }

    #[test]
    fn local_album_query_rejects_update_and_page_failures() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "updating_db: 12\nstate: stop\nOK\n"),
        ]);
        assert!(AudioEngine::get_local_albums_at(&path, &HashSet::new())
            .unwrap_err()
            .contains("atualização"));
        server.join().unwrap();

        for response in ["ACK [5@0] {find} failure\n", "file: Disco/faixa.flac\n", "OK\n"] {
            let (path, server) = fake_album_mpd(vec![
                ("status", "state: stop\nOK\n"),
                ("count \"(base '')\"", "songs: 1\nOK\n"),
                ("find \"(base '')\" window 0:1", response),
            ]);
            assert!(AudioEngine::get_local_albums_at(&path, &HashSet::new()).is_err());
            server.join().unwrap();
        }
    }

    #[test]
    fn volume_backends_have_stable_public_names() {
        let serialized = [
            (VolumeBackend::AlsaHardware, "\"alsa_hardware\""),
            (VolumeBackend::PipeWire, "\"pipewire\""),
            (VolumeBackend::MpdSoftware, "\"mpd_software\""),
            (VolumeBackend::Unavailable, "\"unavailable\""),
        ];

        for (backend, expected) in serialized {
            assert_eq!(serde_json::to_string(&backend).unwrap(), expected);
        }
    }

    #[test]
    fn unavailable_mpd_volume_has_no_public_percentage_backend() {
        let volume = VolumeStatus::from_mpd(-1);

        assert_eq!(volume.value, 0);
        assert!(!volume.available);
        assert!(!volume.writable);
        assert_eq!(volume.backend, VolumeBackend::Unavailable);
    }

    fn engine_with_track(uri: &str) -> AudioEngine {
        AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: vec![TrackMetadata {
                title: "Faixa".to_string(),
                artist: "Artista".to_string(),
                album: "Álbum".to_string(),
                thumb: None,
                plex_image: None,
                media_locator: None,
                uri: uri.to_string(),
                duration: Some(120.0),
            }],
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        }
    }

    fn engine_with_two_tracks() -> AudioEngine {
        let mut engine = engine_with_track("álbum/faixa.flac");
        engine.queue.push(TrackMetadata {
            title: "Segunda".to_string(),
            artist: "Artista".to_string(),
            album: "Álbum".to_string(),
            thumb: None,
            plex_image: None,
            media_locator: None,
            uri: "álbum/segunda.flac".to_string(),
            duration: Some(180.0),
        });
        engine
    }

    fn plex_track() -> TrackMetadata {
        TrackMetadata {
            title: "Faixa Plex".to_string(),
            artist: "Artista".to_string(),
            album: "Álbum".to_string(),
            thumb: None,
            plex_image: None,
            media_locator: Some(MediaLocator::Plex {
                server_id: "server-1".to_string(),
                part_key: "/library/parts/10/file.flac".to_string(),
                rating_key: Some("track-10".to_string()),
                file_path: Some("/srv/music/file.flac".to_string()),
            }),
            uri: String::new(),
            duration: Some(120.0),
        }
    }

    #[test]
    fn compact_queue_roundtrip_preserves_metadata_without_artwork_or_rewrite() {
        let mut local = engine_with_track("Álbum/uma \\\"faixa\\\".flac")
            .queue
            .remove(0);
        local.media_locator = Some(MediaLocator::Local {
            uri: local.uri.clone(),
        });
        local.thumb = Some(format!("data:image/png;base64,{}", "A".repeat(20_000)));
        let mut plex = plex_track();
        plex.plex_image = Some(PlexImageRef {
            server_id: "server-1".into(),
            path: "/library/metadata/10/thumb".into(),
        });
        plex.thumb = Some("data:image/jpeg;base64,SECRET_ARTWORK".into());
        let original = vec![local, plex];
        let json = AudioEngine::encode_queue_cache(&original).unwrap();
        let serialized = std::str::from_utf8(&json).unwrap();
        assert!(serialized.contains("\"version\":2"));
        assert!(!serialized.contains("data:image"));
        assert!(!serialized.contains("SECRET_ARTWORK"));
        assert!(!serialized.contains("X-Plex-Token"));
        let (loaded, changed) = AudioEngine::decode_queue_cache(json.as_slice()).unwrap();
        assert!(!changed);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().all(|track| track.thumb.is_none()));
        assert_eq!(loaded[0].title, original[0].title);
        assert_eq!(loaded[0].artist, original[0].artist);
        assert_eq!(loaded[0].album, original[0].album);
        assert_eq!(loaded[0].uri, original[0].uri);
        assert_eq!(loaded[0].duration, original[0].duration);
        assert_eq!(loaded[0].media_locator, original[0].media_locator);
        assert_eq!(loaded[1].media_locator, original[1].media_locator);
        assert_eq!(loaded[1].plex_image, original[1].plex_image);
        let second = AudioEngine::encode_queue_cache(&loaded).unwrap();
        assert_eq!(second, json);
        assert!(
            !AudioEngine::decode_queue_cache(second.as_slice())
                .unwrap()
                .1
        );
    }

    #[test]
    fn large_runtime_artwork_does_not_grow_persisted_queue() {
        let mut tracks = vec![engine_with_track("music/file.flac").queue.remove(0); 100];
        let baseline = AudioEngine::encode_queue_cache(&tracks).unwrap();
        for track in &mut tracks {
            track.thumb = Some(format!("data:image/jpeg;base64,{}", "Q".repeat(12_000)));
        }
        let with_artwork = AudioEngine::encode_queue_cache(&tracks).unwrap();
        assert_eq!(with_artwork, baseline);
        assert!(!with_artwork.windows(10).any(|part| part == b"data:image"));
    }

    #[test]
    fn legacy_cache_migrates_once_and_discards_artwork() {
        let mut local = engine_with_track("local/faixa.flac").queue.remove(0);
        local.thumb = Some("data:image/png;base64,ARTWORK_BYTES".into());
        let mut plex = plex_track();
        plex.uri = "https://old.invalid/file?X-Plex-Token=SECRET".into();
        plex.thumb =
            Some("https://old.invalid/library/metadata/42/thumb?X-Plex-Token=SECRET".into());
        let legacy = serde_json::to_vec(&vec![local, plex]).unwrap();
        let (queue, changed) = AudioEngine::decode_queue_cache(legacy.as_slice()).unwrap();
        assert!(changed);
        assert_eq!(queue.len(), 2);
        assert!(queue.iter().all(|track| track.thumb.is_none()));
        assert!(queue[1].uri.is_empty());
        assert!(queue[1].plex_image.is_some());
        let compact = AudioEngine::encode_queue_cache(&queue).unwrap();
        assert!(!String::from_utf8_lossy(&compact).contains("SECRET"));
        assert!(!String::from_utf8_lossy(&compact).contains("ARTWORK_BYTES"));
        assert!(
            !AudioEngine::decode_queue_cache(compact.as_slice())
                .unwrap()
                .1
        );
    }

    #[test]
    fn invalid_and_empty_cache_are_safe() {
        assert!(AudioEngine::decode_queue_cache(b"{broken".as_slice()).is_err());
        assert!(AudioEngine::decode_queue_cache(b"".as_slice()).is_err());
        let (legacy_empty, changed) = AudioEngine::decode_queue_cache(b"[]".as_slice()).unwrap();
        assert!(legacy_empty.is_empty());
        assert!(changed);
        let compact = AudioEngine::encode_queue_cache(&legacy_empty).unwrap();
        let (empty, changed) = AudioEngine::decode_queue_cache(compact.as_slice()).unwrap();
        assert!(empty.is_empty());
        assert!(!changed);
    }

    #[test]
    fn test_audio_engine_cache_path_is_private_temp_not_home() {
        let path = get_queue_cache_path().unwrap();
        assert!(path.starts_with(std::env::temp_dir()));
        if let Ok(home) = std::env::var("HOME") {
            assert_ne!(path, Path::new(&home).join(".config/sonante/queue_cache.json"));
        }
        let _engine = AudioEngine::new("/no-test-mpd", "/no-test-music");
    }

    #[test]
    fn resume_cache_roundtrip_and_invalid_cursor_preserve_queue() {
        let queue = engine_with_track("Álbum/uma faixa.flac").queue;
        let resume = PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 201.4,
            state: PlaybackState::Playing,
        };
        let bytes = AudioEngine::encode_queue_cache_with_resume(&queue, Some(&resume)).unwrap();
        let (loaded, restored, changed) = AudioEngine::decode_queue_cache_with_resume(bytes.as_slice()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(restored, Some(resume));
        assert!(!changed);
        assert!(!String::from_utf8_lossy(&bytes).contains("data:image"));

        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for bad in [
            serde_json::json!({"queue_index": 1, "elapsed": 1.0, "state": "paused"}),
            serde_json::json!({"queue_index": 0, "elapsed": -1.0, "state": "paused"}),
            serde_json::json!({"queue_index": 0, "elapsed": 1.0, "state": "unknown"}),
        ] {
            value["resume"] = bad;
            let bytes = serde_json::to_vec(&value).unwrap();
            let (tracks, cursor, _) = AudioEngine::decode_queue_cache_with_resume(bytes.as_slice()).unwrap();
            assert_eq!(tracks.len(), 1);
            assert!(cursor.is_none());
        }
        let invalid_number = PlaybackResumeSnapshot { elapsed: f64::NAN, ..resume };
        assert!(!invalid_number.valid_for(queue.len()));
        let empty = AudioEngine::encode_queue_cache_with_resume(&[], Some(&resume)).unwrap();
        assert!(AudioEngine::decode_queue_cache_with_resume(empty.as_slice()).unwrap().1.is_none());
    }

    #[test]
    fn capture_and_shutdown_persist_resume_without_stop_or_rewrite_on_load() {
        let (socket, server) = fake_album_mpd(vec![
            ("status", "state: play\nsong: 0\nelapsed: 201.4\nOK\n"),
            ("status", "state: play\nsong: 0\nelapsed: 201.4\nOK\n"),
        ]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket.clone();
        engine.persist_shutdown_resume().unwrap();
        let path = get_queue_cache_path().unwrap();
        let before = std::fs::metadata(&path).unwrap();
        engine.persist_shutdown_resume().unwrap();
        server.join().unwrap();
        let saved = std::fs::read(&path).unwrap();
        assert!(String::from_utf8_lossy(&saved).contains("\"resume\""));
        let restored = AudioEngine::new(&socket, "/test-music");
        assert_eq!(restored.restored_resume.unwrap().elapsed, 201.4);
        let after = std::fs::metadata(&path).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), after.ino());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn paused_snapshot_is_saved_and_stopped_without_selection_is_not() {
        let (socket, server) = fake_album_mpd(vec![("status", "state: pause\nsong: 0\nelapsed: 9.5\nOK\n")]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        let captured = engine.capture_playback_snapshot().unwrap().unwrap();
        assert_eq!(captured.state, PlaybackState::Paused);
        assert_eq!(captured.elapsed, 9.5);
        server.join().unwrap();

        let (socket, server) = fake_album_mpd(vec![("status", "state: stop\nOK\n")]);
        engine.socket_path = socket;
        assert!(engine.capture_playback_snapshot().unwrap().is_none());
        server.join().unwrap();

        let (socket, server) = fake_album_mpd(vec![("status", "state: stop\nsong: 0\nelapsed: 9.5\nOK\n")]);
        engine.socket_path = socket;
        assert!(engine.capture_playback_snapshot().unwrap().is_none());
        server.join().unwrap();
        engine.queue.clear();
        assert!(engine.capture_playback_snapshot().unwrap().is_none());
    }

    #[test]
    fn startup_queue_commands_only_clear_and_add_even_for_resume() {
        let engine = engine_with_track("Álbum/uma \\\"faixa\\\".flac");
        let commands = engine.device_switch_queue_restore_commands(None, &[engine.queue[0].uri.clone()]).unwrap();
        assert!(commands.starts_with("command_list_begin\nclear\nadd "));
        assert!(commands.ends_with("command_list_end"));
        assert!(!commands.lines().any(|line| line.starts_with("play") || line.starts_with("pause") || line.starts_with("seek")));
    }

    #[test]
    fn startup_restore_sends_only_clear_add_and_unblocks_queue_click() {
        let mut engine = engine_with_track("Álbum/uma \\\"faixa\\\".flac");
        engine.startup_restore = StartupQueueRestore::Pending;
        assert!(engine.play_index(0).is_err());
        let uri = engine.queue[0].uri.clone();
        let expected = engine.device_switch_queue_restore_commands(None, &[uri.clone()]).unwrap();
        let (socket, server) = fake_startup_restore_mpd(expected);
        engine.socket_path = socket;
        engine.restore_startup_queue(&[uri]).unwrap();
        assert_eq!(engine.startup_restore, StartupQueueRestore::Ready);
        server.join().unwrap();
    }

    #[test]
    fn failed_startup_restore_preserves_logical_queue_and_blocks_play() {
        let mut engine = engine_with_track("local/faixa.flac");
        engine.startup_restore = StartupQueueRestore::Pending;
        let (socket, server) = fake_album_mpd(vec![("command_list_begin", "ACK [5@0] {add} missing\n")]);
        engine.socket_path = socket;
        assert!(engine.restore_startup_queue(&["local/faixa.flac".into()]).is_err());
        assert_eq!(engine.startup_restore, StartupQueueRestore::Failed);
        assert_eq!(engine.queue.len(), 1);
        assert!(engine.play_index(0).is_err());
        server.join().unwrap();
    }

    #[test]
    fn restored_status_is_paused_with_track_position_and_stable_plex_artwork() {
        let (socket, server) = fake_album_mpd(vec![("status", "state: stop\nvolume: 70\nOK\n")]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 201.4, state: PlaybackState::Playing });
        let status = engine.get_status().unwrap();
        assert_eq!(status.state, "pause");
        assert_eq!(status.elapsed, 201.4);
        assert_eq!(status.title, "Faixa");
        assert_eq!(status.duration, 120.0);
        assert_eq!(status.current_media, engine.current_media_for_queue_index(Some(0)));
        server.join().unwrap();

        let (socket, server) = fake_album_mpd(vec![("status", "state: stop\nOK\n")]);
        engine.socket_path = socket;
        engine.queue = vec![plex_track()];
        engine.queue[0].plex_image = Some(PlexImageRef { server_id: "server-1".into(), path: "/library/metadata/10/thumb".into() });
        let status = engine.get_status().unwrap();
        assert_eq!(status.plex_image, engine.queue[0].plex_image);
        assert_eq!(status.current_media, engine.current_media_for_queue_index(Some(0)));
        server.join().unwrap();
    }

    #[test]
    fn play_after_resume_selects_then_seeks_and_queue_click_selects() {
        let (socket, server) = fake_album_mpd(vec![("play 0", "OK\n"), ("seekcur 201.4", "OK\n"), ("status", "state: play\nOK\n"), ("play 0", "OK\n")]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 201.4, state: PlaybackState::Paused });
        engine.toggle_play_pause().unwrap();
        assert!(engine.restored_resume.is_none());
        engine.play_index(0).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn seeking_restored_cursor_updates_position_without_mpd_command() {
        let mut engine = engine_with_track("local/faixa.flac");
        engine.queue[0].thumb = Some("data:image/png;base64,ARTWORK_BYTES".to_string());
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 9.0, state: PlaybackState::Paused });
        engine.seek(33.2, true).unwrap();
        assert_eq!(engine.restored_resume.unwrap().elapsed, 33.2);
        assert!(engine.seek(f64::NAN, true).is_err());
        let path = get_queue_cache_path().unwrap();
        use std::os::unix::fs::MetadataExt;
        let inode = std::fs::metadata(&path).unwrap().ino();
        engine.seek(33.2, true).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("data:image"));
        let (_, cached_resume, _) = AudioEngine::decode_queue_cache_with_resume(bytes.as_slice()).unwrap();
        assert_eq!(cached_resume.unwrap().elapsed, 33.2);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn resume_not_seekable_plays_from_start_but_other_ack_is_error() {
        let (socket, server) = fake_album_mpd(vec![
            ("play 0", "OK\n"),
            ("seekcur 9.0", "ACK [5@0] {seekcur} Not seekable\n"),
            ("seekcur 9.0", "ACK [5@0] {seekcur} Not seekable\n"),
            ("seekcur 9.0", "ACK [5@0] {seekcur} Not seekable\n"),
            ("status", "state: pause\nOK\n"),
            ("pause 0", "OK\n"),
        ]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 9.0, state: PlaybackState::Playing });
        assert!(engine.toggle_play_pause().is_ok());
        assert!(engine.restored_resume.is_none());
        server.join().unwrap();

        let (socket, server) = fake_album_mpd(vec![("play 0", "OK\n"), ("seekcur 9.0", "ACK [5@0] {seekcur} failure\n")]);
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 9.0, state: PlaybackState::Paused });
        assert!(engine.toggle_play_pause().is_err());
        assert!(engine.restored_resume.is_none());
        server.join().unwrap();
    }

    #[test]
    fn one_play_after_logical_seek_unpauses_mpd_when_needed() {
        let (socket, server) = fake_album_mpd(vec![
            ("play 0", "OK\n"),
            ("seekcur 180.0", "OK\n"),
            ("status", "state: pause\nOK\n"),
            ("pause 0", "OK\n"),
            ("seekcur 181.0", "OK\n"),
        ]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 63.0, state: PlaybackState::Playing });
        engine.seek(180.0, true).unwrap();
        engine.toggle_play_pause().unwrap();
        assert!(engine.restored_resume.is_none());
        engine.seek(181.0, false).unwrap();
        server.join().unwrap();
        std::fs::remove_file(get_queue_cache_path().unwrap()).unwrap();
    }

    #[test]
    fn active_seek_does_not_send_pause_or_play_and_stopped_does_not_create_cursor() {
        for (state, response) in [
            ("play", "state: play\nsong: 0\nelapsed: 120.0\nOK\n"),
            ("pause", "state: pause\nsong: 0\nelapsed: 120.0\nOK\n"),
        ] {
            let (socket, server) = fake_album_mpd(vec![
                ("seekcur 120.0", "OK\n"),
                ("status", response),
                ("currentsong", "Pos: 0\nfile: local/faixa.flac\nOK\n"),
            ]);
            let mut engine = engine_with_track("local/faixa.flac");
            engine.socket_path = socket;
            engine.seek(120.0, false).unwrap();
            assert_eq!(engine.get_status().unwrap().state, state);
            assert!(engine.restored_resume.is_none());
            server.join().unwrap();
        }
        let (socket, server) = fake_album_mpd(vec![("seekcur 120.0", "ACK [5@0] {seekcur} Not playing\n")]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        assert!(engine.seek(120.0, false).is_err());
        assert!(engine.restored_resume.is_none());
        server.join().unwrap();
    }

    #[test]
    fn direct_seek_never_polls_or_recovers() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, sleeps) = scripted_seek(&mut engine, 114.0, false, vec![Ok(vec![])]);
        result.unwrap();
        assert_eq!(commands, vec!["seekcur 114.0"]);
        assert!(sleeps.is_empty());
    }

    #[test]
    fn shared_paused_seek_does_not_resume_or_poll_after_seek() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, sleeps) = scripted_seek(
            &mut engine,
            114.0,
            true,
            vec![seek_status("pause", 57.4), Ok(vec![])],
        );
        result.unwrap();
        assert_eq!(commands, vec!["status", "seekcur 114.0"]);
        assert!(sleeps.is_empty());
    }

    #[test]
    fn shared_playing_seek_with_progress_needs_no_recovery() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, sleeps) = scripted_seek(
            &mut engine,
            114.0,
            true,
            vec![
                seek_status("play", 57.4),
                Ok(vec![]),
                seek_status("play", 113.98),
                seek_status("play", 114.06),
            ],
        );
        result.unwrap();
        assert_eq!(commands, vec!["status", "seekcur 114.0", "status", "status"]);
        assert_eq!(sleeps, vec![Duration::from_millis(80); 2]);
    }

    #[test]
    fn shared_playing_frozen_seek_recovers_once_with_pause_resume() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, sleeps) = scripted_seek(
            &mut engine,
            114.0,
            true,
            vec![
                seek_status("play", 57.4),
                Ok(vec![]),
                seek_status("play", 113.98),
                seek_status("play", 113.98),
                Ok(vec![]),
                Ok(vec![]),
                seek_status("play", 113.98),
                seek_status("play", 114.07),
            ],
        );
        result.unwrap();
        assert_eq!(
            commands,
            vec![
                "status",
                "seekcur 114.0",
                "status",
                "status",
                "pause 1",
                "pause 0",
                "status",
                "status",
            ]
        );
        assert_eq!(commands.iter().filter(|command| command.as_str() == "pause 1").count(), 1);
        assert_eq!(sleeps, vec![Duration::from_millis(80); 3]);
    }

    #[test]
    fn shared_seek_recovery_failures_are_explicit_and_bounded() {
        for (responses, expected_command) in [
            (
                vec![
                    seek_status("play", 57.4),
                    Ok(vec![]),
                    seek_status("play", 113.98),
                    seek_status("play", 113.98),
                    Err("ACK [5@0] {pause} failure".to_string()),
                ],
                "pause 1",
            ),
            (
                vec![
                    seek_status("play", 57.4),
                    Ok(vec![]),
                    seek_status("play", 113.98),
                    seek_status("play", 113.98),
                    Ok(vec![]),
                    Err("ACK [5@0] {pause} failure".to_string()),
                ],
                "pause 0",
            ),
        ] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, responses);
            assert!(result.is_err());
            assert_eq!(commands.last().map(String::as_str), Some(expected_command));
        }

        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_seek(
            &mut engine,
            114.0,
            true,
            vec![
                seek_status("play", 57.4),
                Ok(vec![]),
                seek_status("play", 113.98),
                seek_status("play", 113.98),
                Ok(vec![]),
                Ok(vec![]),
                seek_status("play", 113.98),
                seek_status("play", 113.98),
            ],
        );
        assert!(result.unwrap_err().contains("permaneceu congelada"));
        assert_eq!(commands.iter().filter(|command| command.as_str() == "pause 1").count(), 1);
    }

    #[test]
    fn shared_seek_that_stops_is_an_error_without_recovery() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_seek(
            &mut engine,
            114.0,
            true,
            vec![
                seek_status("play", 57.4),
                Ok(vec![]),
                seek_status("stop", 113.98),
                seek_status("stop", 113.98),
            ],
        );
        assert!(result.unwrap_err().contains("parou inesperadamente"));
        assert!(!commands.iter().any(|command| command.starts_with("pause")));
    }

    #[test]
    fn next_previous_and_queue_click_discard_restored_cursor() {
        for command in ["next", "previous", "play 0"] {
            let (socket, server) = fake_album_mpd(vec![(command, "OK\n")]);
            let mut engine = engine_with_track("local/faixa.flac");
            engine.socket_path = socket;
            engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 9.0, state: PlaybackState::Paused });
            match command {
                "next" => engine.next().unwrap(),
                "previous" => engine.previous().unwrap(),
                _ => engine.play_index(0).unwrap(),
            }
            assert!(engine.restored_resume.is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn synthetic_local_cover_is_runtime_only() {
        let dir = cover_test_dir();
        let album_dir = dir.join("Álbum");
        std::fs::create_dir(&album_dir).unwrap();
        std::fs::write(album_dir.join("cover.jpg"), b"fake-image").unwrap();
        let (socket, server) = fake_album_mpd(vec![("status", "state: stop\nOK\n")]);
        let mut engine = engine_with_track("Álbum/faixa.flac");
        engine.socket_path = socket;
        engine.music_dir = dir.to_string_lossy().into_owned();
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 2.0, state: PlaybackState::Paused });
        let status = engine.get_status().unwrap();
        assert!(status.thumb.as_deref().unwrap().starts_with("data:image"));
        let json = AudioEngine::encode_queue_cache_with_resume(&engine.queue, engine.restored_resume.as_ref()).unwrap();
        assert!(!String::from_utf8_lossy(&json).contains("data:image"));
        server.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn current_plex_track_uses_only_stable_queue_identity() {
        let mut track = plex_track();
        if let Some(MediaLocator::Plex { file_path, .. }) = &mut track.media_locator {
            *file_path = None;
        }
        let engine = AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: vec![track],
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        };

        assert_eq!(
            engine.current_media_for_queue_index(Some(0)),
            Some(CurrentMedia::Plex {
                server_id: "server-1".to_string(),
                part_key: "/library/parts/10/file.flac".to_string(),
                queue_index: 0,
            })
        );
    }

    #[test]
    fn current_local_track_preserves_its_uri() {
        let engine = engine_with_track("álbum/faixa.flac");

        assert_eq!(
            engine.current_media_for_queue_index(Some(0)),
            Some(CurrentMedia::Local {
                uri: "álbum/faixa.flac".to_string(),
                queue_index: 0,
            })
        );
    }

    #[test]
    fn current_media_is_none_when_queue_index_is_missing_or_out_of_bounds() {
        let engine = engine_with_track("faixa.flac");

        assert_eq!(engine.current_media_for_queue_index(None), None);
        assert_eq!(engine.current_media_for_queue_index(Some(1)), None);
        assert_eq!(
            AudioEngine::consistent_queue_index(Some(0), Some(1)),
            None
        );
    }

    #[test]
    fn serialized_playback_status_never_exposes_authenticated_plex_uri() {
        let mut track = plex_track();
        track.uri =
            "https://plex.invalid/library/parts/10/file.flac?X-Plex-Token=SECRET".to_string();
        let engine = AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: vec![track],
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        };
        let status = PlaybackStatus {
            state: "play".to_string(),
            elapsed: 1.0,
            duration: 120.0,
            audio_format: "44100:16:2".to_string(),
            current_media: engine.current_media_for_queue_index(Some(0)),
            title: "Faixa Plex".to_string(),
            artist: "Artista".to_string(),
            album: "Álbum".to_string(),
            thumb: None,
            plex_image: None,
            volume: VolumeStatus::from_mpd(100),
            is_updating: false,
        };

        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("server-1"));
        assert!(json.contains("/library/parts/10/file.flac"));
        assert!(!json.contains("current_file"));
        assert!(!json.contains("https://"));
        assert!(!json.contains("X-Plex-Token"));
        assert!(!json.contains("SECRET"));
    }

    #[test]
    fn plex_local_mount_remains_identified_as_plex() {
        let mut track = plex_track();
        track.uri = "/mnt/plex/Álbum/faixa.flac".to_string();
        let engine = AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: vec![track],
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        };

        assert!(matches!(
            engine.current_media_for_queue_index(Some(0)),
            Some(CurrentMedia::Plex {
                ref server_id,
                ref part_key,
                queue_index: 0,
            }) if server_id == "server-1" && part_key == "/library/parts/10/file.flac"
        ));
    }

    #[test]
    fn new_plex_queue_entry_serializes_only_stable_media_reference() {
        let json = serde_json::to_string(&plex_track()).unwrap();

        assert!(json.contains("server-1"));
        assert!(json.contains("/library/parts/10/file.flac"));
        assert!(!json.contains("X-Plex-Token"));
        assert!(!json.contains("\"uri\""));
    }

    #[test]
    fn authenticated_legacy_plex_cache_entry_is_discarded_but_local_is_preserved() {
        let legacy_plex = TrackMetadata {
            uri: "http://old-route/library/parts/1/file.flac?X-Plex-Token=OLD_SECRET"
                .to_string(),
            ..engine_with_track("unused").queue.remove(0)
        };
        let local = engine_with_track("álbum/faixa.flac").queue.remove(0);

        let (migrated, changed) =
            AudioEngine::migrate_loaded_queue(vec![legacy_plex, local.clone()]);

        assert!(changed);
        assert_eq!(migrated.len(), 1);
        assert_eq!(migrated[0].uri, local.uri);
    }

    #[test]
    fn legacy_queue_artwork_token_is_replaced_by_stable_reference() {
        let mut legacy = plex_track();
        legacy.thumb = Some(
            "https://old.invalid/library/metadata/42/thumb/1?X-Plex-Token=SECRET"
                .to_string(),
        );

        let (migrated, changed) = AudioEngine::migrate_loaded_queue(vec![legacy]);
        let json = serde_json::to_string(&migrated).unwrap();

        assert!(changed);
        assert_eq!(migrated.len(), 1);
        assert_eq!(migrated[0].thumb, None);
        assert_eq!(
            migrated[0].plex_image,
            Some(PlexImageRef {
                server_id: "server-1".to_string(),
                path: "/library/metadata/42/thumb/1".to_string(),
            })
        );
        assert!(!json.contains("X-Plex-Token"));
        assert!(!json.contains("SECRET"));
    }

    #[test]
    fn queue_restore_uses_fresh_uri_without_mutating_stable_plex_queue() {
        let engine = AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: vec![plex_track()],
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        };
        let first = engine
            .device_switch_queue_restore_commands(
                None,
                &["http://lan/library/parts/10/file.flac?X-Plex-Token=TOKEN_A".to_string()],
            )
            .unwrap();
        let refreshed = engine
            .device_switch_queue_restore_commands(
                None,
                &["https://remote/library/parts/10/file.flac?X-Plex-Token=TOKEN_B".to_string()],
            )
            .unwrap();

        assert!(first.contains("http://lan/"));
        assert!(refreshed.contains("https://remote/"));
        assert!(engine.queue[0].uri.is_empty());
        assert!(matches!(
            engine.queue[0].media_locator,
            Some(MediaLocator::Plex { .. })
        ));
    }

    #[test]
    fn mpd_errors_for_ephemeral_plex_uris_are_sanitized() {
        let secret_uri =
            "https://route.invalid/library/parts/1/file.flac?X-Plex-Token=SECRET".to_string();
        let error = AudioEngine::sanitize_mpd_error(
            format!("ACK [50@0] {{add}} Falha em {secret_uri}"),
            &[secret_uri],
        );

        assert!(error.contains("ACK [50@0] {add}"));
        assert!(!error.contains("SECRET"));
        assert!(!error.contains("route.invalid"));
        assert!(!error.contains("X-Plex-Token"));
    }

    fn restore_with_seek_responses(
        engine: &AudioEngine,
        snapshot: DeviceSwitchSnapshot,
        mut seek_responses: Vec<Result<(), String>>,
    ) -> (Result<(), String>, Vec<String>) {
        let mut commands = Vec::new();
        let mut send = |command: &str| {
            commands.push(command.to_string());
            if command.starts_with("seekcur") {
                if seek_responses.is_empty() {
                    Ok(Vec::new())
                } else {
                    seek_responses.remove(0).map(|_| Vec::new())
                }
            } else {
                Ok(Vec::new())
            }
        };
        let playback_uris: Vec<String> =
            engine.queue.iter().map(|track| track.uri.clone()).collect();
        let result = engine.restore_after_device_switch_with(
            Some(snapshot),
            &playback_uris,
            &mut send,
            &mut |_| {},
        );
        (result, commands)
    }

    #[test]
    fn restore_playing_batches_queue_and_play_then_seeks_and_pauses() {
        let engine = engine_with_two_tracks();
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 1,
                elapsed: 12.0,
                state: PlaybackState::Playing,
            },
            vec![],
        );

        result.unwrap();
        assert_eq!(commands.len(), 3);
        assert_eq!(
            commands[0],
            "command_list_begin\nclear\nadd \"álbum/faixa.flac\"\nadd \"álbum/segunda.flac\"\nplay 1\ncommand_list_end"
        );
        assert_eq!(commands[1], "seekcur 12.0");
        assert_eq!(commands[2], "pause 1");
        assert!(!commands[0].contains("seekcur"));
        assert!(!commands.iter().any(|command| command == "status"));
    }

    #[test]
    fn restore_paused_batches_queue_and_play_then_seeks_and_pauses() {
        let engine = engine_with_track("faixa.flac");
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 0,
                elapsed: 42.5,
                state: PlaybackState::Paused,
            },
            vec![Ok(())],
        );

        result.unwrap();
        assert_eq!(commands.len(), 3);
        assert!(commands[0].contains("\nplay 0\ncommand_list_end"));
        assert_eq!(commands[1], "seekcur 42.5");
        assert_eq!(commands[2], "pause 1");
        assert!(!commands.iter().any(|command| command == "status"));
    }

    #[test]
    fn device_switch_restore_never_copies_volume_to_the_new_output() {
        let engine = engine_with_track("faixa.flac");
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 0,
                elapsed: 0.0,
                state: PlaybackState::Paused,
            },
            vec![],
        );

        result.unwrap();
        assert!(!commands
            .iter()
            .any(|command| command.starts_with("setvol")));
    }

    #[test]
    fn restore_retries_temporary_not_seekable_then_succeeds() {
        let engine = engine_with_track("faixa.flac");
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 0,
                elapsed: 9.0,
                state: PlaybackState::Playing,
            },
            vec![
                Err("ACK [5@0] {seekcur} Not seekable".to_string()),
                Ok(()),
            ],
        );

        result.unwrap();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("seekcur"))
                .count(),
            2
        );
        assert_eq!(commands.last().map(String::as_str), Some("pause 1"));
    }

    #[test]
    fn restore_treats_persistent_not_seekable_as_partial_success_and_pauses() {
        let engine = engine_with_track("faixa.flac");
        let not_seekable = || Err("ACK [5@0] {seekcur} Not seekable".to_string());
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 0,
                elapsed: 30.0,
                state: PlaybackState::Playing,
            },
            vec![not_seekable(), not_seekable(), not_seekable()],
        );

        result.unwrap();
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("seekcur"))
                .count(),
            3
        );
        assert_eq!(commands.last().map(String::as_str), Some("pause 1"));
    }

    #[test]
    fn restore_stopped_rebuilds_only_queue_without_selecting_a_current_song() {
        let engine = engine_with_two_tracks();
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 1,
                elapsed: 12.0,
                state: PlaybackState::Stopped,
            },
            vec![],
        );

        result.unwrap();
        assert_eq!(commands.len(), 1);
        assert!(commands[0].contains("add \"álbum/segunda.flac\""));
        assert!(!commands.iter().any(|command| command.starts_with("play")));
        assert!(!commands.iter().any(|command| command.starts_with("seek")));
        assert!(!commands.iter().any(|command| command.starts_with("pause")));
        assert!(!commands.iter().any(|command| command == "status"));
        assert!(!commands[0].contains("\nplay "));
    }

    #[test]
    fn restore_propagates_other_seek_ack_errors() {
        let engine = engine_with_track("faixa.flac");
        let (result, commands) = restore_with_seek_responses(
            &engine,
            DeviceSwitchSnapshot {
                queue_index: 0,
                elapsed: 12.0,
                state: PlaybackState::Playing,
            },
            vec![Err("ACK [50@0] {seekcur} No such song".to_string())],
        );

        assert_eq!(result.unwrap_err(), "ACK [50@0] {seekcur} No such song");
        assert_eq!(commands.len(), 2);
    }

    #[test]
    fn queue_restore_commands_escape_mpd_arguments_and_reject_line_breaks() {
        let engine = engine_with_track("pasta/uma \\\"faixa\\\".flac");
        let commands = engine
            .device_switch_queue_restore_commands(
                None,
                &["pasta/uma \\\"faixa\\\".flac".to_string()],
            )
            .unwrap();
        assert!(commands.contains(r#"add "pasta/uma \\\"faixa\\\".flac""#));

        let plex_engine =
            engine_with_track("http://plex.local/library/parts/1/file.flac?download=1");
        let plex_commands = plex_engine
            .device_switch_queue_restore_commands(
                None,
                &["http://plex.local/library/parts/1/file.flac?download=1".to_string()],
            )
            .unwrap();
        assert!(plex_commands
            .contains(r#"add "http://plex.local/library/parts/1/file.flac?download=1""#));

        let engine = engine_with_track("pasta/faixa.flac\nkill");
        assert!(engine
            .device_switch_queue_restore_commands(None, &["pasta/faixa.flac\nkill".to_string()])
            .is_err());
    }

    #[test]
    fn textual_mpd_arguments_use_one_quoting_strategy() {
        assert_eq!(
            AudioEngine::quote_mpd_argument(" pasta/Único \\\"mix\\\".flac ").unwrap(),
            r#"" pasta/Único \\\"mix\\\".flac ""#
        );
        assert!(AudioEngine::quote_mpd_argument("faixa\nkill").is_err());
        assert!(AudioEngine::quote_mpd_argument("faixa\rkill").is_err());
        assert!(AudioEngine::quote_mpd_argument("faixa\0kill").is_err());

        let engine = engine_with_track("faixa.flac");
        assert!(engine.list_directory("pasta\nkill").is_err());

        let mut engine = engine_with_track("anterior.flac");
        let invalid_track = TrackMetadata {
            title: "Inválida".to_string(),
            artist: String::new(),
            album: String::new(),
            thumb: Some(String::new()),
            plex_image: None,
            media_locator: None,
            uri: "faixa.flac\nkill".to_string(),
            duration: None,
        };
        assert!(engine
            .play_tracks(
                vec![invalid_track],
                vec!["faixa.flac\nkill".to_string()],
                0,
            )
            .is_err());
        assert_eq!(engine.queue[0].uri, "anterior.flac");
    }

    #[test]
    fn empty_queue_requires_no_mpd_restore_command() {
        let engine = AudioEngine {
            socket_path: "/socket/que/não/existe".to_string(),
            music_dir: String::new(),
            queue: Vec::new(),
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
        };
        assert!(engine.restore_after_device_switch(None, &[]).is_ok());
    }

    #[test]
    fn playback_state_parser_rejects_unknown_mpd_state() {
        assert_eq!(
            AudioEngine::parse_playback_state("stop").unwrap(),
            PlaybackState::Stopped
        );
        assert!(AudioEngine::parse_playback_state("unknown").is_err());
    }

    #[test]
    fn mpd_response_parser_distinguishes_ok_ack_and_early_eof() {
        let mut ok = std::io::Cursor::new(b"state: play\nOK\n");
        assert_eq!(
            AudioEngine::read_mpd_response(&mut ok).unwrap(),
            vec!["state: play"]
        );

        let mut ack = std::io::Cursor::new(b"ACK [50@0] {play} No such song\n");
        assert!(AudioEngine::read_mpd_response(&mut ack)
            .unwrap_err()
            .starts_with("ACK"));

        let mut eof = std::io::Cursor::new(b"state: play\n");
        assert!(AudioEngine::read_mpd_response(&mut eof)
            .unwrap_err()
            .contains("antes da resposta final"));

        let mut partial = std::io::Cursor::new(b"state: play");
        assert!(AudioEngine::read_mpd_response(&mut partial)
            .unwrap_err()
            .contains("antes da resposta final"));
    }
}
