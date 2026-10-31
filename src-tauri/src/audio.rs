use crate::local_media;
use crate::plex::{contains_plex_token, legacy_plex_image_ref, PlexImageRef};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    DirectEquipment,
    AlsaHardware,
    #[serde(rename = "pipewire")]
    PipeWire,
    MpdSoftware,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetedLibraryUpdateErrorCode {
    InvalidUri,
    MpdUnavailable,
    UpdateCommandFailed,
    InvalidResponse,
    RefreshTaskFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum MpdJobIdParseError {
    MissingJob,
    DuplicateJob,
    InvalidJob,
    ContradictoryResponse,
    Ack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum MpdRefreshCompletion {
    Completed,
    Timeout,
    TransportError(String),
    ProtocolError(String),
    Unavailable(String),
}

#[allow(dead_code)]
enum MpdStatusQueryError {
    Transport(String),
    Protocol(String),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct CatalogAuditResult {
    pub(crate) total_files: usize,
    pub(crate) invalid_count: usize,
    pub(crate) invalid_uris: Vec<String>,
}

#[allow(dead_code)]
impl CatalogAuditResult {
    pub(crate) fn is_clean(&self) -> bool {
        self.invalid_count == 0
    }
}

#[allow(dead_code)]
const MAX_CATALOG_AUDIT_SAMPLES: usize = 8;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct VolumeStatus {
    pub value: u32,
    pub muted: bool,
    pub writable: bool,
    pub available: bool,
    pub backend: VolumeBackend,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub direct_gain: Option<crate::direct_volume::Gain>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub direct_token: Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub shared_token: Option<String>,
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
            direct_gain: None,
            direct_token: None,
            shared_token: None,
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
            direct_gain: None,
            direct_token: None,
            shared_token: None,
        }
    }

    pub(crate) fn pipewire_unavailable() -> Self {
        Self {
            value: 0,
            muted: false,
            writable: false,
            available: false,
            backend: VolumeBackend::Unavailable,
            direct_gain: None,
            direct_token: None,
            shared_token: None,
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

/// Presence of raw values in accepted MPD index rows, not a physical/semantic inspection.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct LocalAlbumMissingTags {
    pub album: usize,
    pub artist: usize,
    pub album_artist: usize,
    pub date: usize,
    pub genre: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct LocalAlbumTagPresence {
    pub evaluated_track_count: usize,
    pub missing: LocalAlbumMissingTags,
}

impl LocalAlbumTagPresence {
    fn observe(&mut self, present: [bool; 5]) {
        self.evaluated_track_count += 1;
        self.missing.album += usize::from(!present[0]);
        self.missing.artist += usize::from(!present[1]);
        self.missing.album_artist += usize::from(!present[2]);
        self.missing.date += usize::from(!present[3]);
        self.missing.genre += usize::from(!present[4]);
    }

    fn merge(&mut self, other: &Self) {
        self.evaluated_track_count += other.evaluated_track_count;
        self.missing.album += other.missing.album;
        self.missing.artist += other.missing.artist;
        self.missing.album_artist += other.missing.album_artist;
        self.missing.date += other.missing.date;
        self.missing.genre += other.missing.genre;
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbum {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub folder_path: String,
    pub source_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
    pub track_count: usize,
    pub discs: Vec<LocalAlbumDisc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_presence: Option<LocalAlbumTagPresence>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbumDisc {
    pub number: u32,
    pub label: String,
    pub folder_path: String,
    pub track_count: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalAlbumDiscTracks {
    pub disc: LocalAlbumDisc,
    pub tracks: Vec<LocalItem>,
}

/// Only allowlisted codes cross IPC; MPD error text may contain credentials or paths.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PlaybackDiagnostic {
    OutputBusy,
    OutputUnavailable,
    FormatUnsupported,
    PlaybackError,
    QueueUncertain,
    PersistenceFailed,
}

impl PlaybackDiagnostic {
    fn from_mpd_error(error: &str) -> Self {
        // Match ALSA output context AND a known terminal error, not arbitrary
        // "busy" / "not found" text which could describe a media source.
        if error.contains("Sonante Direct strict:") { return Self::FormatUnsupported; }
        let alsa_open = error.contains("Failed to open ALSA device ");
        if alsa_open && error.ends_with(": Device or resource busy") {
            Self::OutputBusy
        } else if (alsa_open
            && [
                ": No such device",
                ": No such device or address",
                ": No such file or directory",
            ]
            .iter()
            .any(|suffix| error.ends_with(suffix)))
            || error == "All audio outputs are disabled"
        {
            Self::OutputUnavailable
        } else {
            Self::PlaybackError
        }
    }
}

fn snapshot_consistent_default() -> bool { true }

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PlaybackStatus {
    /// False only when this query observed contradictory MPD identities.
    #[serde(default = "snapshot_consistent_default")]
    pub snapshot_consistent: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<crate::signal_processing::ProcessingStatus>,
    #[serde(default)]
    pub options: Option<crate::playback_options::PlaybackOptions>,
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
    #[serde(default)]
    pub playback_error: Option<PlaybackDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MpdProbeFailure {
    SocketUnavailable,
    ProtocolUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceSwitchRestoreOutcome { Complete, PositionNotRestored }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Stopped,
    Paused,
    Playing,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct PlaybackResumeSnapshot {
    pub(crate) queue_index: usize,
    pub(crate) elapsed: f64,
    pub(crate) state: PlaybackState,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplicitPlaybackRequest {
    Play,
    Pause,
}

fn explicit_playback_command(
    state: PlaybackState,
    request: ExplicitPlaybackRequest,
) -> Option<&'static str> {
    match (state, request) {
        (PlaybackState::Playing, ExplicitPlaybackRequest::Play)
        | (PlaybackState::Paused | PlaybackState::Stopped, ExplicitPlaybackRequest::Pause) => None,
        (PlaybackState::Paused, ExplicitPlaybackRequest::Play) => Some("pause 0"),
        (PlaybackState::Stopped, ExplicitPlaybackRequest::Play) => Some("play"),
        (PlaybackState::Playing, ExplicitPlaybackRequest::Pause) => Some("pause 1"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PlaybackProgressStatus {
    state: PlaybackState,
    elapsed: f64,
}

const SHARED_PROGRESS_SAMPLE_DELAY: Duration = Duration::from_millis(80);
const SHARED_MIN_PROGRESS: f64 = 0.02;

// Seek-only budget: 4 samples over 960 ms; after the existing single recovery,
// 2 more tail samples (880 ms including recovery's 80 ms). At most 11 status
// queries + seekcur + pause 1/0. Reads/writes/sleeps use a 3 s deadline under
// AudioState; mutex acquisition and the existing Unix connect are not timed here.
const SHARED_SEEK_BUDGET: Duration = Duration::from_secs(3);
const SEEK_TAIL_DELAY: Duration = Duration::from_millis(400);

struct MpdDeadlineReader {
    stream: UnixStream,
    deadline: Option<Instant>,
}

fn mpd_deadline_timeout(deadline: Option<Instant>) -> Result<Duration, String> {
    let remaining = deadline
        .map(|end| end.saturating_duration_since(Instant::now()))
        .unwrap_or(Duration::from_millis(500));
    if remaining.is_zero() {
        return Err("Prazo de confirmação do seek Shared esgotado.".into());
    }
    Ok(remaining.min(Duration::from_millis(500)))
}

impl Read for MpdDeadlineReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.deadline.is_some() {
            let timeout = mpd_deadline_timeout(self.deadline)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::TimedOut, error))?;
            self.stream.set_read_timeout(Some(timeout))?;
        }
        self.stream.read(buffer)
    }
}

#[derive(Clone)]
pub(crate) struct EqTransitionWatch {epoch:Arc<AtomicU64>,expected:u64}
impl EqTransitionWatch {pub(crate) fn current(&self)->bool{self.epoch.load(Ordering::Acquire)==self.expected}}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceSwitchSnapshot {
    pub queue_index: usize,
    pub elapsed: f64,
    pub state: PlaybackState,
}

#[derive(Clone, PartialEq)]
#[allow(dead_code)]
pub(crate) struct MpdPlaybackSnapshot {
    pub(crate) uris: Vec<String>,
    pub(crate) current_index: Option<usize>,
    pub(crate) elapsed: f64,
    pub(crate) state: PlaybackState,
    pub(crate) playlist_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MpdQueueStatus {
    playlist_version: u64,
    playlist_length: usize,
    current_index: Option<usize>,
    elapsed: f64,
    state: PlaybackState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MpdQueueEntry {
    pub(crate) uri: String,
    pub(crate) pos: usize,
    pub(crate) id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MpdQueueStructure {
    pub(crate) entries: Vec<MpdQueueEntry>,
    pub(crate) playlist_version: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MpdQueueStructureError {
    Invalid,
    Changed,
    QueryFailed,
}

impl std::fmt::Display for MpdQueueStructureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "LOCAL_SOURCE_REMOVAL_QUEUE_INVALID",
            Self::Changed => "LOCAL_SOURCE_REMOVAL_QUEUE_CHANGED",
            Self::QueryFailed => "LOCAL_SOURCE_REMOVAL_QUEUE_QUERY_FAILED",
        })
    }
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

pub(crate) const LOCAL_FOLDER_COVER_NAMES: &[&str] = &[
    "cover.jpg", "cover.jpeg", "cover.png", "cover.webp",
    "Folder.jpg", "folder.jpg", "folder.jpeg", "folder.png", "folder.webp",
    "front.jpg", "Front.jpg", "front.png", "Front.png", "front.webp", "Front.webp",
    "albumart.jpg", "AlbumArtSmall.jpg",
];

pub fn find_folder_cover_path(dir: &Path) -> Option<String> {
    for name in LOCAL_FOLDER_COVER_NAMES {
        let p = dir.join(name);
        if let Ok(metadata) = std::fs::metadata(&p) {
            if !metadata.is_file() || metadata.len() > MAX_LOCAL_COVER_BYTES as u64 {
                continue;
            }
            if let Ok(file) = std::fs::File::open(&p) {
                if let Some(bytes) = read_limited_local_cover(file, metadata.len()) {
                    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("jpeg").to_lowercase();
                    let mime = match ext.as_str() {
                        "png" => "image/png",
                        "webp" => "image/webp",
                        _ => "image/jpeg",
                    };
                    let b64 = BASE64.encode(&bytes);
                    return Some(format!("data:{};base64,{}", mime, b64));
                }
            }
        }
    }
    None
}

fn read_limited_local_cover(reader: impl Read, size_hint: u64) -> Option<Vec<u8>> {
    if size_hint > MAX_LOCAL_COVER_BYTES as u64 {
        return None;
    }
    // Metadata is only an early rejection: the file may grow before or during reading.
    let mut bytes = Vec::with_capacity(size_hint as usize + 1);
    reader
        .take(MAX_LOCAL_COVER_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_LOCAL_COVER_BYTES).then_some(bytes)
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
    queue_structure_reserved: Arc<AtomicU8>,
    queue_coherent: bool,
    cache_persistence_pending: bool,
    playback_intent: u64,
    eq_intervention:Arc<AtomicU64>,
    direct_playback:Option<Arc<crate::direct_playback::Gate>>,
}

const QUEUE_STRUCTURE_RESERVED: u8 = 1;
const LOGICAL_RESUME_RESERVED: u8 = 2;

/// Excludes structural commands without retaining AudioState during MPD jobs.
/// An ordinary reservation releases on drop. Destructive owners must prove
/// completion/recovery before releasing; uncertainty keeps the fence active.
pub(crate) struct QueueStructureReservation(Arc<AtomicU8>, bool, Arc<()>);

impl Drop for QueueStructureReservation {
    fn drop(&mut self) {
        if self.1 {
            self.0.store(0, Ordering::SeqCst);
        }
    }
}

impl QueueStructureReservation {
    pub(crate) fn arm_destructive(&mut self) {
        self.1 = false;
    }
    pub(crate) fn release_after_confirmed_completion(&mut self) {
        self.1 = true;
    }
}

/// Owns the structural fence while playback references are resolved off-lock.
pub(crate) struct DeviceSwitchQueueSnapshot {
    owner: QueueStructureReservation,
    tracks: Vec<TrackMetadata>,
}

pub(crate) struct ResolvedDeviceSwitchQueue {
    snapshot: DeviceSwitchQueueSnapshot,
    playback_uris: Vec<String>,
}

impl DeviceSwitchQueueSnapshot {
    pub(crate) fn tracks(&self) -> &[TrackMetadata] {
        &self.tracks
    }

    pub(crate) fn bind_resolved_uris(
        self,
        playback_uris: Vec<String>,
    ) -> Result<ResolvedDeviceSwitchQueue, String> {
        if playback_uris.len() != self.tracks.len() {
            return Err("A fila mudou durante a preparação da troca de saída.".into());
        }
        for uri in &playback_uris {
            if uri.trim().is_empty() {
                return Err("Faixa sem referência de mídia válida.".into());
            }
            AudioEngine::quote_mpd_argument(uri)?;
        }
        Ok(ResolvedDeviceSwitchQueue {
            snapshot: self,
            playback_uris,
        })
    }
}

/// Captured under AudioState. The cursor fence is opt-in, so ordinary structural
/// reservations for proven unaffected queues still allow player commands.
#[allow(dead_code)] // Prepared for a later transactional admission change.
pub(crate) struct ReservedLogicalQueueSnapshot {
    pub(crate) tracks: Vec<TrackMetadata>,
    pub(crate) resume: PlaybackResumeSnapshot,
    identity: Arc<()>,
}

/// Issued only after read-only MPD recovery verification, bound to one owner.
#[allow(dead_code)]
pub(crate) struct VerifiedLogicalQueueRestore {
    tracks: Vec<TrackMetadata>,
    resume: PlaybackResumeSnapshot,
    cache: Vec<u8>,
    identity: Arc<()>,
}

/// Short-lived I/O capability issued under AudioState to its reservation owner.
/// Player mutations are fenced; status/queue queries retain access during I/O.
pub(crate) struct ReservedQueueEffect<'reservation> {
    socket_path: String,
    owner: Arc<AtomicU8>,
    identity: Arc<()>,
    _owner_scope: std::marker::PhantomData<&'reservation QueueStructureReservation>,
}

impl ReservedQueueEffect<'_> {
    fn ensure_active(&self) -> Result<(), String> {
        if self.owner.load(Ordering::SeqCst) == 0 {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_OWNER_INVALID".into());
        }
        Ok(())
    }

    /// Recovery itself is the owner's responsibility. Verify a neutral stopped
    /// MPD and restored occurrences; never start audio or position the PCM here.
    #[allow(dead_code)]
    pub(crate) fn confirm_reserved_logical_queue_restore(
        &self,
        captured: &ReservedLogicalQueueSnapshot,
        expected: &MpdPlaybackSnapshot,
    ) -> Result<VerifiedLogicalQueueRestore, String> {
        self.confirm_reserved_logical_queue_restore_with(
            captured,
            expected,
            AudioEngine::encode_queue_cache_with_resume,
        )
    }

    fn confirm_reserved_logical_queue_restore_with(
        &self,
        captured: &ReservedLogicalQueueSnapshot,
        expected: &MpdPlaybackSnapshot,
        encode: impl FnOnce(
            &[TrackMetadata],
            Option<&PlaybackResumeSnapshot>,
        ) -> Result<Vec<u8>, String>,
    ) -> Result<VerifiedLogicalQueueRestore, String> {
        self.ensure_active()?;
        let failure = "LOCAL_SOURCE_REMOVAL_QUEUE_RECOVERY_UNCONFIRMED";
        if !Arc::ptr_eq(&self.identity, &captured.identity)
            || self.owner.load(Ordering::SeqCst) & LOGICAL_RESUME_RESERVED == 0
            || !captured.resume.valid_for(captured.tracks.len())
            || expected.state != PlaybackState::Stopped
            || expected.current_index.is_some()
            || expected.elapsed != 0.0
            || expected.uris.len() != captured.tracks.len()
        {
            return Err(failure.into());
        }
        let queue =
            AudioEngine::capture_mpd_queue_structure(&self.socket_path).map_err(|_| failure)?;
        let actual =
            AudioEngine::capture_mpd_playback_snapshot(&self.socket_path).map_err(|_| failure)?;
        let status = AudioEngine::send_command_to_socket(&self.socket_path, "status")
            .map_err(|_| failure)?;
        let final_status = AudioEngine::parse_mpd_queue_status(&status).map_err(|_| failure)?;
        if actual.uris != expected.uris
            || actual.state != PlaybackState::Stopped
            || actual.current_index.is_some()
            || actual.elapsed != 0.0
            || final_status.state != PlaybackState::Stopped
            || final_status.current_index.is_some()
            || final_status.elapsed != 0.0
            || final_status.playlist_version != actual.playlist_version
            || final_status.playlist_length != actual.uris.len()
            || status.iter().any(|line| line.starts_with("error: "))
        {
            return Err(failure.into());
        }
        // Reuse the durable contract's occurrence/media validation. Plex URIs
        // remain ephemeral and the original stable identity is retained.
        crate::managed_save_intent::QueueBackup::new_with_logical_resume(
            &captured.tracks,
            &expected.uris,
            &queue,
            &actual,
            crate::managed_save_intent::BackupLogicalResume {
                queue_index: captured.resume.queue_index,
                elapsed: captured.resume.elapsed,
                state: captured.resume.state,
            },
        )
        .map_err(|_| failure)?;
        let mut tracks = captured.tracks.clone();
        for (track, uri) in tracks.iter_mut().zip(&expected.uris) {
            track.uri = uri.clone();
            track.thumb = None;
            track.plex_image = None;
        }
        let cache = encode(&tracks, Some(&captured.resume)).map_err(|_| failure)?;
        self.ensure_active()?;
        Ok(VerifiedLogicalQueueRestore {
            tracks,
            resume: captured.resume,
            cache,
            identity: Arc::clone(&self.identity),
        })
    }

    /// Only the owner can clear. Cache I/O is performed by the durable intention
    /// after releasing AudioState. Neither ACK nor a lost response is a proof.
    pub(crate) fn clear_reserved_queue(
        &self,
        expected: &MpdQueueStructure,
    ) -> Result<(MpdQueueStructure, Vec<u8>), String> {
        self.ensure_active()?;
        if AudioEngine::capture_mpd_queue_structure(&self.socket_path)
            .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED")?
            != *expected
        {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CONFIRMATION_STALE".into());
        }
        AudioEngine::send_command_to_socket(&self.socket_path, "clear")
            .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED")?;
        let queue = AudioEngine::capture_mpd_queue_structure(&self.socket_path)
            .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED")?;
        let playback = AudioEngine::capture_mpd_playback_snapshot(&self.socket_path)
            .map_err(|_| "LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED")?;
        if !queue.entries.is_empty()
            || !playback.uris.is_empty()
            || queue.playlist_version != playback.playlist_version
            || playback.state != PlaybackState::Stopped
            || playback.current_index.is_some()
            || playback.elapsed != 0.0
        {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_CLEAR_UNCONFIRMED".into());
        }
        Ok((
            queue,
            AudioEngine::encode_queue_cache_with_resume(&[], None)?,
        ))
    }

    /// The existing recovery restores active snapshots paused first. Verify
    /// that seek, then resume only an originally Playing snapshot and verify it.
    pub(crate) fn confirm_reserved_queue_restore(
        &self,
        tracks: Vec<TrackMetadata>,
        snapshot: &MpdPlaybackSnapshot,
        is_shared: bool,
    ) -> Result<(Vec<TrackMetadata>, Vec<u8>), String> {
        self.ensure_active()?;
        let failure = "LOCAL_SOURCE_REMOVAL_QUEUE_RECOVERY_UNCONFIRMED";
        if tracks.len() != snapshot.uris.len()
            || tracks.iter().zip(&snapshot.uris).any(|(track, uri)| track.uri != *uri)
            // MPD's stopped restore cannot select a cursor without starting audio.
            // Never claim that such a snapshot has been recovered.
            || (snapshot.state == PlaybackState::Stopped
                && (snapshot.current_index.is_some() || snapshot.elapsed != 0.0))
        {
            return Err(failure.into());
        }
        let queue =
            AudioEngine::capture_mpd_queue_structure(&self.socket_path).map_err(|_| failure)?;
        let mut actual =
            AudioEngine::capture_mpd_playback_snapshot(&self.socket_path).map_err(|_| failure)?;
        let paused = snapshot.state != PlaybackState::Stopped;
        if queue
            .entries
            .iter()
            .map(|entry| &entry.uri)
            .ne(snapshot.uris.iter())
            || actual.uris != snapshot.uris
            || actual.playlist_version != queue.playlist_version
            || actual.state
                != if paused {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Stopped
                }
            || actual.current_index != snapshot.current_index
            || (actual.elapsed - snapshot.elapsed).abs() > 0.01
            || AudioEngine::send_command_to_socket(&self.socket_path, "status")
                .map_err(|_| failure)?
                .iter()
                .any(|line| line.starts_with("error: "))
        {
            return Err(failure.into());
        }
        if snapshot.state == PlaybackState::Playing {
            AudioEngine::send_command_to_socket(&self.socket_path, "pause 0")
                .map_err(|_| failure)?;
            AudioEngine::verify_shared_progress_at(&self.socket_path, is_shared)
                .map_err(|_| failure)?;
            actual = AudioEngine::capture_mpd_playback_snapshot(&self.socket_path)
                .map_err(|_| failure)?;
            if actual.uris != snapshot.uris
                || actual.state != PlaybackState::Playing
                || actual.current_index != snapshot.current_index
                || actual.elapsed < snapshot.elapsed
                || AudioEngine::send_command_to_socket(&self.socket_path, "status")
                    .map_err(|_| failure)?
                    .iter()
                    .any(|line| line.starts_with("error: "))
            {
                return Err(failure.into());
            }
        }
        let resume = if paused {
            Some(PlaybackResumeSnapshot {
                queue_index: snapshot.current_index.ok_or(failure)?,
                elapsed: actual.elapsed,
                state: actual.state,
            })
        } else {
            None
        };
        let bytes = AudioEngine::encode_queue_cache_with_resume(&tracks, resume.as_ref())?;
        Ok((tracks, bytes))
    }
}

const LOCAL_ALBUM_PAGE_SIZE: usize = 500;

#[derive(Clone, Default)]
struct AlbumCollector {
    title: String,
    year: Option<String>,
    added_at: Option<String>,
    genres: Vec<String>,
    track_count: usize,
    tag_presence: LocalAlbumTagPresence,
    folder_path: String,
    track_uris: Vec<String>,
    tracks: Vec<(String, LocalItem)>,
    album_artist: Option<(String, String)>,
    album_artist_consistent: bool,
    artists: HashMap<String, (String, usize)>,
    discs: Vec<LocalAlbumDisc>,
}

// The catalog needs the same canonical metadata as track reads, without
// retaining every URI and LocalItem. Full remains the default for those reads.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LocalAlbumCollectionMode {
    Summary,
    #[default]
    Full,
}

#[derive(Clone, Default)]
struct LocalAlbumCollection {
    mode: LocalAlbumCollectionMode,
    physical: HashMap<String, AlbumCollector>,
    cue: HashMap<String, AlbumCollector>,
}

struct VirtualCueTrackPath {
    cue_directory: String,
    physical_folder: String,
}

// MPD emits Added in UTC at second precision. Unknown/invalid optional
// metadata must not prevent the rest of the catalog from being displayed.
fn parse_local_album_added_at(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    if bytes.len() != 20 || !bytes.iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        10 => *b == b'T',
        13 | 16 => *b == b':',
        19 => *b == b'Z',
        _ => b.is_ascii_digit(),
    }) { return None; }
    let year = value[..4].parse::<u32>().ok()?;
    let month = value[5..7].parse::<usize>().ok()?;
    let day = value[8..10].parse::<u32>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if year < 1970 || !(1..=12).contains(&month) || day == 0 || day > days[month - 1]
        || value[11..13].parse::<u32>().ok()? > 23
        || value[14..16].parse::<u32>().ok()? > 59
        || value[17..19].parse::<u32>().ok()? > 59 { return None; }
    Some(value.to_owned())
}

impl AlbumCollector {
    // The album entered the catalog with its first known track. Adding another
    // track/disc or editing tags must not make an existing album look new.
    fn observe_added_at(&mut self, added: Option<String>) {
        if let Some(added) = added {
            if self.added_at.as_ref().is_none_or(|previous| added < *previous) {
                self.added_at = Some(added);
            }
        }
    }

    fn into_disc_tracks(self) -> Vec<LocalAlbumDiscTracks> {
        let tracks = self.tracks;
        self.discs
            .into_iter()
            .map(|disc| LocalAlbumDiscTracks {
                tracks: tracks
                    .iter()
                    .filter(|(disc_path, _)| disc_path == &disc.folder_path)
                    .map(|(_, track)| track.clone())
                    .collect(),
                disc,
            })
            .collect()
    }

    fn merge_title_variant(&mut self, other: Self) {
        for genre in &other.genres {
            crate::catalog_genres::add_genre(&mut self.genres, genre);
        }
        self.observe_added_at(other.added_at);
        self.track_count += other.track_count;
        self.tag_presence.merge(&other.tag_presence);
        self.track_uris.extend(other.track_uris);
        self.tracks.extend(other.tracks);
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
        for genre in &other.genres {
            crate::catalog_genres::add_genre(&mut self.genres, genre);
        }
        self.observe_added_at(other.added_at);
        self.track_count += other.track_count;
        self.tag_presence.merge(&other.tag_presence);
        self.track_uris.extend(other.track_uris);
        self.tracks.extend(other.tracks);
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
        if !local_media::is_virtual_cue_track_uri(file) {
            return None;
        }
        let path = Path::new(file);
        let cue_directory = path.parent()?;
        let physical_folder = cue_directory.parent()?;

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

    fn local_album_scope(album_id: &str) -> Result<&str, String> {
        let (length, remainder) = album_id
            .split_once(':')
            .ok_or_else(|| "ID de álbum local inválido.".to_string())?;
        let folder_length = length
            .parse::<usize>()
            .map_err(|_| "ID de álbum local inválido.".to_string())?;
        let folder = remainder
            .get(..folder_length)
            .ok_or_else(|| "ID de álbum local inválido.".to_string())?;
        let title = remainder
            .get(folder_length..)
            .and_then(|suffix| suffix.strip_prefix(':'))
            .filter(|title| !title.is_empty())
            .ok_or_else(|| "ID de álbum local inválido.".to_string())?;
        if Self::local_album_id(folder, title) != album_id
            || !local_media::is_safe_relative_mpd_uri(folder)
        {
            return Err("ID de álbum local inválido.".to_string());
        }
        Ok(folder)
    }

    pub(crate) fn local_album_base_filter(folder: &str) -> Result<String, String> {
        if !local_media::is_safe_relative_mpd_uri(folder) {
            return Err("Diretório de álbum local inválido.".to_string());
        }
        let escaped = folder.replace('\\', "\\\\").replace('\'', "\\'");
        Self::quote_mpd_argument(&format!("(base '{escaped}')"))
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
        if !self.queue_coherent { return None; }
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
        let collection = Self::get_local_album_collection_scoped_at(
            socket_path, "\"(base '')\"", None, false, LocalAlbumCollectionMode::Summary,
        )?;
        Ok(Self::finalize_local_album_collection(
            collection,
            valid_source_ids,
        ))
    }

    fn get_local_album_collection_at(socket_path: &str) -> Result<LocalAlbumCollection, String> {
        Self::get_local_album_collection_for_filter_at(socket_path, "\"(base '')\"")
    }

    fn get_local_album_collection_for_filter_at(
        socket_path: &str,
        filter: &str,
    ) -> Result<LocalAlbumCollection, String> {
        Self::get_local_album_collection_limited_at(socket_path, filter, None)
    }

    fn get_local_album_collection_limited_at(socket_path: &str, filter: &str, limit: Option<usize>) -> Result<LocalAlbumCollection, String> {
        Self::get_local_album_collection_scoped_at(socket_path, filter, limit, false, LocalAlbumCollectionMode::Full)
    }
    fn get_local_album_collection_scoped_at(socket_path: &str, filter: &str, limit: Option<usize>, confirmed_addition: bool, mode: LocalAlbumCollectionMode) -> Result<LocalAlbumCollection, String> {
        let status = Self::send_command_to_socket(socket_path, "status")
            .map_err(|e| format!("Falha ao verificar atualização da biblioteca: {e}"))?;
        if !confirmed_addition && status.iter().any(|line| line.starts_with("updating_db: ")) {
            return Err("Biblioteca local em atualização no MPD; tente novamente após a atualização.".to_string());
        }
        let count = Self::send_command_to_socket(socket_path, &format!("count {filter}"))
            .map_err(|e| format!("Falha ao contar músicas da biblioteca local: {e}"))?;
        let total = Self::parse_local_album_count(&count)?;
        if limit.is_some_and(|limit| total > limit) { return Err("SOURCE_ALBUM_LIMIT".into()); }
        let mut collection = LocalAlbumCollection { mode, ..LocalAlbumCollection::default() };
        for (start, end) in Self::local_album_windows(total) {
            let command = format!("find {filter} window {start}:{end}");
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
        Ok(collection)
    }

    pub(crate) fn get_local_album_track_uris_at(
        socket_path: &str,
        album_id: &str,
    ) -> Result<Option<Vec<String>>, String> {
        let collection = Self::get_local_album_collection_at(socket_path)?;
        Ok(Self::canonical_local_album_map(collection)
            .remove(album_id)
            .filter(|collector| collector.track_count > 0 && !collector.folder_path.is_empty())
            .map(|collector| collector.track_uris))
    }

    // Optional cover badges: reject an oversized scope BEFORE find/pages/track allocation.
    pub(crate) fn get_source_album_tracks_at(socket_path: &str, album_id: &str) -> Result<Option<Vec<LocalAlbumDiscTracks>>, String> {
        let scope = Self::local_album_scope(album_id)?;
        let filter = Self::local_album_base_filter(scope)?;
        let collection = Self::get_local_album_collection_limited_at(socket_path, &filter, Some(32))?;
        Ok(Self::canonical_local_album_map(collection).remove(album_id).map(AlbumCollector::into_disc_tracks))
    }

    // The caller holds an addition-only peer lease and validates every returned
    // URI against its immutable pre-addition catalog before exposing this read.
    pub(crate) fn get_confirmed_album_tracks_at(socket: &str, album_id: &str) -> Result<Option<Vec<LocalAlbumDiscTracks>>, String> {
        let filter = Self::local_album_base_filter(Self::local_album_scope(album_id)?)?;
        let collection = Self::get_local_album_collection_scoped_at(socket, &filter, None, true, LocalAlbumCollectionMode::Full)?;
        Ok(Self::canonical_local_album_map(collection).remove(album_id)
            .filter(|collector| collector.track_count > 0 && !collector.folder_path.is_empty())
            .map(AlbumCollector::into_disc_tracks))
    }

    pub(crate) fn get_checked_album_tracks_at(socket: &str, album_id: &str, confirmed: &HashSet<String>) -> Result<Option<Vec<LocalAlbumDiscTracks>>, String> {
        let filter = Self::local_album_base_filter(Self::local_album_scope(album_id)?)?;
        let lines = Self::send_command_to_socket(socket, &format!("find {filter}"))?;
        // Exclude new/unconfirmed records BEFORE grouping titles/discs, so a
        // just-indexed row cannot alter the identity of an existing album.
        let mut accepted = false;
        let mut old_lines = Vec::new();
        for line in lines {
            if line.starts_with("file:") {
                let uri = line.strip_prefix("file: ").ok_or("LOCAL_CHECK_TRACK_RESPONSE_INVALID")?;
                accepted = confirmed.contains(uri);
            }
            if accepted { old_lines.push(line); }
        }
        let mut collection = LocalAlbumCollection::default();
        Self::collect_local_album_page(&mut collection, old_lines)?;
        let Some(collector) = Self::canonical_local_album_map(collection).remove(album_id) else { return Ok(None); };
        let mut groups = collector.into_disc_tracks();
        for group in &mut groups { group.tracks.retain(|track| confirmed.contains(&track.path)); }
        groups.retain(|group| !group.tracks.is_empty());
        Ok((!groups.is_empty()).then_some(groups))
    }

    pub(crate) fn get_local_album_tracks_at(
        socket_path: &str,
        album_id: &str,
    ) -> Result<Option<Vec<LocalAlbumDiscTracks>>, String> {
        let scope = Self::local_album_scope(album_id)?;
        let filter = Self::local_album_base_filter(scope)?;
        let collection = Self::get_local_album_collection_for_filter_at(socket_path, &filter)?;
        Ok(Self::canonical_local_album_map(collection)
            .remove(album_id)
            .filter(|collector| collector.track_count > 0 && !collector.folder_path.is_empty())
            .map(AlbumCollector::into_disc_tracks))
    }

    fn finalize_local_album_collection(
        collection: LocalAlbumCollection,
        valid_source_ids: &HashSet<String>,
    ) -> Vec<LocalAlbum> {
        let map = Self::canonical_local_album_map(collection);

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
                    added_at: col.added_at,
                    genres: col.genres,
                    folder_path: col.folder_path,
                    source_id,
                    source_label: None,
                    track_count: col.track_count,
                    discs: col.discs,
                    tag_presence: (col.tag_presence.evaluated_track_count == col.track_count)
                        .then_some(col.tag_presence),
                }
            })
            .collect();

        albums.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        albums
    }

    fn canonical_local_album_map(
        collection: LocalAlbumCollection,
    ) -> HashMap<String, AlbumCollector> {
        let mut map = Self::canonical_local_album_collectors(collection);
        Self::consolidate_dominant_album_titles(&mut map);
        Self::consolidate_local_album_discs(&mut map);
        map
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
        for (key, mut cue) in collection.cue {
            match canonical.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let mut genres = entry.get().genres.clone();
                    for genre in &cue.genres {
                        crate::catalog_genres::add_genre(&mut genres, genre);
                    }
                    if entry.get().track_count == 1
                        && cue.track_count >= 2
                        && cue.discs.len() == 1
                    {
                        cue.observe_added_at(entry.get().added_at.clone());
                        cue.genres = genres;
                        entry.insert(cue);
                    } else {
                        entry.get_mut().observe_added_at(cue.added_at);
                        entry.get_mut().genres = genres;
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
        let mut cur_title = None;
        let mut cur_album = None;
        let mut cur_artist = None;
        let mut cur_album_artist = None;
        let mut cur_date = None;
        let mut cur_added_at = None;
        let mut cur_genres = Vec::new();
        let mut cur_duration = None;
        let mut cur_presence = [false; 5];

        let commit_track = |collection: &mut LocalAlbumCollection,
                            file: &str,
                            track_title: Option<String>,
                            album: Option<String>,
                            artist: Option<String>,
                            album_artist: Option<String>,
                            date: Option<String>,
                            added_at: Option<String>,
                            genres: Vec<String>,
                            duration: Option<f64>,
                            present: [bool; 5]| {
            if file.is_empty() || !local_media::is_catalog_track_uri(file) {
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
            let retain_tracks = collection.mode == LocalAlbumCollectionMode::Full;
            let map = if is_cue {
                &mut collection.cue
            } else {
                &mut collection.physical
            };

            let entry = map.entry(key).or_insert_with(|| AlbumCollector {
                title: title.clone(),
                year: date.clone(),
                added_at: None,
                genres: Vec::new(),
                track_count: 0,
                tag_presence: LocalAlbumTagPresence::default(),
                folder_path: physical_folder.clone(),
                track_uris: Vec::new(),
                tracks: Vec::new(),
                album_artist: None,
                album_artist_consistent: true,
                artists: HashMap::new(),
                discs: Vec::new(),
            });

            if title > entry.title {
                entry.title = title.clone();
            }
            entry.observe_added_at(added_at);
            entry.track_count += 1;
            entry.tag_presence.observe(present);
            if retain_tracks {
                entry.track_uris.push(file.to_string());
                entry.tracks.push((
                    disc_path.clone(),
                    LocalItem {
                        item_type: "file".to_string(),
                        path: file.to_string(),
                        name: path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or(file)
                            .to_string(),
                        title: track_title,
                        artist: artist.clone(),
                        album: Some(title.clone()),
                        duration,
                    },
                ));
            }
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
            for genre in genres {
                crate::catalog_genres::add_genre(&mut entry.genres, &genre);
            }
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
                            cur_title.take(),
                            cur_album.take(),
                            cur_artist.take(),
                            cur_album_artist.take(),
                            cur_date.take(),
                            cur_added_at.take(),
                            std::mem::take(&mut cur_genres),
                            cur_duration.take(),
                            std::mem::replace(&mut cur_presence, [false; 5]),
                        );
                        cur_file = v.to_string();
                        found += 1;
                    }
                    "Added" | "added" if !cur_file.is_empty() => cur_added_at = parse_local_album_added_at(v),
                    "Title" if !cur_file.is_empty() => cur_title = Some(v.to_string()),
                    "Album" if !cur_file.is_empty() => {
                        cur_presence[0] |= !v.trim().is_empty();
                        cur_album = Some(v.to_string());
                    }
                    "Artist" if !cur_file.is_empty() => {
                        cur_presence[1] |= !v.trim().is_empty();
                        cur_artist = Some(v.to_string());
                    }
                    "AlbumArtist" if !cur_file.is_empty() => {
                        cur_presence[2] |= !v.trim().is_empty();
                        cur_album_artist = Some(v.to_string());
                    }
                    "Genre" if !cur_file.is_empty() => {
                        cur_presence[4] |= !v.trim().is_empty();
                        crate::catalog_genres::add_genre(&mut cur_genres, v);
                    }
                    "Date" if !cur_file.is_empty() => {
                        cur_presence[3] |= !v.trim().is_empty();
                        cur_date = Some(v.chars().take(4).collect::<String>());
                    }
                    "duration" | "Time" | "time" | "Duration" if !cur_file.is_empty() => {
                        if cur_duration.is_none() {
                            cur_duration = v.trim().parse::<f64>().ok();
                        }
                    }
                    _ => {}
                }
            }
        }

        commit_track(
            collection,
            &cur_file,
            cur_title,
            cur_album,
            cur_artist,
            cur_album_artist,
            cur_date,
            cur_added_at,
            cur_genres,
            cur_duration,
            cur_presence,
        );

        Ok(found)
    }

    pub fn rescan_library(&self) -> Result<(), String> {
        Self::rescan_library_at(&self.socket_path)
    }

    pub fn rescan_library_at(socket_path: &str) -> Result<(), String> {
        Self::send_command_to_socket(socket_path, "rescan").map(|_| ())
    }

    #[allow(dead_code)]
    pub(crate) fn request_mpd_rescan_job(socket_path: &str) -> Result<u64, String> {
        let lines = Self::send_command_to_socket(socket_path, "rescan")?;
        Self::parse_mpd_update_job_id(&lines).map_err(|error| match error {
            MpdJobIdParseError::MissingJob => {
                "Resposta do rescan MPD não contém job ID.".to_string()
            }
            MpdJobIdParseError::DuplicateJob => {
                "Resposta do rescan MPD contém mais de um job ID.".to_string()
            }
            MpdJobIdParseError::InvalidJob => {
                "Resposta do rescan MPD contém job ID inválido.".to_string()
            }
            MpdJobIdParseError::ContradictoryResponse => {
                "Resposta do rescan MPD contém campos inesperados.".to_string()
            }
            MpdJobIdParseError::Ack => "MPD recusou o comando rescan.".to_string(),
        })
    }

    pub fn update_library(socket_path: &str) -> Result<(), String> {
        Self::send_command_to_socket(socket_path, "update").map(|_| ())
    }

    /// Filesystem evidence proves content may have changed even within MPD's
    /// one-second mtime precision. Force only the validated relative subtree.
    pub(crate) fn rescan_library_path(socket_path: &str, relative_uri: &str) -> Result<u64, String> {
        if !crate::local_media::is_safe_relative_mpd_uri(relative_uri) { return Err("LOCAL_RESCAN_INVALID_PATH".into()); }
        let quoted = Self::quote_mpd_argument(relative_uri)?;
        let lines = Self::send_command_to_socket(socket_path, &format!("rescan {quoted}"))?;
        Self::parse_mpd_update_job_id(&lines).map_err(|_| "LOCAL_RESCAN_INVALID_RESPONSE".into())
    }

    pub fn update_library_path(
        socket_path: &str,
        relative_uri: &str,
    ) -> Result<u64, TargetedLibraryUpdateErrorCode> {
        if !crate::local_media::is_safe_relative_mpd_uri(relative_uri) {
            return Err(TargetedLibraryUpdateErrorCode::InvalidUri);
        }
        let quoted = Self::quote_mpd_argument(relative_uri)
            .map_err(|_| TargetedLibraryUpdateErrorCode::InvalidUri)?;
        let lines = Self::send_command_to_socket(socket_path, &format!("update {quoted}"))
            .map_err(|error| {
                if error.starts_with("ACK") {
                    TargetedLibraryUpdateErrorCode::UpdateCommandFailed
                } else {
                    TargetedLibraryUpdateErrorCode::MpdUnavailable
                }
            })?;
        let mut job_ids = lines.iter().filter_map(|line| {
            line.strip_prefix("updating_db: ")
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
        });
        let Some(job_id) = job_ids.next() else {
            return Err(TargetedLibraryUpdateErrorCode::InvalidResponse);
        };
        if job_ids.next().is_some() {
            return Err(TargetedLibraryUpdateErrorCode::InvalidResponse);
        }
        Ok(job_id)
    }

    #[allow(dead_code)]
    pub(crate) fn parse_mpd_update_job_id(
        lines: &[String],
    ) -> Result<u64, MpdJobIdParseError> {
        Self::parse_mpd_updating_db(lines, false)?.ok_or(MpdJobIdParseError::MissingJob)
    }

    fn parse_mpd_updating_db(
        lines: &[String],
        allow_unrelated_fields: bool,
    ) -> Result<Option<u64>, MpdJobIdParseError> {
        let mut job_id = None;
        for line in lines {
            if line.starts_with("ACK") {
                return Err(MpdJobIdParseError::Ack);
            }
            if line.starts_with("updating_db") {
                let value = line
                    .strip_prefix("updating_db: ")
                    .ok_or(MpdJobIdParseError::InvalidJob)?;
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(MpdJobIdParseError::InvalidJob);
                }
                let parsed = value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or(MpdJobIdParseError::InvalidJob)?;
                if job_id.replace(parsed).is_some() {
                    return Err(MpdJobIdParseError::DuplicateJob);
                }
            } else if !allow_unrelated_fields {
                return Err(MpdJobIdParseError::ContradictoryResponse);
            }
        }
        Ok(job_id)
    }

    #[allow(dead_code)]
    pub(crate) fn wait_for_mpd_update_job(
        socket_path: &str,
        expected_job_id: u64,
        timeout: Duration,
        poll_interval: Duration,
    ) -> MpdRefreshCompletion {
        if expected_job_id == 0 || poll_interval.is_zero() {
            return MpdRefreshCompletion::ProtocolError(
                "Parâmetros inválidos para espera do refresh MPD.".to_string(),
            );
        }
        Self::wait_for_mpd_update_state(
            socket_path,
            Some(expected_job_id),
            timeout,
            poll_interval,
        )
    }

    #[cfg(test)]
    pub(crate) fn wait_for_mpd_idle(
        socket_path: &str,
        timeout: Duration,
        poll_interval: Duration,
    ) -> MpdRefreshCompletion {
        Self::wait_for_mpd_update_state(socket_path, None, timeout, poll_interval)
    }

    /// Settings Save owns a managed transaction: an active MPD job is pending,
    /// irrespective of its duration. Only shutdown cancels this wait; transport
    /// and protocol failures retain the existing explicit error classification.
    pub(crate) fn wait_for_managed_mpd_update(
        socket_path: &str,
        expected_job_id: Option<u64>,
        poll_interval: Duration,
        shutdown_requested: impl FnMut() -> bool,
    ) -> Result<MpdRefreshCompletion, ()> {
        Self::wait_for_managed_mpd_update_with(
            expected_job_id,
            poll_interval,
            shutdown_requested,
            || Self::active_mpd_update_job(socket_path),
            std::thread::sleep,
        )
    }

    fn wait_for_managed_mpd_update_with(
        expected_job_id: Option<u64>,
        poll_interval: Duration,
        mut shutdown_requested: impl FnMut() -> bool,
        mut query: impl FnMut() -> Result<Option<u64>, MpdStatusQueryError>,
        mut sleep: impl FnMut(Duration),
    ) -> Result<MpdRefreshCompletion, ()> {
        if expected_job_id == Some(0) || poll_interval.is_zero() {
            return Ok(MpdRefreshCompletion::ProtocolError(
                "Parâmetros inválidos para espera do refresh MPD.".to_string(),
            ));
        }
        let poll_interval = poll_interval.clamp(Duration::from_millis(10), Duration::from_secs(1));
        loop {
            if shutdown_requested() {
                return Err(());
            }
            let status = query();
            if shutdown_requested() {
                return Err(());
            }
            match status {
                // A different active job may PRECEDE our queued request (MPD
                // automatically scans a missing first database). Its id is not
                // proof that our job completed. Wait until the update queue is
                // idle; this also avoids assuming job ids never wrap.
                Ok(Some(_)) => {}
                Ok(None) => return Ok(MpdRefreshCompletion::Completed),
                Err(MpdStatusQueryError::Unavailable(message)) => {
                    return Ok(MpdRefreshCompletion::Unavailable(message))
                }
                Err(MpdStatusQueryError::Transport(message)) => {
                    return Ok(MpdRefreshCompletion::TransportError(message))
                }
                Err(MpdStatusQueryError::Protocol(message)) => {
                    return Ok(MpdRefreshCompletion::ProtocolError(message))
                }
            }
            sleep(poll_interval);
        }
    }

    fn wait_for_mpd_update_state(
        socket_path: &str,
        expected_job_id: Option<u64>,
        timeout: Duration,
        poll_interval: Duration,
    ) -> MpdRefreshCompletion {
        if poll_interval.is_zero() {
            return MpdRefreshCompletion::ProtocolError(
                "Parâmetros inválidos para espera do refresh MPD.".to_string(),
            );
        }
        let poll_interval = poll_interval.clamp(
            Duration::from_millis(10),
            Duration::from_secs(1),
        );
        let started = Instant::now();
        loop {
            match Self::active_mpd_update_job(socket_path) {
                Ok(Some(active_job_id))
                    if expected_job_id.is_none()
                        || expected_job_id == Some(active_job_id) => {}
                Ok(_) => return MpdRefreshCompletion::Completed,
                Err(MpdStatusQueryError::Unavailable(message)) => {
                    return MpdRefreshCompletion::Unavailable(message)
                }
                Err(MpdStatusQueryError::Transport(message)) => {
                    return MpdRefreshCompletion::TransportError(message)
                }
                Err(MpdStatusQueryError::Protocol(message)) => {
                    return MpdRefreshCompletion::ProtocolError(message)
                }
            }
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return MpdRefreshCompletion::Timeout;
            }
            std::thread::sleep(poll_interval.min(timeout.saturating_sub(elapsed)));
        }
    }

    #[allow(dead_code)]
    fn active_mpd_update_job(socket_path: &str) -> Result<Option<u64>, MpdStatusQueryError> {
        let lines = Self::send_command_to_socket(socket_path, "status").map_err(|error| {
            if error.starts_with("Falha ao conectar no socket MPD:") {
                MpdStatusQueryError::Unavailable(error)
            } else if error.starts_with("ACK") || error.starts_with("Handshake inválido") {
                MpdStatusQueryError::Protocol(error)
            } else {
                MpdStatusQueryError::Transport(error)
            }
        })?;
        Self::parse_mpd_updating_db(&lines, true).map_err(|_| {
            MpdStatusQueryError::Protocol(
                "Status MPD contém campo updating_db inválido.".to_string(),
            )
        })
    }

    #[allow(dead_code)]
    pub(crate) fn audit_raw_local_catalog(
        lines: &[String],
    ) -> Result<CatalogAuditResult, String> {
        let mut result = CatalogAuditResult {
            total_files: 0,
            invalid_count: 0,
            invalid_uris: Vec::new(),
        };
        for line in lines {
            if line.starts_with("ACK") {
                return Err("MPD recusou a leitura do catálogo local.".to_string());
            }
            if line.starts_with("file:") && !line.starts_with("file: ") {
                return Err("Resposta do catálogo contém registro file malformado.".to_string());
            }
            let Some(uri) = line.strip_prefix("file: ") else {
                continue;
            };
            result.total_files += 1;
            let structurally_valid = local_media::is_structurally_valid_local_uri(uri);
            let accepted = structurally_valid && local_media::is_catalog_track_uri(uri);
            if !accepted {
                result.invalid_count += 1;
                if result.invalid_uris.len() < MAX_CATALOG_AUDIT_SAMPLES {
                    result.invalid_uris.push(if structurally_valid {
                        uri.to_string()
                    } else {
                        "<invalid local URI>".to_string()
                    });
                }
            }
        }
        Ok(result)
    }

    pub(crate) fn audit_mpd_local_catalog_at(
        socket_path: &str,
    ) -> Result<CatalogAuditResult, String> {
        let lines = Self::send_command_to_socket(socket_path, "listall")?;
        Self::audit_raw_local_catalog(&lines)
    }

    pub(crate) fn read_validated_local_catalog_at(
        socket_path: &str,
    ) -> Result<HashSet<String>, String> {
        let lines = Self::send_command_to_socket(socket_path, "listall")?;
        if !Self::audit_raw_local_catalog(&lines)?.is_clean() {
            return Err("LOCAL_SOURCE_REMOVAL_INVALID_CATALOG".to_string());
        }
        // Compare typed records: a CUE can legitimately appear both as a
        // directory and a playlist, but duplicate records are ambiguous.
        let mut records = HashSet::new();
        for line in lines {
            let Some((kind, uri)) = line.split_once(": ") else {
                return Err("LOCAL_SOURCE_REMOVAL_INVALID_CATALOG".to_string());
            };
            if !matches!(kind, "file" | "directory" | "playlist")
                || !local_media::is_safe_relative_mpd_uri(uri)
            {
                return Err("LOCAL_SOURCE_REMOVAL_INVALID_CATALOG".to_string());
            }
            if !records.insert(line) {
                return Err("LOCAL_SOURCE_REMOVAL_DUPLICATE_URI".to_string());
            }
        }
        Ok(records)
    }

    /// Identity proof for removal refreshes. Playback is deliberately excluded:
    /// status/song/elapsed may advance while the ordered playlist stays intact.
    pub(crate) fn capture_mpd_queue_structure(
        socket_path: &str,
    ) -> Result<MpdQueueStructure, MpdQueueStructureError> {
        use MpdQueueStructureError::{Changed, Invalid, QueryFailed};
        let before = Self::parse_mpd_queue_status(
            &Self::send_command_to_socket(socket_path, "status").map_err(|_| QueryFailed)?,
        ).map_err(|_| Invalid)?;
        let entries = Self::parse_mpd_queue_entries(
            &Self::send_command_to_socket(socket_path, "playlistinfo").map_err(|_| QueryFailed)?,
        )?;
        let after = Self::parse_mpd_queue_status(
            &Self::send_command_to_socket(socket_path, "status").map_err(|_| QueryFailed)?,
        ).map_err(|_| Invalid)?;
        if before.playlist_version != after.playlist_version
            || before.playlist_length != after.playlist_length
        {
            return Err(Changed);
        }
        if entries.len() != after.playlist_length {
            return Err(Invalid);
        }
        Self::validate_mpd_playback_cursor(&after).map_err(|_| Invalid)?;
        Ok(MpdQueueStructure { entries, playlist_version: after.playlist_version })
    }

    fn parse_mpd_queue_entries(lines: &[String]) -> Result<Vec<MpdQueueEntry>, MpdQueueStructureError> {
        use MpdQueueStructureError::Invalid;
        let mut entries = Vec::new();
        let mut current: Option<(String, Option<usize>, Option<u64>)> = None;
        let mut ids = HashSet::new();
        let mut finish = |entry: (String, Option<usize>, Option<u64>)| {
            let (uri, pos, id) = entry;
            let pos = pos.ok_or(Invalid)?;
            let id = id.ok_or(Invalid)?;
            if pos != entries.len() || !ids.insert(id) {
                return Err(Invalid);
            }
            entries.push(MpdQueueEntry { uri, pos, id });
            Ok(())
        };
        for line in lines {
            let Some((key, value)) = line.split_once(": ") else {
                if ["file:", "Pos:", "Id:"].iter().any(|key| line.starts_with(key)) {
                    return Err(Invalid);
                }
                continue;
            };
            match key {
                "file" => {
                    if let Some(entry) = current.take() { finish(entry)?; }
                    if value.is_empty() || Self::quote_mpd_argument(value).is_err() { return Err(Invalid); }
                    current = Some((value.to_string(), None, None));
                }
                "Pos" => {
                    let entry = current.as_mut().ok_or(Invalid)?;
                    if entry.1.replace(value.parse().map_err(|_| Invalid)?).is_some() { return Err(Invalid); }
                }
                "Id" => {
                    let entry = current.as_mut().ok_or(Invalid)?;
                    if entry.2.replace(value.parse().map_err(|_| Invalid)?).is_some() { return Err(Invalid); }
                }
                _ => {} // Tags do not define queue identity.
            }
        }
        if let Some(entry) = current { finish(entry)?; }
        Ok(entries)
    }

    #[allow(dead_code)]
    pub(crate) fn capture_mpd_playback_snapshot(
        socket_path: &str,
    ) -> Result<MpdPlaybackSnapshot, String> {
        let before = Self::parse_mpd_queue_status(&Self::send_command_to_socket(
            socket_path,
            "status",
        )?)?;
        let uris = Self::parse_mpd_playlist_uris(&Self::send_command_to_socket(
            socket_path,
            "playlistinfo",
        )?)?;
        let after = Self::parse_mpd_queue_status(&Self::send_command_to_socket(
            socket_path,
            "status",
        )?)?;

        if before.playlist_version != after.playlist_version
            || before.playlist_length != after.playlist_length
        {
            return Err("A fila do MPD mudou durante a captura de rollback.".to_string());
        }
        if uris.len() != after.playlist_length {
            return Err(format!(
                "playlistinfo retornou {} item(ns), mas playlistlength informou {}.",
                uris.len(), after.playlist_length
            ));
        }
        Self::validate_mpd_playback_cursor(&after)?;

        Ok(MpdPlaybackSnapshot {
            uris,
            current_index: after.current_index,
            elapsed: after.elapsed,
            state: after.state,
            playlist_version: after.playlist_version,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn restore_mpd_playback_snapshot(
        socket_path: &str,
        snapshot: &MpdPlaybackSnapshot,
    ) -> Result<(), String> {
        Self::validate_mpd_playback_snapshot(snapshot)?;
        let mut commands = String::from("command_list_begin\nclear\n");
        for uri in &snapshot.uris {
            commands.push_str(&format!("add {}\n", Self::quote_mpd_argument(uri)?));
        }
        if snapshot.state != PlaybackState::Stopped {
            let current_index = snapshot.current_index.ok_or_else(|| {
                "Snapshot ativo do MPD não contém índice da faixa atual.".to_string()
            })?;
            commands.push_str(&format!("play {current_index}\n"));
            commands.push_str("pause 1\n");
        }
        commands.push_str("command_list_end");
        Self::send_command_to_socket(socket_path, &commands)
            .map_err(|error| Self::sanitize_mpd_error(error, &snapshot.uris))?;

        if snapshot.state == PlaybackState::Stopped {
            return Ok(());
        }
        if snapshot.elapsed > 0.0 {
            Self::send_command_to_socket(
                socket_path,
                &format!("seekcur {:.3}", snapshot.elapsed),
            )
            .map_err(|error| Self::sanitize_mpd_error(error, &snapshot.uris))?;
        }
        Ok(())
    }

    fn parse_mpd_queue_status(lines: &[String]) -> Result<MpdQueueStatus, String> {
        let mut playlist_version: Option<u64> = None;
        let mut playlist_length: Option<usize> = None;
        let mut current_index: Option<usize> = None;
        let mut elapsed: Option<f64> = None;
        let mut state: Option<PlaybackState> = None;
        for line in lines {
            let Some((key, value)) = line.split_once(": ") else {
                if ["playlist", "playlistlength", "song", "elapsed", "state"]
                    .iter()
                    .any(|key| line.starts_with(key))
                {
                    return Err("Status MPD contém campo de fila malformado.".to_string());
                }
                continue;
            };
            match key {
                "playlist" => {
                    if playlist_version
                        .replace(Self::parse_mpd_number("playlist", value)?)
                        .is_some()
                    {
                        return Err("Status MPD contém playlist duplicada.".to_string());
                    }
                }
                "playlistlength" => {
                    if playlist_length
                        .replace(Self::parse_mpd_number("playlistlength", value)?)
                        .is_some()
                    {
                        return Err("Status MPD contém playlistlength duplicado.".to_string());
                    }
                }
                "song" => {
                    if current_index
                        .replace(Self::parse_mpd_number("song", value)?)
                        .is_some()
                    {
                        return Err("Status MPD contém song duplicado.".to_string());
                    }
                }
                "elapsed" => {
                    if elapsed
                        .replace(Self::parse_mpd_number("elapsed", value)?)
                        .is_some()
                    {
                        return Err("Status MPD contém elapsed duplicado.".to_string());
                    }
                }
                "state" => {
                    if state.replace(Self::parse_playback_state(value)?).is_some() {
                        return Err("Status MPD contém state duplicado.".to_string());
                    }
                }
                _ => {}
            }
        }

        let state = state.ok_or_else(|| "Status MPD sem estado de reprodução.".to_string())?;
        let elapsed = match elapsed {
            None if state == PlaybackState::Stopped => 0.0,
            Some(value) if value.is_finite() && value >= 0.0 => value,
            _ => return Err("Status MPD sem posição válida da faixa atual.".to_string()),
        };
        Ok(MpdQueueStatus {
            playlist_version: playlist_version
                .ok_or_else(|| "Status MPD sem versão da playlist.".to_string())?,
            playlist_length: playlist_length
                .ok_or_else(|| "Status MPD sem playlistlength.".to_string())?,
            current_index,
            elapsed,
            state,
        })
    }

    fn parse_mpd_playlist_uris(lines: &[String]) -> Result<Vec<String>, String> {
        let mut uris = Vec::new();
        for line in lines {
            if line.starts_with("file:") && !line.starts_with("file: ") {
                return Err("playlistinfo contém URI malformada.".to_string());
            }
            let Some(uri) = line.strip_prefix("file: ") else {
                continue;
            };
            if uri.is_empty() {
                return Err("playlistinfo contém URI vazia.".to_string());
            }
            Self::quote_mpd_argument(uri)?;
            uris.push(uri.to_string());
        }
        Ok(uris)
    }

    fn validate_mpd_playback_cursor(status: &MpdQueueStatus) -> Result<(), String> {
        if status.playlist_length == 0 {
            if status.current_index.is_some() || status.state != PlaybackState::Stopped {
                return Err("Status MPD inconsistente para fila vazia.".to_string());
            }
            return Ok(());
        }
        if status
            .current_index
            .is_some_and(|index| index >= status.playlist_length)
        {
            return Err("Status MPD contém índice fora da fila.".to_string());
        }
        if status.state != PlaybackState::Stopped && status.current_index.is_none() {
            return Err("Status MPD ativo sem índice da faixa atual.".to_string());
        }
        Ok(())
    }

    fn validate_mpd_playback_snapshot(snapshot: &MpdPlaybackSnapshot) -> Result<(), String> {
        for uri in &snapshot.uris {
            if uri.is_empty() {
                return Err("Snapshot do MPD contém URI vazia.".to_string());
            }
            Self::quote_mpd_argument(uri)?;
        }
        if !snapshot.elapsed.is_finite() || snapshot.elapsed < 0.0 {
            return Err("Snapshot do MPD contém posição inválida.".to_string());
        }
        let status = MpdQueueStatus {
            playlist_version: snapshot.playlist_version,
            playlist_length: snapshot.uris.len(),
            current_index: snapshot.current_index,
            elapsed: snapshot.elapsed,
            state: snapshot.state,
        };
        Self::validate_mpd_playback_cursor(&status)
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
                eprintln!(
                    "[Persistence] Failed to validate queue cache permissions: {}",
                    crate::diagnostics::error_summary(&error)
                );
                return (Vec::new(), None);
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
                                            eprintln!("[Persistence] Failed to persist migrated queue cache: {}", crate::diagnostics::error_summary(&error));
                                        }
                                    }
                                    Err(error) => eprintln!("[Persistence] Failed to encode migrated queue cache: {}", crate::diagnostics::error_summary(&error)),
                                }
                            }
                            return (queue, resume);
                        }
                        Err(error) => eprintln!(
                            "[Persistence] Failed to decode queue cache: {}",
                            crate::diagnostics::error_summary(&error)
                        ),
                    },
                    Err(error) => {
                        eprintln!(
                            "[Persistence] Failed to open queue cache: {}",
                            crate::diagnostics::error_summary(&error)
                        )
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
        }
    }

    pub(crate) fn ensure_queue_coherent(&self) -> Result<(), String> {
        if self.queue_coherent { Ok(()) } else { Err("PLAYBACK_QUEUE_UNCERTAIN".into()) }
    }

    pub(crate) fn ensure_queue_persisted(&self) -> Result<(), String> {
        self.ensure_queue_coherent()?;
        if self.cache_persistence_pending { return Err("PLAYBACK_PERSISTENCE_FAILED".into()); }
        Ok(())
    }

    pub(crate) fn watch_eq_transition(&self) -> EqTransitionWatch {
        EqTransitionWatch {
            epoch: self.eq_intervention.clone(),
            expected: self.eq_intervention.load(Ordering::Acquire),
        }
    }
    pub(crate) fn note_eq_intervention(&self) {
        self.eq_intervention.fetch_add(1, Ordering::AcqRel);
    }

    /// EQ-only restoration: select paused in the command batch, then seek;
    /// unrelated output switches retain their accepted paused contract.
    pub(crate) fn restore_eq_switch(
        &mut self,
        saved: Option<DeviceSwitchSnapshot>,
        resolved: &ResolvedDeviceSwitchQueue,
    ) -> Result<DeviceSwitchRestoreOutcome, String> {
        if saved.is_some_and(|s| !s.elapsed.is_finite() || s.elapsed < 0.) {
            return Err("EQ_RESUME_INVALID_POSITION".into());
        }
        self.validate_device_switch_queue(resolved)?;
        let mut batch =
            self.device_switch_queue_restore_commands(saved, &resolved.playback_uris)?;
        if saved.is_some_and(|s| s.state != PlaybackState::Stopped) {
            batch = batch.replace("command_list_end", "pause 1\ncommand_list_end");
        }
        if self.send_command(&batch).is_err(){
            self.mark_queue_uncertain();return Err("PLAYBACK_QUEUE_UNCERTAIN".into());
        }
        if let Some(s) = saved {
            if s.state != PlaybackState::Stopped && s.elapsed > 0.0 {
                return Self::restore_position(
                    s.elapsed,
                    &mut |cmd| self.send_command(cmd),
                    &mut std::thread::sleep,
                );
            }
        }
        Ok(DeviceSwitchRestoreOutcome::Complete)
    }
    /// Linearized with human transport by AudioState's mutex. Recheck the watch
    /// AFTER observation I/O, immediately before the single resume effect.
    pub(crate) fn resume_eq_switch(
        &mut self,
        saved: Option<DeviceSwitchSnapshot>,
        resolved: &ResolvedDeviceSwitchQueue,
        watch: &EqTransitionWatch,
    ) -> Result<&'static str, String> {
        self.resume_checked_output_switch(saved,resolved,watch,false,|audio,_|audio.send_command("pause 0"))
    }
    pub(crate) fn resume_saved_route(
        &mut self,
        saved: Option<DeviceSwitchSnapshot>,
        resolved: &ResolvedDeviceSwitchQueue,
        watch: &EqTransitionWatch,
        owner: &crate::config_save_operation::Transaction<'_>,
        verify_mute: impl FnOnce(&Self)->Result<(),String>,
    ) -> Result<&'static str,String> {
        let result=self.resume_checked_output_switch(saved,resolved,watch,true,|audio,expected| {
            let s=saved.ok_or("OUTPUT_SAVE_RESUME_INVALID")?;
            audio.direct_playback.as_ref().ok_or("OUTPUT_SAVE_RESUME_GUARD_UNAVAILABLE")?
                .start_saved_route(&audio.socket_path,owner,||watch.current(),s.queue_index,s.elapsed,||{
                    let actual=Self::capture_mpd_queue_structure(&audio.socket_path).map_err(|_|"OUTPUT_SAVE_RESUME_QUEUE_UNCONFIRMED")?;
                    if actual.playlist_version!=expected.playlist_version||actual.entries!=expected.entries {
                        return Err("OUTPUT_SAVE_RESUME_QUEUE_CHANGED".into());
                    }
                    let status=Self::parse_mpd_queue_status(&audio.send_command("status")?)?;
                    if status.state!=PlaybackState::Stopped{return Err("OUTPUT_SAVE_RESUME_CONTEXT_CHANGED".into());}
                    verify_mute(audio)
                })
        });
        if matches!(result,Ok("resumed")){self.restored_resume=None;}
        result
    }
    fn resume_checked_output_switch(
        &mut self,saved:Option<DeviceSwitchSnapshot>,resolved:&ResolvedDeviceSwitchQueue,
        watch:&EqTransitionWatch,logical:bool,resume:impl FnOnce(&Self,&MpdQueueStructure)->Result<Vec<String>,String>,
    )->Result<&'static str,String> {
        let Some(s) = saved else { return Ok("stopped") };
        if s.state != PlaybackState::Playing {
            return Ok(if s.state == PlaybackState::Stopped {
                "stopped"
            } else {
                "paused"
            });
        }
        if !watch.current() {
            return Ok("superseded");
        }
        if saved.is_some_and(|s| !s.elapsed.is_finite() || s.elapsed < 0.) {
            return Err("EQ_RESUME_INVALID_POSITION".into());
        }
        self.validate_device_switch_queue(resolved)?;
        let before = Self::parse_mpd_queue_status(&self.send_command("status")?)?;
        let context_valid=if logical {
            before.state==PlaybackState::Stopped
                && self.restored_resume==Some(PlaybackResumeSnapshot{queue_index:s.queue_index,elapsed:s.elapsed,state:PlaybackState::Paused})
        } else {
            before.state==PlaybackState::Paused && before.current_index==Some(s.queue_index)
                && before.elapsed.is_finite() && (before.elapsed-s.elapsed).abs()<=0.25
        };
        if !context_valid {
            return Err("EQ_RESUME_CONTEXT_CHANGED".into());
        }
        let structure = Self::capture_mpd_queue_structure(&self.socket_path)
            .map_err(|_| "EQ_RESUME_QUEUE_UNCONFIRMED")?;
        if structure.entries.len() != resolved.playback_uris.len()
            || !structure
                .entries
                .iter()
                .zip(&resolved.playback_uris)
                .all(|(a, b)| &a.uri == b)
        {
            self.mark_queue_uncertain();
            return Err("EQ_RESUME_QUEUE_CHANGED".into());
        }
        if !watch.current() {
            return Ok("superseded");
        }
        if resume(self,&structure).is_err() {
            let _ = self.send_command("pause 1");
            return Err("EQ_RESUME_UNCONFIRMED".into());
        }
        let after = match self
            .send_command("status")
            .and_then(|v| Self::parse_mpd_queue_status(&v))
        {
            Ok(v) => v,
            Err(_) => {
                let _ = self.send_command("pause 1");
                return Err("EQ_RESUME_UNCONFIRMED".into());
            }
        };
        if after.state != PlaybackState::Playing
            || after.current_index != Some(s.queue_index)
            || after.playlist_version != structure.playlist_version
        {
            let _ = self.send_command("pause 1");
            return Err("EQ_RESUME_UNCONFIRMED".into());
        }
        Ok("resumed")
    }
    pub(crate) fn invalidate_playback_intents(&mut self) {
        self.playback_intent = self.playback_intent.saturating_add(1);
    }

    pub(crate) fn admit_playback_intent(&mut self) -> Result<String, String> {
        self.ensure_queue_structure_available()?;
        self.playback_intent = self.playback_intent.checked_add(1).ok_or("PLAYBACK_INTENT_EXHAUSTED")?;
        Ok(self.playback_intent.to_string())
    }

    pub(crate) fn validate_playback_intent(&self, token: &str) -> Result<(), String> {
        if token == self.playback_intent.to_string() { Ok(()) } else { Err("PLAYBACK_SUPERSEDED".into()) }
    }

    pub(crate) fn cancel_playback_intent(&mut self, token: &str) {
        if self.validate_playback_intent(token).is_ok() { self.invalidate_playback_intents(); }
    }

    pub(crate) fn mark_queue_uncertain(&mut self) {
        self.queue_coherent = false;
        self.restored_resume = None;
        self.startup_restore = StartupQueueRestore::Failed;
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

    pub(crate) fn observed_software_volume(&self) -> Result<u32,String> {
        crate::direct_volume::observed_software_volume(self.send_command("status")?)
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

    pub(crate) fn attach_direct_playback(&mut self,gate:Arc<crate::direct_playback::Gate>){self.direct_playback=Some(gate);}
    fn send_with_gate(socket:&str,gate:Option<&crate::direct_playback::Gate>,command:&str)->Result<Vec<String>,String>{
        match gate{Some(gate)=>gate.send(socket,command),None=>Self::send_command_to_socket(socket,command)}
    }
    fn send_command(&self, command: &str) -> Result<Vec<String>, String> {
        Self::send_with_gate(&self.socket_path,self.direct_playback.as_deref(),command)
    }

    pub(crate) fn send_command_to_socket(socket_path: &str, command: &str) -> Result<Vec<String>, String> {
        Self::send_command_to_socket_before(socket_path, command, None)
    }

    fn send_command_to_socket_before(
        socket_path: &str,
        command: &str,
        deadline: Option<Instant>,
    ) -> Result<Vec<String>, String> {
        mpd_deadline_timeout(deadline)?;
        if let Some(pid) = crate::initial_plex_playback::command_peer(socket_path)? {
            let deadline = deadline.unwrap_or_else(|| Instant::now() + Duration::from_millis(500));
            let stream = crate::signal_processing::connect(Path::new(socket_path), pid, deadline)
                .map_err(|_| "PLAYBACK_SUPERSEDED".to_string())?;
            return Self::send_command_on_stream(stream, command, Some(deadline));
        }
        let stream = UnixStream::connect(socket_path)
            .map_err(|e| format!("Falha ao conectar no socket MPD: {}", e))?;
        Self::send_command_on_stream(stream, command, deadline)
    }

    pub(crate) fn send_owned_command_to_socket(socket_path: &str, pid: u32, command: &str) -> Result<Vec<String>, String> {
        if crate::initial_plex_playback::command_peer(socket_path)?.is_some_and(|expected| expected != pid) {
            return Err("PLAYBACK_SUPERSEDED".into());
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        let stream = crate::signal_processing::connect(Path::new(socket_path), pid, deadline)
            .map_err(|_| crate::direct_volume::STALE.to_string())?;
        Self::send_command_on_stream(stream, command, Some(deadline))
    }

    fn send_command_on_stream(mut stream: UnixStream, command: &str, deadline: Option<Instant>) -> Result<Vec<String>, String> {
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| format!("Falha ao configurar timeout de leitura do MPD: {}", e))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| format!("Falha ao configurar timeout de escrita do MPD: {}", e))?;

        let mut reader = BufReader::new(MpdDeadlineReader {
            stream: stream.try_clone().map_err(|e| e.to_string())?,
            deadline,
        });
        let mut welcome = String::new();
        let welcome_bytes = reader
            .read_line(&mut welcome)
            .map_err(|e| format!("Falha ao ler handshake do MPD: {}", e))?;
        if welcome_bytes == 0 || !welcome.starts_with("OK MPD ") || !welcome.ends_with('\n') {
            return Err("Handshake inválido ou incompleto recebido do MPD.".to_string());
        }

        let cmd = format!("{}\n", command);
        if deadline.is_some() {
            stream
                .set_write_timeout(Some(mpd_deadline_timeout(deadline)?))
                .map_err(|e| format!("Falha ao configurar prazo de escrita do MPD: {e}"))?;
        }
        stream
            .write_all(cmd.as_bytes())
            .map_err(|e| format!("Falha ao enviar comando ao MPD: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("Falha ao concluir envio do comando ao MPD: {}", e))?;

        match deadline {
            Some(_) => Self::read_mpd_response_before(&mut reader, deadline),
            None => Self::read_mpd_response(&mut reader),
        }
    }

    #[cfg(test)]
    pub fn get_local_cover_at(
        socket_path: &str,
        library_dir: &Path,
        path: &str,
        local_folders: &[String],
    ) -> Result<Option<String>, String> {
        let resolved = {
            let _diagnostic = crate::diagnostics::DevSpan::new("cover.resolve_root");
            local_media::resolve_local_path(library_dir, path, local_folders)
                .map_err(|error| error.to_string())?
        };
        let full_path = resolved.physical_path;
        let local_cover = {
            let _diagnostic = crate::diagnostics::DevSpan::new("cover.folder");
            if full_path.is_dir() {
                find_folder_cover_path(&full_path)
            } else {
                full_path.parent().and_then(find_folder_cover_path)
            }
        };
        if local_cover.is_some() {
            return Ok(local_cover);
        }
        let _diagnostic = crate::diagnostics::DevSpan::new("cover.mpd");
        Self::embedded_cover_for_folder(socket_path, path)
    }

    // Source-scoped path: the caller owns real lifecycle/Lease admission and
    // the target owns checked FDs. Legacy get_local_cover_at stays unchanged.
    pub(crate) fn get_local_cover_from_source(
        socket_path:&str,path:&str,target:&local_media::SourceCoverTarget<'_>,
        verify_context:&dyn Fn()->Result<(),String>,
    )->Result<Option<String>,String> {
        target.validate()?;
        for name in LOCAL_FOLDER_COVER_NAMES {
            if let Some(file)=target.open_cover(name)? {
                let bytes=read_limited_local_cover(file.reader(),file.len());
                file.validate()?;
                if let Some(bytes)=bytes {
                    let mime=match Path::new(name).extension().and_then(|v|v.to_str()).unwrap_or("jpeg") {
                        "png"=>"image/png","webp"=>"image/webp",_=>"image/jpeg",
                    };
                    verify_context()?;target.validate()?;
                    return Ok(Some(format!("data:{mime};base64,{}",BASE64.encode(&bytes))));
                }
            }
        }
        // MPD still opens its own URI: local FDs do not bind remote readpicture.
        // Keep the existing commands, size/MIME handling and three-probe limit.
        verify_context()?;target.validate()?;
        let command=format!("lsinfo {}",Self::quote_mpd_argument(path)?);
        let lines=Self::send_command_to_socket(socket_path,&command)?;
        verify_context()?;target.validate()?;
        let tracks=lines.iter().filter_map(|line|line.strip_prefix("file: "))
            .filter(|uri|Path::new(uri).parent()==Some(Path::new(path)))
            .filter(|uri|local_media::is_catalog_track_uri(uri)).take(MAX_EMBEDDED_COVER_TRACK_PROBES);
        for uri in tracks {
            verify_context()?;target.validate_probe_uri(uri)?;
            let cover=Self::embedded_cover_for_track(socket_path,uri)?;
            verify_context()?;target.validate()?;
            if cover.is_some() {return Ok(cover);}
        }
        verify_context()?;target.validate()?;Ok(None)
    }

    #[cfg(test)]
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
            .filter(|uri| local_media::is_catalog_track_uri(uri))
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
        Self::read_mpd_response_before(reader, None)
    }

    fn read_mpd_response_before<R: BufRead>(
        reader: &mut R,
        deadline: Option<Instant>,
    ) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        loop {
            mpd_deadline_timeout(deadline)?;
            let mut line = String::new();
            let bytes = reader
                .read_line(&mut line)
                .map_err(|e| format!("Falha ao ler resposta do MPD: {}", e))?;
            mpd_deadline_timeout(deadline)?;
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
        self.invalidate_playback_intents();
        self.ensure_queue_coherent()?;
        if self.cache_persistence_pending { return Err("PLAYBACK_PERSISTENCE_FAILED".into()); }
        self.ensure_logical_resume_available()?;
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

    /// Called under AudioState, before copying the logical queue. Startup must
    /// be ready; device/catalog recovery is serialized by the Save slot.
    pub(crate) fn reserve_queue_structure(&mut self) -> Option<QueueStructureReservation> {
        if self.startup_restore != StartupQueueRestore::Ready
            || self
                .queue_structure_reserved
                .compare_exchange(
                    0,
                    QUEUE_STRUCTURE_RESERVED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
        {
            return None;
        }
        Some(QueueStructureReservation(
            Arc::clone(&self.queue_structure_reserved),
            true,
            Arc::new(()),
        ))
    }

    fn require_queue_owner(&self, owner: &QueueStructureReservation) -> Result<(), String> {
        if !Arc::ptr_eq(&owner.0, &self.queue_structure_reserved)
            || owner.0.load(Ordering::SeqCst) == 0
        {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_OWNER_INVALID".into());
        }
        Ok(())
    }

    pub(crate) fn capture_device_switch_queue(
        &mut self,
    ) -> Result<DeviceSwitchQueueSnapshot, String> {
        self.ensure_queue_coherent()?;
        self.invalidate_playback_intents();
        let owner = self
            .reserve_queue_structure()
            .ok_or("A fila não está disponível para preparar a troca de saída.")?;
        Ok(DeviceSwitchQueueSnapshot {
            owner,
            tracks: self.queue.clone(),
        })
    }

    pub(crate) fn validate_device_switch_queue(
        &self,
        resolved: &ResolvedDeviceSwitchQueue,
    ) -> Result<(), String> {
        self.require_queue_owner(&resolved.snapshot.owner)?;
        // Compare ordered media identities, including duplicate occurrences.
        // Presentation metadata and transient Plex routes are not identities.
        let tracks = &resolved.snapshot.tracks;
        if self.startup_restore != StartupQueueRestore::Ready
            || self.queue.len() != tracks.len()
            || resolved.playback_uris.len() != tracks.len()
            || !self.queue.iter().zip(tracks).all(|(current, captured)| {
                current.media_locator == captured.media_locator && current.uri == captured.uri
            })
        {
            return Err("A fila mudou durante a preparação da troca de saída.".into());
        }
        Ok(())
    }

    pub(crate) fn prepare_reserved_device_switch(
        &mut self,
        resolved: &ResolvedDeviceSwitchQueue,
    ) -> Result<Option<DeviceSwitchSnapshot>, DeviceSwitchPreparationError> {
        self.validate_device_switch_queue(resolved)
            .map_err(|cause| DeviceSwitchPreparationError {
                cause,
                snapshot: None,
            })?;
        self.prepare_device_switch()
    }

    pub(crate) fn prepare_reserved_route_save(&mut self,resolved:&ResolvedDeviceSwitchQueue)->Result<Option<DeviceSwitchSnapshot>,DeviceSwitchPreparationError>{
        let logical=self.restored_resume;
        let captured=self.prepare_reserved_device_switch(resolved)?;
        if captured.is_some_and(|s|s.state==PlaybackState::Stopped) {
            if let Some(cursor)=logical.filter(|s|s.valid_for(self.queue.len())) {
                // A stopped daemon with a logical paused cursor is never a
                // Playing origin. Keep its position across successive Saves.
                return Ok(Some(DeviceSwitchSnapshot{queue_index:cursor.queue_index,elapsed:cursor.elapsed,state:PlaybackState::Paused}));
            }
        }
        Ok(captured)
    }

    pub(crate) fn queue_has_logical_resume(&self) -> bool {
        self.restored_resume.is_some()
    }

    /// Called under AudioState: capture and fence are atomic relative to all
    /// player commands. No MPD/filesystem I/O and no cursor fence on failure.
    #[allow(dead_code)]
    pub(crate) fn capture_reserved_logical_queue(
        &self,
        owner: &QueueStructureReservation,
    ) -> Result<ReservedLogicalQueueSnapshot, String> {
        self.require_queue_owner(owner)?;
        if self.startup_restore != StartupQueueRestore::Ready {
            return Err("LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_INVALID".into());
        }
        let resume = self
            .restored_resume
            .filter(|cursor| cursor.valid_for(self.queue.len()))
            .ok_or("LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_INVALID")?;
        let tracks = self.queue.clone();
        owner.0.fetch_or(LOGICAL_RESUME_RESERVED, Ordering::SeqCst);
        Ok(ReservedLogicalQueueSnapshot {
            tracks,
            resume,
            identity: Arc::clone(&owner.2),
        })
    }

    /// Consume a verified capability under AudioState. Durable cache writing is
    /// still performed by the transaction owner after releasing AudioState.
    #[allow(dead_code)]
    pub(crate) fn apply_verified_reserved_logical_queue(
        &mut self,
        owner: &QueueStructureReservation,
        verified: VerifiedLogicalQueueRestore,
    ) -> Result<Vec<u8>, String> {
        self.require_queue_owner(owner)?;
        if !Arc::ptr_eq(&owner.2, &verified.identity)
            || owner.0.load(Ordering::SeqCst) & LOGICAL_RESUME_RESERVED == 0
            || !verified.resume.valid_for(verified.tracks.len())
        {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_OWNER_INVALID".into());
        }
        self.queue = verified.tracks;
        self.restored_resume = Some(verified.resume);
        self.startup_restore = StartupQueueRestore::Ready;
        Ok(verified.cache)
    }

    fn ensure_logical_resume_available(&self) -> Result<(), String> {
        if self.queue_structure_reserved.load(Ordering::SeqCst) & LOGICAL_RESUME_RESERVED != 0 {
            return Err("LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_RESERVED: aguarde a conclusão da remoção de fontes locais.".into());
        }
        Ok(())
    }

    /// A destructive transaction that cannot prove recovery keeps mutations
    /// fenced even after its worker returns. Startup is also blocked by its intent.
    pub(crate) fn retain_uncertain_queue_reservation(
        &mut self,
        owner: &mut QueueStructureReservation,
    ) {
        if self.require_queue_owner(owner).is_ok() {
            self.startup_restore = StartupQueueRestore::Failed;
            owner.1 = false;
        }
    }

    pub(crate) fn begin_reserved_queue_effect<'reservation>(
        &mut self,
        owner: &'reservation mut QueueStructureReservation,
    ) -> Result<ReservedQueueEffect<'reservation>, String> {
        self.require_queue_owner(owner)?;
        owner.arm_destructive();
        self.startup_restore = StartupQueueRestore::Failed;
        Ok(ReservedQueueEffect {
            socket_path: self.socket_path.clone(),
            owner: Arc::clone(&owner.0),
            identity: Arc::clone(&owner.2),
            _owner_scope: std::marker::PhantomData,
        })
    }

    pub(crate) fn apply_verified_reserved_queue(
        &mut self,
        owner: &QueueStructureReservation,
        tracks: Vec<TrackMetadata>,
    ) -> Result<(), String> {
        self.require_queue_owner(owner)?;
        self.queue = tracks;
        self.restored_resume = None;
        self.startup_restore = StartupQueueRestore::Ready;
        Ok(())
    }

    fn ensure_queue_structure_available(&self) -> Result<(), String> {
        if self.queue_structure_reserved.load(Ordering::SeqCst) != 0 {
            return Err("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED: aguarde a conclusão da remoção de fontes locais.".to_string());
        }
        Ok(())
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
        self.ensure_queue_structure_available()?;
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
        self.ensure_logical_resume_available()
            .map_err(before_stop)?;
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
    #[cfg(test)]
    pub fn restore_after_device_switch(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        playback_uris: &[String],
    ) -> Result<DeviceSwitchRestoreOutcome, String> {
        if self.queue.is_empty() {
            return Ok(DeviceSwitchRestoreOutcome::Complete);
        }

        let mut send_command = |command: &str| self.send_command(command);
        self.restore_after_device_switch_with(
            saved_state,
            playback_uris,
            &mut send_command,
            &mut std::thread::sleep,
        )
    }

    /// Route Save stages only the queue and a logical paused cursor. It sends no
    /// play/seek before durable commit, including on an unattenuated Shared sink.
    pub(crate) fn restore_for_route_save(&mut self,saved:Option<DeviceSwitchSnapshot>,resolved:&ResolvedDeviceSwitchQueue)->Result<DeviceSwitchRestoreOutcome,String>{
        self.validate_device_switch_queue(resolved)?;
        if saved.is_some_and(|s|!s.elapsed.is_finite()||s.elapsed<0.||s.queue_index>=self.queue.len()){return Err("OUTPUT_SAVE_RESUME_INVALID".into());}
        let command=self.device_switch_queue_restore_commands(None,&resolved.playback_uris)?;
        if Self::send_command_to_socket(&self.socket_path,&command).is_err(){self.mark_queue_uncertain();return Err("PLAYBACK_QUEUE_UNCERTAIN".into());}
        self.restored_resume=saved.filter(|s|s.state!=PlaybackState::Stopped).map(|s|PlaybackResumeSnapshot{queue_index:s.queue_index,elapsed:s.elapsed,state:PlaybackState::Paused});
        Ok(DeviceSwitchRestoreOutcome::Complete)
    }

    #[cfg(test)]
    pub(crate) fn restore_after_reserved_device_switch(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        resolved: &ResolvedDeviceSwitchQueue,
    ) -> Result<DeviceSwitchRestoreOutcome, String> {
        self.restore_after_reserved_device_switch_with(
            saved_state,
            resolved,
            &mut |command| Self::send_command_to_socket(&self.socket_path,command),
            &mut std::thread::sleep,
        )
    }

    #[cfg(test)]
    fn restore_after_reserved_device_switch_with<F, S>(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        resolved: &ResolvedDeviceSwitchQueue,
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<DeviceSwitchRestoreOutcome, String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        self.validate_device_switch_queue(resolved)?;
        if self.queue.is_empty() {
            return Ok(DeviceSwitchRestoreOutcome::Complete);
        }
        self.restore_device_switch_playback_with(
            saved_state,
            &resolved.playback_uris,
            send_command,
            sleep,
        )
    }

    #[cfg(test)]
    fn restore_after_device_switch_with<F, S>(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        playback_uris: &[String],
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<DeviceSwitchRestoreOutcome, String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        self.ensure_queue_structure_available()?;
        self.restore_device_switch_playback_with(saved_state, playback_uris, send_command, sleep)
    }

    #[cfg(test)]
    fn restore_device_switch_playback_with<F, S>(
        &self,
        saved_state: Option<DeviceSwitchSnapshot>,
        playback_uris: &[String],
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<DeviceSwitchRestoreOutcome, String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        let restore_queue =
            self.device_switch_queue_restore_commands(saved_state, playback_uris)?;
        send_command(&restore_queue)
            .map_err(|error| Self::sanitize_mpd_error(error, playback_uris))?;

        let Some(snapshot) = saved_state else {
            return Ok(DeviceSwitchRestoreOutcome::Complete);
        };
        if snapshot.state == PlaybackState::Stopped {
            return Ok(DeviceSwitchRestoreOutcome::Complete);
        }

        let outcome = if snapshot.elapsed > 0.5 {
            Self::restore_position(snapshot.elapsed, send_command, sleep)?
        } else { DeviceSwitchRestoreOutcome::Complete };

        send_command("pause 1")?;

        Ok(outcome)
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
    ) -> Result<DeviceSwitchRestoreOutcome, String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        const ATTEMPTS: usize = 3;
        const RETRY_DELAY: Duration = Duration::from_millis(75);
        let command = format!("seekcur {:.1}", elapsed);

        for attempt in 0..ATTEMPTS {
            match send_command(&command) {
                Ok(_) => return Ok(DeviceSwitchRestoreOutcome::Complete),
                Err(error) if Self::is_not_seekable_ack(&error) => {
                    if attempt + 1 < ATTEMPTS {
                        sleep(RETRY_DELAY);
                    }
                }
                Err(error) => return Err(error),
            }
        }

        eprintln!(
            "[Audio] Warning: MPD kept the track active but refused to restore its time position (Not seekable)."
        );
        Ok(DeviceSwitchRestoreOutcome::PositionNotRestored)
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

    pub fn play_tracks(&mut self, tracks: Vec<TrackMetadata>, playback_uris: Vec<String>, start_index: usize) -> Result<(), String> {
        self.note_eq_intervention();
        let socket = self.socket_path.clone();
        let gate=self.direct_playback.clone();
        self.play_tracks_with_io(tracks, playback_uris, start_index,
            |command| Self::send_with_gate(&socket,gate.as_deref(),command), |audio| audio.save_queue_cache())
    }

    fn play_tracks_with_io(
        &mut self,
        mut tracks: Vec<TrackMetadata>,
        playback_uris: Vec<String>,
        start_index: usize,
        mut send: impl FnMut(&str) -> Result<Vec<String>, String>,
        mut persist: impl FnMut(&Self) -> Result<(), String>,
    ) -> Result<(), String> {
        self.ensure_queue_structure_available()?;
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

        if let Err(error) = send(&batch) {
            if crate::direct_playback::is_admission_refusal(&error){return Err(error);}
            // A command list is sequential, never transactional. ACK/lost reply
            // cannot establish which effects ran; retain the last durable cache.
            self.mark_queue_uncertain();
            return Err("PLAYBACK_QUEUE_UNCERTAIN".into());
        }
        self.queue = tracks;
        self.restored_resume = None;
        self.startup_restore = StartupQueueRestore::Ready;
        self.queue_coherent = true;
        self.cache_persistence_pending = true;
        persist(self).map_err(|_| "PLAYBACK_PERSISTENCE_FAILED".to_string())?;
        self.cache_persistence_pending = false;
        Ok(())
    }

    pub fn play_uris(&mut self, uris: Vec<String>, start_index: usize) -> Result<(), String> {
        self.ensure_queue_structure_available()?;
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
        self.ensure_queue_coherent()?;
        self.ensure_logical_resume_available()?;
        match self.startup_restore {
            StartupQueueRestore::Ready => Ok(()),
            StartupQueueRestore::Pending => Err("A fila ainda está sendo restaurada no MPD.".to_string()),
            StartupQueueRestore::Failed => Err("A fila não pôde ser restaurada no MPD; selecione uma faixa novamente.".to_string()),
        }
    }

    fn playback_state_with_missing_error(
        &self,
        missing_error: &str,
    ) -> Result<PlaybackState, String> {
        self.send_command("status")?
            .into_iter()
            .find_map(|line| line.strip_prefix("state: ").map(str::to_string))
            .ok_or_else(|| missing_error.to_string())
            .and_then(|state| Self::parse_playback_state(&state))
    }

    pub fn playback_state(&self) -> Result<PlaybackState, String> {
        self.playback_state_with_missing_error("Status MPD sem estado de reprodução.")
    }

    fn observed_mpd_playback_state(&self) -> Result<PlaybackState, String> {
        self.playback_state_with_missing_error(
            "Status MPD sem estado de reprodução após retomar a faixa.",
        )
    }

    pub fn play(&mut self, is_shared: bool) -> Result<(), String> {
        self.note_eq_intervention();
        self.invalidate_playback_intents();
        self.ensure_startup_queue_ready()?;
        if self.restored_resume.is_some() {
            return self.toggle_play_pause(is_shared);
        }
        if let Some(command) =
            explicit_playback_command(self.playback_state()?, ExplicitPlaybackRequest::Play)
        {
            self.send_command(command)?;
        }
        self.verify_shared_play_resume_progress(is_shared)
    }

    pub fn pause(&mut self) -> Result<(), String> {
        self.note_eq_intervention();
        self.invalidate_playback_intents();
        if !self.queue_coherent { return self.send_command("pause 1").map(|_| ()); }
        self.ensure_startup_queue_ready()?;
        if self.restored_resume.is_some() {
            return Ok(());
        }
        if let Some(command) =
            explicit_playback_command(self.playback_state()?, ExplicitPlaybackRequest::Pause)
        {
            self.send_command(command)?;
        }
        Ok(())
    }

    pub fn toggle_play_pause(&mut self, is_shared: bool) -> Result<(), String> {
        self.note_eq_intervention();
        self.invalidate_playback_intents();
        if !self.queue_coherent {
            return if self.playback_state()? == PlaybackState::Playing {
                self.send_command("pause 1").map(|_| ())
            } else { Err("PLAYBACK_QUEUE_UNCERTAIN".into()) };
        }
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
            match self.observed_mpd_playback_state()? {
                PlaybackState::Playing => Ok(()),
                PlaybackState::Paused => self.send_command("pause 0").map(|_| ()),
                PlaybackState::Stopped => {
                    Err("O MPD parou após a tentativa de retomar a faixa.".to_string())
                }
            }?;
        } else {
            let status = self.get_status()?;
            match status.state.as_str() {
                "play" => return self.send_command("pause 1").map(|_| ()),
                "pause" => self.send_command("pause 0")?,
                _ => self.send_command("play")?,
            };
        }
        self.verify_shared_play_resume_progress(is_shared)
    }

    fn verify_shared_play_resume_progress(&self, is_shared: bool) -> Result<(), String> {
        Self::verify_shared_progress_at(&self.socket_path, is_shared)
    }

    fn verify_shared_progress_at(socket_path: &str, is_shared: bool) -> Result<(), String> {
        if !is_shared {
            return Ok(());
        }
        // Same bounded sampling and single recovery as Shared Next/Previous.
        // Never seek, rebuild the queue or retry the play command here.
        let operation = "Play/Resume";
        std::thread::sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let first =
            Self::parse_progress_status(Self::send_command_to_socket(socket_path, "status")?)?;
        std::thread::sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let second =
            Self::parse_progress_status(Self::send_command_to_socket(socket_path, "status")?)?;
        Self::validate_shared_playing_samples(operation, first, second)?;
        if second.elapsed - first.elapsed >= SHARED_MIN_PROGRESS {
            return Ok(());
        }
        Self::recover_shared_stall(
            operation,
            &mut |command| Self::send_command_to_socket(socket_path, command),
            &mut std::thread::sleep,
        )
    }

    fn change_track_with<F, S>(
        &mut self,
        command: &str,
        is_shared: bool,
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<(), String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        self.ensure_startup_queue_ready()?;
        let pre_state = if is_shared {
            Some(Self::parse_progress_status(send_command("status")?)?.state)
        } else {
            None
        };
        send_command(command)?;
        self.restored_resume = None;
        if pre_state != Some(PlaybackState::Playing) {
            return Ok(());
        }

        sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let first = Self::parse_progress_status(send_command("status")?)?;
        sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let second = Self::parse_progress_status(send_command("status")?)?;
        Self::validate_shared_playing_samples(command, first, second)?;
        if second.elapsed - first.elapsed >= SHARED_MIN_PROGRESS {
            return Ok(());
        }
        Self::recover_shared_stall(command, send_command, sleep)
    }

    pub fn next(&mut self, is_shared: bool) -> Result<(), String> {
        self.note_eq_intervention();
        self.invalidate_playback_intents();
        let socket_path = self.socket_path.clone();
        let gate=self.direct_playback.clone();
        let mut send_command = |command: &str| Self::send_with_gate(&socket_path,gate.as_deref(),command);
        self.change_track_with(
            "next",
            is_shared,
            &mut send_command,
            &mut std::thread::sleep,
        )
    }

    pub fn previous(&mut self, is_shared: bool) -> Result<(), String> {
        self.note_eq_intervention();
        self.invalidate_playback_intents();
        let socket_path = self.socket_path.clone();
        let gate=self.direct_playback.clone();
        let mut send_command = |command: &str| Self::send_with_gate(&socket_path,gate.as_deref(),command);
        self.change_track_with(
            "previous",
            is_shared,
            &mut send_command,
            &mut std::thread::sleep,
        )
    }

    fn validate_shared_playing_samples(
        operation: &str,
        first: PlaybackProgressStatus,
        second: PlaybackProgressStatus,
    ) -> Result<(), String> {
        if first.state == PlaybackState::Stopped || second.state == PlaybackState::Stopped {
            return Err(format!(
                "O MPD parou inesperadamente após {operation} em Shared."
            ));
        }
        if first.state != PlaybackState::Playing || second.state != PlaybackState::Playing {
            return Err(format!(
                "O MPD deixou o estado de reprodução após {operation} em Shared."
            ));
        }
        Ok(())
    }

    fn recover_shared_stall<F, S>(
        operation: &str,
        send_command: &mut F,
        sleep: &mut S,
    ) -> Result<(), String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        send_command("pause 1").map_err(|error| {
            format!("Falha ao pausar para recuperar {operation} em Shared: {error}")
        })?;
        send_command("pause 0").map_err(|error| {
            format!("Falha ao retomar após recuperar {operation} em Shared: {error}")
        })?;
        let recovery_first = Self::parse_progress_status(send_command("status")?)?;
        sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let recovery_second = Self::parse_progress_status(send_command("status")?)?;
        Self::validate_shared_playing_samples(operation, recovery_first, recovery_second)?;
        if recovery_second.elapsed - recovery_first.elapsed < SHARED_MIN_PROGRESS {
            return Err(format!(
                "A saída Shared permaneceu congelada após uma tentativa de pause/resume para {operation}."
            ));
        }
        Ok(())
    }

    fn parse_progress_status(lines: Vec<String>) -> Result<PlaybackProgressStatus, String> {
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
        let elapsed: f64 = elapsed.unwrap_or(0.0);
        if !elapsed.is_finite() || elapsed < 0.0 {
            return Err(
                "Status MPD com elapsed inválido durante verificação de progresso.".to_string(),
            );
        }
        Ok(PlaybackProgressStatus {
            state: state.ok_or_else(|| {
                "Status MPD sem estado durante verificação de progresso.".to_string()
            })?,
            elapsed,
        })
    }

    fn seek_progress(
        lines: &[String],
        expected_state: PlaybackState,
        song_id: Option<u32>,
    ) -> Result<(PlaybackProgressStatus, u32), String> {
        let field = |prefix: &str| {
            let mut values = lines.iter().filter_map(|line| line.strip_prefix(prefix));
            let value = values.next();
            if values.next().is_some() {
                None
            } else {
                value
            }
        };
        if lines.iter().any(|line| line.starts_with("error: ")) {
            return Err("O MPD informou erro durante confirmação do seek Shared.".into());
        }
        if field("state: ").is_none() || field("elapsed: ").is_none() {
            return Err("Status incompleto ou ambíguo durante confirmação do seek Shared.".into());
        }
        let progress = Self::parse_progress_status(lines.to_vec())?;
        if progress.state != expected_state {
            return Err(if progress.state == PlaybackState::Stopped {
                "O MPD parou inesperadamente após o seek Shared."
            } else {
                "O MPD mudou de estado durante confirmação do seek Shared."
            }
            .into());
        }
        let id = field("songid: ")
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or("Identidade da faixa indisponível durante confirmação do seek Shared.")?;
        if song_id.is_some_and(|expected| expected != id) {
            return Err("A faixa mudou durante confirmação do seek Shared.".into());
        }
        Ok((progress, id))
    }

    fn seek_tail_continues<F, S>(
        song_id: u32,
        previous: PlaybackProgressStatus,
        send: &mut F,
        sleep: &mut S,
    ) -> Result<(PlaybackProgressStatus, bool), String>
    where
        F: FnMut(&str) -> Result<Vec<String>, String>,
        S: FnMut(Duration),
    {
        sleep(SEEK_TAIL_DELAY);
        let first = Self::seek_progress(&send("status")?, PlaybackState::Playing, Some(song_id))?.0;
        if first.elapsed < previous.elapsed {
            return Err(
                "A posição mudou inesperadamente durante confirmação do seek Shared.".into(),
            );
        }
        sleep(SEEK_TAIL_DELAY);
        let second =
            Self::seek_progress(&send("status")?, PlaybackState::Playing, Some(song_id))?.0;
        if second.elapsed < first.elapsed {
            return Err(
                "A posição mudou inesperadamente durante confirmação do seek Shared.".into(),
            );
        }
        Ok((
            second,
            second.elapsed - first.elapsed >= SHARED_MIN_PROGRESS,
        ))
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

        let pre = if is_shared {
            let lines = send_command("status")?;
            let state = Self::parse_progress_status(lines.clone())?.state;
            let id = if state == PlaybackState::Playing {
                Some(Self::seek_progress(&lines, PlaybackState::Playing, None)?.1)
            } else {
                None
            };
            Some((state, id))
        } else {
            None
        };
        send_command(&format!("seekcur {:.1}", seconds))?;
        let Some((PlaybackState::Playing, Some(song_id))) = pre else {
            return Ok(());
        };

        const TARGET_TOLERANCE: f64 = 2.0;
        sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let first = Self::seek_progress(
            &send_command("status")?,
            PlaybackState::Playing,
            Some(song_id),
        )?
        .0;
        sleep(SHARED_PROGRESS_SAMPLE_DELAY);
        let second = Self::seek_progress(
            &send_command("status")?,
            PlaybackState::Playing,
            Some(song_id),
        )?
        .0;
        let near_target = (first.elapsed - seconds).abs() <= TARGET_TOLERANCE
            && (second.elapsed - seconds).abs() <= TARGET_TOLERANCE;
        if !near_target || second.elapsed < first.elapsed {
            return Err("A posição não confirmou o seek Shared solicitado.".into());
        }
        let (mut last, continuous) =
            Self::seek_tail_continues(song_id, second, send_command, sleep)?;
        if continuous {
            return Ok(());
        }
        // Use the existing pause/resume exactly once. Validate identity/error
        // and expected state immediately before each command, including pause 0.
        Self::recover_shared_stall(
            "o seek",
            &mut |command: &str| {
                if command.starts_with("pause ") {
                    let state = if command == "pause 1" {
                        PlaybackState::Playing
                    } else {
                        PlaybackState::Paused
                    };
                    let progress =
                        Self::seek_progress(&send_command("status")?, state, Some(song_id))?.0;
                    if progress.elapsed < last.elapsed {
                        return Err("A posição mudou antes de recuperar o seek Shared.".into());
                    }
                    last = progress;
                }
                let lines = send_command(command)?;
                if command == "status" {
                    let progress =
                        Self::seek_progress(&lines, PlaybackState::Playing, Some(song_id))?.0;
                    if progress.elapsed < last.elapsed {
                        return Err("A posição mudou durante recuperação do seek Shared.".into());
                    }
                    last = progress;
                }
                Ok(lines)
            },
            sleep,
        )?;
        if !Self::seek_tail_continues(song_id, last, send_command, sleep)?.1 {
            return Err("A saída Shared permaneceu congelada após uma tentativa de pause/resume para o seek.".into());
        }
        Ok(())
    }

    fn seek_deadline(
        started: Instant,
        shared: bool,
        status: &Result<Vec<String>, String>,
    ) -> Option<Instant> {
        (shared
            && status.as_ref().ok().is_some_and(|lines| {
                Self::parse_progress_status(lines.clone())
                    .is_ok_and(|status| status.state == PlaybackState::Playing)
            }))
        .then_some(started + SHARED_SEEK_BUDGET)
    }

    pub fn seek(&mut self, seconds: f64, is_shared: bool) -> Result<(), String> {
        self.note_eq_intervention();
        let socket_path = self.socket_path.clone();
        let started = Instant::now();
        let deadline = std::cell::Cell::new(None::<Instant>);
        let mut pre_status = true;
        let gate=self.direct_playback.clone();
        let mut send_command = |command: &str| {
            let result = if !is_shared {Self::send_with_gate(&socket_path,gate.as_deref(),command)}else{Self::send_command_to_socket_before(&socket_path, command, deadline.get())};
            // Activate only after observing Shared Playing. Direct, paused,
            // stopped and logical restored seeks retain their original I/O path.
            if is_shared && pre_status && command == "status" {
                pre_status = false;
                deadline.set(Self::seek_deadline(started, is_shared, &result));
            }
            crate::diagnostics::now_playing_seek_command(command, &result);
            result
        };
        self.seek_with(seconds, is_shared, &mut send_command, &mut |delay| {
            std::thread::sleep(
                deadline
                    .get()
                    .map(|end| delay.min(end.saturating_duration_since(Instant::now())))
                    .unwrap_or(delay),
            );
        })
    }

    pub fn get_queue(&self) -> Vec<TrackMetadata> {
        if self.queue_coherent { self.queue.clone() } else { Vec::new() }
    }

    pub fn play_index(&mut self, index: usize, is_shared: bool) -> Result<(), String> {
        self.note_eq_intervention();
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
        self.verify_shared_play_resume_progress(is_shared)
    }

    pub fn clear_queue(&mut self) -> Result<(), String> {
        self.note_eq_intervention();
        self.ensure_queue_structure_available()?;
        if self.send_command("clear").is_err() {
            self.mark_queue_uncertain();
            return Err("PLAYBACK_QUEUE_UNCERTAIN".into());
        }
        self.queue.clear();
        self.restored_resume = None;
        self.startup_restore = StartupQueueRestore::Ready;
        self.queue_coherent = true;
        self.cache_persistence_pending = true;
        self.save_queue_cache().map_err(|_| "PLAYBACK_PERSISTENCE_FAILED".to_string())?;
        self.cache_persistence_pending = false;
        Ok(())
    }

    #[cfg(test)]
    pub fn list_directory(&self, path: &str) -> Result<Vec<LocalItem>, String> {
        Self::list_directory_at(&self.socket_path, path)
    }

    pub(crate) fn list_directory_at(socket_path: &str, path: &str) -> Result<Vec<LocalItem>, String> {
        let cmd = if path.trim().is_empty() {
            "lsinfo".to_string()
        } else {
            format!("lsinfo {}", Self::quote_mpd_argument(path)?)
        };

        let lines = Self::send_command_to_socket(socket_path, &cmd)?;
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
                        current_item = local_media::is_catalog_track_uri(v).then(|| {
                            let name = v.split('/').last().unwrap_or(v).to_string();
                            LocalItem {
                                item_type: "file".to_string(),
                                path: v.to_string(),
                                name,
                                title: None,
                                artist: None,
                                album: None,
                                duration: None,
                            }
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

    pub fn change_playback_option(
        &self,
        action: crate::playback_options::PlaybackOptionAction,
        expected: crate::playback_options::PlaybackOptions,
    ) -> Result<crate::playback_options::PlaybackOptions, String> {
        crate::playback_options::change(action, expected, |command| self.send_command(command))
    }

    pub fn get_status(&self) -> Result<PlaybackStatus, String> {
        self.get_status_with_options(true, true)
    }

    pub(crate) fn get_mpris_status(&self) -> Result<PlaybackStatus, String> {
        self.get_status_with_options(false, false)
    }

    fn get_status_with_options(
        &self,
        include_artwork: bool,
        include_logical_resume: bool,
    ) -> Result<PlaybackStatus, String> {
        self.get_status_with_cover_lookup(include_artwork, include_logical_resume, |uri| {
            self.resolve_cover(uri)
        })
    }

    // Telemetry must never touch collection/NAS paths. Queue artwork is already
    // resolved by the playback preparation path and can be copied without I/O.
    pub(crate) fn get_telemetry_status(&self) -> Result<PlaybackStatus, String> {
        self.get_telemetry_status_with(|command| self.send_command(command))
    }

    fn get_telemetry_status_with(
        &self,
        query: impl Fn(&str) -> Result<Vec<String>, String>,
    ) -> Result<PlaybackStatus, String> {
        self.get_status_with_io(true, true, |_| None, query)
    }

    fn get_status_with_cover_lookup(
        &self,
        include_artwork: bool,
        include_logical_resume: bool,
        cover: impl Fn(&str) -> Option<String>,
    ) -> Result<PlaybackStatus, String> {
        self.get_status_with_io(include_artwork, include_logical_resume, cover, |command| {
            self.send_command(command)
        })
    }

    fn get_status_with_io(
        &self,
        include_artwork: bool,
        include_logical_resume: bool,
        cover: impl Fn(&str) -> Option<String>,
        query: impl Fn(&str) -> Result<Vec<String>, String>,
    ) -> Result<PlaybackStatus, String> {
        let lines = query("status")?;
        let options = crate::playback_options::PlaybackOptions::from_status(&lines);
        let processing = crate::signal_processing::ProcessingStatus::from_status(&lines);
        crate::diagnostics::now_playing_seek_telemetry(&lines);
        let mpd_error = lines.iter().filter_map(|line| line.strip_prefix("error: ")).last();
        crate::diagnostics::playback_mpd_error(mpd_error);
        crate::diagnostics::now_playing_mpd_error(mpd_error);

        let mut state = None;
        let mut elapsed = 0.0;
        let mut duration = 0.0;
        let mut audio_format = String::new();
        let mut song_index: Option<usize> = None;
        let mut song_id: Option<u64> = None;
        let mut volume: i32 = 100;
        let mut is_updating = false;
        let mut playback_error = None;

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
                    "songid" => song_id = Some(Self::parse_mpd_number("songid", v)?),
                    "volume" => volume = Self::parse_mpd_number("volume", v)?,
                    "updating_db" => is_updating = true,
                    "error" => playback_error = Some(PlaybackDiagnostic::from_mpd_error(v)),
                    _ => {}
                }
            }
        }
        let state = state.ok_or_else(|| {
            "Resposta de status do MPD não contém o estado de reprodução.".to_string()
        })?;

        if !self.queue_coherent { playback_error = Some(PlaybackDiagnostic::QueueUncertain); }
        else if self.cache_persistence_pending { playback_error = Some(PlaybackDiagnostic::PersistenceFailed); }
        let queue = if self.queue_coherent { self.queue.as_slice() } else { &[] };

        // Se o MPD estiver parado e sem nenhuma faixa ativa, retorna estado neutro e limpo
        if state == "stop" && song_index.is_none() {
            if include_logical_resume && self.startup_restore == StartupQueueRestore::Ready {
                if let Some(resume) = self.restored_resume.as_ref().filter(|resume| resume.valid_for(self.queue.len())) {
                    let track = &self.queue[resume.queue_index];
                    let local_uri = match &track.media_locator {
                        Some(MediaLocator::Local { uri }) => Some(uri.as_str()),
                        Some(MediaLocator::Plex { .. }) => None,
                        None if !track.uri.starts_with("http://") && !track.uri.starts_with("https://") => Some(track.uri.as_str()),
                        None => None,
                    };
                    return Ok(PlaybackStatus {
                        snapshot_consistent: true,
                        processing: self.queue_coherent.then_some(processing),
                        options,
                        state: "pause".to_string(),
                        elapsed: resume.elapsed,
                        duration: track.duration.unwrap_or(0.0),
                        audio_format,
                        current_media: self.current_media_for_queue_index(Some(resume.queue_index)),
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        album: track.album.clone(),
                        thumb: include_artwork
                            .then(|| {
                                track.thumb.clone().or_else(|| {
                                    local_uri.and_then(&cover)
                                })
                            })
                            .flatten(),
                        plex_image: include_artwork
                            .then(|| track.plex_image.clone())
                            .flatten(),
                        volume: VolumeStatus::from_mpd(volume),
                        is_updating,
                        playback_error,
                    });
                }
            }
            return Ok(PlaybackStatus {
                snapshot_consistent: true,
                processing: self.queue_coherent.then_some(processing),
                options,
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
                playback_error,
            });
        }

        let song_lines = query("currentsong")?;
        let mut current_file = String::new();
        let mut current_song_index = None;
        let mut current_song_id: Option<u64> = None;
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
                    "Id" => current_song_id = Some(Self::parse_mpd_number("Id", v)?),
                    "Pos" => {
                        current_song_index = Some(Self::parse_mpd_number("Pos", v)?);
                    }
                    _ => {}
                }
            }
        }

        let contradicted = song_index.zip(current_song_index).is_some_and(|(a,b)| a != b)
            || song_id.zip(current_song_id).is_some_and(|(a,b)| a != b);
        if contradicted {
            // The individual reads succeeded, but they do not describe one track.
            // Keep controls/options, not stale metadata, cursor or route evidence.
            return Ok(PlaybackStatus {
                snapshot_consistent: false, processing: None, options, state,
                elapsed: 0.0, duration: 0.0, audio_format: String::new(), current_media: None,
                title: String::new(), artist: String::new(), album: String::new(),
                thumb: None, plex_image: None, volume: VolumeStatus::from_mpd(volume),
                is_updating, playback_error,
            });
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
            if let Some(track) = queue.get(idx) {
                title = track.title.clone();
                artist = track.artist.clone();
                album = track.album.clone();
                if include_artwork {
                    thumb = track.thumb.clone();
                    plex_image = track.plex_image.clone();
                }
                if duration <= 0.0 {
                    if let Some(d) = track.duration {
                        duration = d;
                    }
                }
            }
        }

        if (title.is_empty() || thumb.is_none()) && !current_file.is_empty() {
            if let Some(track) = queue.iter().find(|t| t.uri == current_file) {
                if title.is_empty() {
                    title = track.title.clone();
                }
                if artist.is_empty() {
                    artist = track.artist.clone();
                }
                if album.is_empty() {
                    album = track.album.clone();
                }
                if include_artwork && thumb.is_none() {
                    thumb = track.thumb.clone();
                }
                if include_artwork && plex_image.is_none() {
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

        if self.queue_coherent && include_artwork && thumb.is_none() && !current_file.is_empty() {
            thumb = cover(&current_file);
        }

        let current_media = self.current_media_for_queue_index(current_media_queue_index);

        Ok(PlaybackStatus {
            snapshot_consistent: true,
            processing: self.queue_coherent.then_some(processing),
            options,
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
            playback_error,
        })
    }
}

pub struct AudioState(pub Mutex<AudioEngine>, Arc<AtomicU64>);
impl AudioState {
    pub fn new(engine:AudioEngine)->Self {
        let epoch=engine.eq_intervention.clone(); Self(Mutex::new(engine),epoch)
    }
    // The same existing epoch, reachable before waiting for AudioState's mutex.
    pub(crate) fn note_transport_intervention(&self) { self.1.fetch_add(1,Ordering::AcqRel); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    static ALBUM_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn processing_status_follows_existing_telemetry_without_extra_queries() {
        for plex in [false, true] {
            let mut engine = engine_with_track("fixture.flac");
            if plex { engine.queue[0] = plex_track(); }
            let before = serde_json::to_string(&engine.queue).unwrap();
            for state in ["play", "pause", "stop"] {
                for (volume_line, expected) in [("volume: 43", Some(43)), ("volume: 100", Some(100)), ("volume: -1", None), ("", None)] {
                    let commands = std::cell::RefCell::new(Vec::new());
                    let status = engine.get_status_with_io(false, false, |_| None, |command| {
                        commands.borrow_mut().push(command.to_string());
                        match command {
                            "status" => Ok(format!("state: {state}\nsong: 0\naudio: 96000:24:2\n{volume_line}\nxfade: 0\nmixrampdelay: nan").lines().map(str::to_owned).collect()),
                            "currentsong" => Ok(vec!["Pos: 0".into(), "file: fixture.flac".into()]),
                            _ => panic!("unexpected diagnostic/transport command"),
                        }
                    }).unwrap();
                    let processing = status.processing.unwrap();
                    assert_eq!(processing.mpd_volume, expected);
                    assert_eq!(processing.crossfade_seconds, Some(0.0));
                    assert_eq!(processing.replay_gain, crate::signal_processing::ReplayGain::Unknown);
                    assert_eq!(*commands.borrow(), ["status", "currentsong"]);
                    assert_eq!(serde_json::to_string(&engine.queue).unwrap(), before);
                    assert_eq!(status.state, state);
                }
            }
        }
    }

    #[test]
    fn playback_options_status_preserves_stopped_restored_and_local_plex_identity() {
        for plex in [false, true] {
            let mut engine = engine_with_logical_resume();
            engine.restored_resume.as_mut().unwrap().queue_index = 0;
            if plex {
                engine.queue[0] = plex_track();
            }
            let before_queue = serde_json::to_string(&engine.queue).unwrap();
            let before_resume = engine.restored_resume;
            for state in ["stop", "pause", "play"] {
                let query = |command: &str| match command {
                    "status" => {
                        let mut lines = vec![
                            format!("state: {state}"),
                            "repeat: 1".into(),
                            "single: 1".into(),
                            "random: 1".into(),
                        ];
                        if state != "stop" {
                            lines.extend([
                                "song: 0".into(),
                                "elapsed: 9".into(),
                                "duration: 120".into(),
                            ]);
                        }
                        Ok(lines)
                    }
                    "currentsong" => Ok(vec!["Pos: 0".into(), "file: fixture.flac".into()]),
                    _ => panic!("unexpected queue/transport query"),
                };
                let status = engine
                    .get_status_with_io(false, true, |_| None, query)
                    .unwrap();
                let options = status.options.unwrap();
                assert!(options.repeat && options.random);
                assert_eq!(options.single, crate::playback_options::SingleMode::On);
                assert_eq!(
                    status.current_media,
                    engine.current_media_for_queue_index(Some(0))
                );
                if state == "stop" {
                    assert_eq!(status.state, "pause");
                    assert_eq!(status.elapsed, before_resume.unwrap().elapsed);
                    let neutral = engine
                        .get_status_with_io(false, false, |_| None, query)
                        .unwrap();
                    assert_eq!(neutral.state, "stop");
                    assert_eq!(neutral.options, Some(options));
                }
            }
            assert_eq!(serde_json::to_string(&engine.queue).unwrap(), before_queue);
            assert_eq!(engine.restored_resume, before_resume);
        }
    }

    #[test]
    fn toggle_play_pause_characterizes_transport_commands_without_recovery() {
        for (state, command) in [("play", "pause 1"), ("pause", "pause 0"), ("stop", "play")] {
            let mut script = vec![(
                "status",
                match state {
                    "play" => "state: play\nelapsed: 16.0\nOK\n",
                    "pause" => "state: pause\nelapsed: 16.0\nOK\n",
                    _ => "state: stop\nOK\n",
                },
            )];
            if state != "stop" {
                script.push(("currentsong", "OK\n"));
            }
            script.push((command, "OK\n"));
            let (socket, server) = fake_album_mpd(script);
            let mut engine = engine_with_track("local/faixa.flac");
            engine.socket_path = socket;
            let queue = engine.queue.clone();
            engine.toggle_play_pause(false).unwrap();
            assert_eq!(engine.queue, queue);
            server.join().unwrap();
        }
    }

    #[test]
    fn explicit_play_characterizes_idempotence_and_resume_transport() {
        for (state, command) in [
            ("play", None),
            ("pause", Some("pause 0")),
            ("stop", Some("play")),
        ] {
            let response = match state {
                "play" => "state: play\nelapsed: 16.0\nOK\n",
                "pause" => "state: pause\nelapsed: 16.0\nOK\n",
                _ => "state: stop\nOK\n",
            };
            let mut script = vec![("status", response)];
            if let Some(command) = command {
                script.push((command, "OK\n"));
            }
            let (socket, server) = fake_album_mpd(script);
            let mut engine = engine_with_track("local/faixa.flac");
            engine.socket_path = socket;
            engine.play(false).unwrap();
            server.join().unwrap();
        }
    }

    fn play_resume_script(
        script: Vec<(&'static str, &'static str)>,
        track: TrackMetadata,
        explicit_play: bool,
        is_shared: bool,
        cursor: Option<PlaybackResumeSnapshot>,
    ) -> (Result<(), String>, Option<PlaybackResumeSnapshot>) {
        let (socket, server) = fake_album_mpd(script);
        let mut engine = engine_with_track("local/Música com espaços.flac");
        engine.socket_path = socket;
        engine.queue = vec![track];
        engine.restored_resume = cursor;
        let queue = engine.queue.clone();
        let result = if explicit_play {
            engine.play(is_shared)
        } else {
            engine.toggle_play_pause(is_shared)
        };
        assert_eq!(engine.queue, queue);
        server.join().unwrap();
        (result, engine.restored_resume)
    }

    #[test]
    fn shared_play_resume_with_progress_does_not_recover() {
        for (state, explicit, command) in [
            ("pause", false, Some("pause 0")),
            ("stop", false, Some("play")),
            ("pause", true, Some("pause 0")),
            ("stop", true, Some("play")),
            ("play", true, None),
        ] {
            let mut script = vec![(
                "status",
                match state {
                    "pause" => "state: pause\nelapsed: 16.0\nOK\n",
                    "play" => "state: play\nelapsed: 16.0\nOK\n",
                    _ => "state: stop\nOK\n",
                },
            )];
            if !explicit && state != "stop" {
                script.push(("currentsong", "OK\n"));
            }
            if let Some(command) = command {
                script.push((command, "OK\n"));
            }
            script.extend([
                ("status", "state: play\nelapsed: 16.0\nOK\n"),
                ("status", "state: play\nelapsed: 16.09\nOK\n"),
            ]);
            let track = engine_with_track("local/Música com espaços.flac")
                .queue
                .remove(0);
            play_resume_script(script, track, explicit, true, None)
                .0
                .unwrap();
        }
    }

    #[test]
    fn shared_play_resume_stall_recovers_once_without_queue_or_position_commands() {
        for track in [
            engine_with_track("local/Música com espaços.flac")
                .queue
                .remove(0),
            plex_track(),
        ] {
            for explicit in [false, true] {
                let mut script = vec![("status", "state: pause\nelapsed: 16.0\nOK\n")];
                if !explicit {
                    script.push(("currentsong", "OK\n"));
                }
                script.extend([
                    ("pause 0", "OK\n"),
                    ("status", "state: play\nelapsed: 16.0\nOK\n"),
                    ("status", "state: play\nelapsed: 16.0\nOK\n"),
                    ("pause 1", "OK\n"),
                    ("pause 0", "OK\n"),
                    ("status", "state: play\nelapsed: 16.0\nOK\n"),
                    ("status", "state: play\nelapsed: 16.09\nOK\n"),
                ]);
                play_resume_script(script, track.clone(), explicit, true, None)
                    .0
                    .unwrap();
            }
        }
    }

    #[test]
    fn shared_play_resume_restored_cursor_is_selected_and_sought_only_once() {
        for explicit in [false, true] {
            for track in [
                engine_with_track("local/Música com espaços.flac")
                    .queue
                    .remove(0),
                plex_track(),
            ] {
                let cursor = PlaybackResumeSnapshot {
                    queue_index: 0,
                    elapsed: 16.0,
                    state: PlaybackState::Paused,
                };
                let script = vec![
                    ("play 0", "OK\n"),
                    ("seekcur 16.0", "OK\n"),
                    ("status", "state: pause\nelapsed: 16.0\nOK\n"),
                    ("pause 0", "OK\n"),
                    ("status", "state: play\nelapsed: 16.0\nOK\n"),
                    ("status", "state: play\nelapsed: 16.0\nOK\n"),
                    ("pause 1", "OK\n"),
                    ("pause 0", "OK\n"),
                    ("status", "state: play\nelapsed: 16.0\nOK\n"),
                    ("status", "state: play\nelapsed: 16.09\nOK\n"),
                ];
                let (result, remaining) =
                    play_resume_script(script, track, explicit, true, Some(cursor));
                result.unwrap();
                assert!(remaining.is_none());
            }
        }
    }

    #[test]
    fn shared_play_resume_persistent_stall_is_an_error_after_one_recovery() {
        let script = vec![
            ("status", "state: stop\nOK\n"),
            ("play", "OK\n"),
            ("status", "state: play\nelapsed: 16.0\nOK\n"),
            ("status", "state: play\nelapsed: 16.0\nOK\n"),
            ("pause 1", "OK\n"),
            ("pause 0", "OK\n"),
            ("status", "state: play\nelapsed: 16.0\nOK\n"),
            ("status", "state: play\nelapsed: 16.0\nOK\n"),
        ];
        let track = engine_with_track("local/faixa.flac").queue.remove(0);
        let error = play_resume_script(script, track, false, true, None)
            .0
            .unwrap_err();
        assert!(error.contains("permaneceu congelada"));
        assert!(error.contains("Play/Resume"));
    }

    #[test]
    fn shared_play_resume_pause_has_no_progress_poll_or_recovery() {
        let script = vec![
            ("status", "state: play\nelapsed: 16.0\nOK\n"),
            ("currentsong", "OK\n"),
            ("pause 1", "OK\n"),
        ];
        let track = engine_with_track("local/faixa.flac").queue.remove(0);
        play_resume_script(script, track, false, true, None)
            .0
            .unwrap();
    }

    #[test]
    fn shared_play_resume_propagates_mpd_errors_at_each_step() {
        let commands = [
            "status", "play", "status", "status", "pause 1", "pause 0", "status", "status",
        ];
        let replies = [
            "state: stop\nOK\n",
            "OK\n",
            "state: play\nelapsed: 16.0\nOK\n",
            "state: play\nelapsed: 16.0\nOK\n",
            "OK\n",
            "OK\n",
            "state: play\nelapsed: 16.0\nOK\n",
            "state: play\nelapsed: 16.09\nOK\n",
        ];
        for fail_at in 0..commands.len() {
            let script = commands[..=fail_at]
                .iter()
                .enumerate()
                .map(|(index, &command)| {
                    (
                        command,
                        if index == fail_at {
                            "ACK [5@0] {test} simulated failure\n"
                        } else {
                            replies[index]
                        },
                    )
                })
                .collect();
            let track = engine_with_track("local/faixa.flac").queue.remove(0);
            let error = play_resume_script(script, track, true, true, None)
                .0
                .unwrap_err();
            assert!(error.contains("ACK"), "step {fail_at}: {error}");
        }
    }

    #[test]
    fn shared_play_resume_invalid_status_or_changed_state_never_recovers() {
        for response in [
            "state: stop\nelapsed: 16.0\nOK\n",
            "state: pause\nelapsed: 16.0\nOK\n",
            "state: play\nelapsed: NaN\nOK\n",
            "state: play\nelapsed: -1.0\nOK\n",
            "state: invalid\nelapsed: 16.0\nOK\n",
            "elapsed: 16.0\nOK\n",
        ] {
            let mut script = vec![
                ("status", "state: stop\nOK\n"),
                ("play", "OK\n"),
                ("status", response),
            ];
            if response.starts_with("state: stop") || response.starts_with("state: pause") {
                script.push(("status", response));
            }
            let track = engine_with_track("local/faixa.flac").queue.remove(0);
            assert!(play_resume_script(script, track, true, true, None)
                .0
                .is_err());
        }
    }

    #[test]
    fn shared_play_resume_requires_ready_startup_queue() {
        for state in [StartupQueueRestore::Pending, StartupQueueRestore::Failed] {
            let mut engine = engine_with_track("local/faixa.flac");
            engine.startup_restore = state;
            assert!(engine.play(true).unwrap_err().contains("restaurada"));
            assert!(engine
                .toggle_play_pause(true)
                .unwrap_err()
                .contains("restaurada"));
        }
    }

    #[test]
    fn shared_play_resume_transport_failure_during_progress_is_not_success() {
        let (socket, server) = fake_mpd_script(vec![
            ("status".into(), Some("state: stop\nOK\n".into())),
            ("play".into(), Some("OK\n".into())),
            ("status".into(), None),
        ]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        assert!(engine.play(true).unwrap_err().contains("encerrada"));
        server.join().unwrap();
    }

    #[test]
    fn explicit_play_is_idempotent_and_uses_mpd_resume_commands() {
        assert_eq!(
            explicit_playback_command(PlaybackState::Playing, ExplicitPlaybackRequest::Play),
            None
        );
        assert_eq!(
            explicit_playback_command(PlaybackState::Paused, ExplicitPlaybackRequest::Play),
            Some("pause 0")
        );
        assert_eq!(
            explicit_playback_command(PlaybackState::Stopped, ExplicitPlaybackRequest::Play),
            Some("play")
        );
    }

    #[test]
    fn explicit_pause_is_idempotent_and_only_pauses_playing_audio() {
        assert_eq!(
            explicit_playback_command(PlaybackState::Playing, ExplicitPlaybackRequest::Pause),
            Some("pause 1")
        );
        assert_eq!(
            explicit_playback_command(PlaybackState::Paused, ExplicitPlaybackRequest::Pause),
            None
        );
        assert_eq!(
            explicit_playback_command(PlaybackState::Stopped, ExplicitPlaybackRequest::Pause),
            None
        );
    }

    fn seek_status(state: &str, elapsed: f64) -> Result<Vec<String>, String> {
        Ok(vec![
            format!("state: {state}"),
            format!("elapsed: {elapsed}"),
            "songid: 42".into(),
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

    fn scripted_track_change(
        engine: &mut AudioEngine,
        command: &str,
        is_shared: bool,
        responses: Vec<Result<Vec<String>, String>>,
    ) -> (Result<(), String>, Vec<String>, Vec<Duration>) {
        let mut responses = VecDeque::from(responses);
        let mut commands = Vec::new();
        let mut sleeps = Vec::new();
        let result = engine.change_track_with(
            command,
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

    fn assert_summary_has_no_track_material(collection: &LocalAlbumCollection) {
        for album in collection.physical.values().chain(collection.cue.values()) {
            assert_eq!(album.track_uris.len(), 0);
            assert_eq!(album.track_uris.capacity(), 0);
            assert_eq!(album.tracks.len(), 0);
            assert_eq!(album.tracks.capacity(), 0);
        }
    }

    fn assert_summary_matches_full(pages: Vec<Vec<String>>, sources: &HashSet<String>) {
        let mut full = LocalAlbumCollection::default();
        let mut summary = LocalAlbumCollection {
            mode: LocalAlbumCollectionMode::Summary,
            ..LocalAlbumCollection::default()
        };
        for page in pages {
            let full_rows = AudioEngine::collect_local_album_page(&mut full, page.clone()).unwrap();
            assert_eq!(AudioEngine::collect_local_album_page(&mut summary, page).unwrap(), full_rows);
            assert_summary_has_no_track_material(&summary);
        }
        for album in full.physical.values().chain(full.cue.values()) {
            assert_eq!(album.track_uris.len(), album.track_count);
            assert_eq!(album.tracks.len(), album.track_count);
        }
        // Canonicalization may replace physical tracks with CUE, absorb title
        // variants and merge discs; none of those paths may create track data.
        for album in AudioEngine::canonical_local_album_map(summary.clone()).values() {
            assert!(album.track_uris.is_empty());
            assert!(album.tracks.is_empty());
        }
        let full = AudioEngine::finalize_local_album_collection(full, sources);
        let summary = AudioEngine::finalize_local_album_collection(summary, sources);
        // Keep the existing title-only ordering, including its unspecified
        // ordering within equal lowercase titles. Compare every payload by ID.
        let titles = |albums: &[LocalAlbum]| albums.iter()
            .map(|album| album.title.to_lowercase()).collect::<Vec<_>>();
        assert_eq!(titles(&summary), titles(&full));
        let payloads = |albums: Vec<LocalAlbum>| albums.into_iter()
            .map(|album| (album.id.clone(), serde_json::to_value(album).unwrap()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(payloads(summary), payloads(full));
    }

    #[test]
    fn local_album_added_at_validates_optional_protocol_metadata() {
        for value in ["1970-01-01T00:00:00Z", "2000-02-29T23:59:59Z", "2024-02-29T12:00:00Z"] {
            assert_eq!(parse_local_album_added_at(value).as_deref(), Some(value));
        }
        for value in ["", "1969-12-31T23:59:59Z", "1900-02-29T00:00:00Z",
            "2026-02-29T00:00:00Z", "2026-04-31T00:00:00Z", "2026-00-01T00:00:00Z",
            "2026-01-00T00:00:00Z", "2026-01-01T24:00:00Z", "2026-01-01T00:60:00Z",
            "2026-01-01T00:00:60Z", "2026-01-01T00:00:00+00:00", "2026-01-01T00:00:00.000Z",
            "2026-1-1T00:00:00Z", "２０２６-01-01T00:00:00Z"] {
            assert_eq!(parse_local_album_added_at(value), None, "{value}");
        }
    }

    #[test]
    fn local_album_added_at_keeps_first_entry_across_pages_without_leaking_to_other_albums() {
        let pages = vec![album_test_lines(&[
            "file: Music/Album/one.flac", "Album: Album", "Added: 2026-10-30T12:00:00Z",
            "file: Music/Legacy/one.flac", "Album: Legacy",
        ]), album_test_lines(&[
            "file: Music/Album/two.flac", "Album: Album", "Added: 2026-10-29T12:00:00Z",
            "file: Music/Album/three.flac", "Album: Album", "Added: 2026-10-31T12:00:00Z",
            "file: Music/Invalid/one.flac", "Album: Invalid", "Added: 2026-02-30T12:00:00Z",
        ])];
        let sources = HashSet::from(["Music".into()]);
        assert_summary_matches_full(pages.clone(), &sources);
        let albums = AudioEngine::finalize_local_album_collection(collect_album_pages(pages), &sources);
        let added = |title: &str| albums.iter().find(|a| a.title == title).unwrap().added_at.as_deref();
        assert_eq!(added("Album"), Some("2026-10-29T12:00:00Z"));
        assert_eq!(added("Legacy"), None);
        assert_eq!(added("Invalid"), None);
        assert_eq!(albums.iter().find(|a| a.title == "Album").unwrap().track_count, 3);
    }

    #[test]
    fn local_album_added_at_survives_dominant_title_and_disc_consolidation() {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.extend([format!("file: Music/Box/CD1/{i}.flac"), "Album: Box (Disc 1)".into(),
                "Added: 2026-10-30T12:00:00Z".into()]);
        }
        lines.extend(album_test_lines(&[
            "file: Music/Box/CD1/variant.flac", "Album: Typo", "Added: 2026-10-29T12:00:00Z",
            "file: Music/Box/CD2/one.flac", "Album: Box (Disc 2)", "Added: 2026-10-28T12:00:00Z",
        ]));
        let sources = HashSet::from(["Music".into()]);
        assert_summary_matches_full(vec![lines.clone()], &sources);
        let albums = AudioEngine::finalize_local_album_collection(collect_album_pages(vec![lines]), &sources);
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Box");
        assert_eq!(albums[0].added_at.as_deref(), Some("2026-10-28T12:00:00Z"));
        assert_eq!(albums[0].track_count, 6);
        assert_eq!(albums[0].discs.len(), 2);
    }

    #[test]
    fn local_album_added_at_survives_both_physical_and_cue_precedence() {
        let sources = HashSet::from(["Music".into()]);
        for (physical, cue) in [(1, 3), (3, 3)] {
            for physical_older in [true, false] {
                let old = "2026-10-29T12:00:00Z";
                let new = "2026-10-30T12:00:00Z";
                let mut lines = Vec::new();
                for line in cue_fixture_lines("Music/Album", "Album", physical, cue) {
                    let added = line.starts_with("file:").then(|| if line.contains(".cue/") {
                        if physical_older { new } else { old }
                    } else if physical_older { old } else { new });
                    lines.push(line);
                    if let Some(added) = added { lines.push(format!("Added: {added}")); }
                }
                assert_summary_matches_full(vec![lines.clone()], &sources);
                let albums = AudioEngine::finalize_local_album_collection(collect_album_pages(vec![lines]), &sources);
                assert_eq!(albums.len(), 1);
                assert_eq!(albums[0].added_at.as_deref(), Some(old));
                assert_eq!(albums[0].track_count, 3);
                assert_eq!(albums[0].discs[0].folder_path.contains(".cue"), physical == 1);
            }
        }
    }

    #[test]
    fn local_album_summary_matches_full_characterization() {
        let sources = HashSet::from(["Music".into(), "Other".into()]);
        // Reuse the existing CUE characterization generator and boundary cases.
        for (physical, cue) in [(20, 20), (1, 10), (1, 1), (0, 4), (3, 0)] {
            assert_summary_matches_full(vec![
                cue_fixture_lines("Music/Artist/Album", "Album", physical, 0),
                cue_fixture_lines("Music/Artist/Album", "Album", 0, cue),
            ], &sources);
        }
        assert_summary_matches_full(vec![album_test_lines(&[
            "file: Music/Artist/Album/image.flac", "Album: Album", "Genre: Physical",
            "file: Music/Artist/Album/first.cue/track0001", "Album: Album", "Genre: Cue1",
            "file: Music/Artist/Album/second.cue/track0001", "Album: Album", "Genre: Cue2",
            "file: Music/Unknown/one.flac",
            "file: Music/Blank/two.flac", "Album:   ", "Artist: ", "AlbumArtist:   ", "Date:    ", "Genre:   ",
            "file: ../rejected.flac", "Album: Rejected",
        ])], &sources);
        // Exact 80% title/artist thresholds, including missing AlbumArtist and
        // a variant on another page. The same titles on another root stay apart.
        for (main, guest) in [(8, 2), (7, 3), (5, 5), (3, 1)] {
            let mut first = Vec::new();
            for i in 0..main {
                first.extend([format!("file: Music/Box/CD1/{i}.flac"), "Album: Box (Disc 1)".into(),
                    "Artist: Main".into(), "AlbumArtist: Band".into(), "Date: 2001-03-04".into(), "Genre: Jazz".into()]);
            }
            let mut variant = Vec::new();
            for i in 0..guest {
                variant.extend([format!("file: Music/Box/CD1/guest-{i}.flac"), "Album: Typo".into(),
                    "Artist: Guest".into(), "Genre: Rock".into()]);
            }
            assert_summary_matches_full(vec![first, variant, album_test_lines(&[
                "file: Music/Box/CD2/one.flac", "Title: One", "duration: 123.5", "Album: Box (Disc 2)",
                "Artist: Main", "AlbumArtist: Band", "Date: 2002", "Genre: Blues",
                "file: Other/Box/CD1/one.flac", "Album: Box (Disc 1)", "Artist: Other",
                "file: Other/Box/CD2/two.flac", "Album: Box (Disc 2)", "Artist: Other",
                "file: Music/Case/a.flac", "Album: case", "Artist: A", "Artist: Artist", "Genre: Rock", "Genre: rock",
                "file: Music/Case/b.flac", "Album: Case", "Artist: Artist",
            ])], &sources);
        }
        // CUE plus a physical sibling disc, tags and presence across pages.
        assert_summary_matches_full(vec![
            cue_fixture_lines("Music/Multi/CD1", "Multi", 1, 0),
            cue_fixture_lines("Music/Multi/CD1", "Multi", 0, 4),
            album_test_lines(&["file: Music/Multi/CD2/one.flac", "Album: Multi", "Artist: A", "Date: 2024", "Genre: Folk"]),
        ], &sources);
        assert_summary_matches_full(Vec::new(), &sources);
    }

    #[test]
    fn local_album_summary_large_catalog_retains_no_track_material() {
        const ALBUMS: usize = 2_000;
        const TRACKS_PER_ALBUM: usize = 50;
        const TOTAL: usize = ALBUMS * TRACKS_PER_ALBUM;
        let mut full = LocalAlbumCollection::default();
        let mut summary = LocalAlbumCollection { mode: LocalAlbumCollectionMode::Summary, ..LocalAlbumCollection::default() };
        // Generate at most one existing-size MPD page at a time, only in RAM.
        for start in (0..TOTAL).step_by(LOCAL_ALBUM_PAGE_SIZE) {
            let mut page = Vec::new();
            for i in start..(start + LOCAL_ALBUM_PAGE_SIZE).min(TOTAL) {
                page.extend([format!("file: Synthetic/Album-{}/{i}.flac", i / TRACKS_PER_ALBUM),
                    format!("Album: Album-{}", i / TRACKS_PER_ALBUM), "Title: Track".into(),
                    "Artist: Artist".into(), "AlbumArtist: Artist".into(), "Date: 2001".into(), "duration: 123.5".into()]);
            }
            let count = AudioEngine::collect_local_album_page(&mut full, page.clone()).unwrap();
            assert_eq!(AudioEngine::collect_local_album_page(&mut summary, page).unwrap(), count);
            assert_summary_has_no_track_material(&summary);
        }
        let retained = |collection: &LocalAlbumCollection| collection.physical.values().chain(collection.cue.values())
            .fold((0, 0, 0), |(uris, items, tracks), album| (uris + album.track_uris.len(), items + album.tracks.len(), tracks + album.track_count));
        assert_eq!(retained(&full), (TOTAL, TOTAL, TOTAL));
        assert_eq!(retained(&summary), (0, 0, TOTAL));
        assert_eq!(summary.physical.len(), ALBUMS);
        let sources = HashSet::from(["Synthetic".into()]);
        let full = AudioEngine::finalize_local_album_collection(full, &sources);
        let summary = AudioEngine::finalize_local_album_collection(summary, &sources);
        assert_eq!(serde_json::to_value(summary).unwrap(), serde_json::to_value(full).unwrap());
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
    fn local_album_collection_counts_raw_files_but_keeps_only_catalog_tracks() {
        let lines = album_test_lines(&[
            "file: Music/Artist/First/01.flac",
            "Album: First",
            "Artist: First Artist",
            "file: Music/Artist/Rejected/program.prg",
            "Album: Rejected",
            "Artist: Rejected Artist",
            "Date: 1999",
            "file: Music/Artist/Artwork/cover.jpg",
            "Album: Artwork",
            "file: Music/Artist/Second/02.opus",
            "Album: Second",
            "Artist: Second Artist",
            "file: Music/Artist/Cue/image.cue/track0001",
            "Album: Cue",
            "Artist: Cue Artist",
        ]);
        let mut collection = LocalAlbumCollection::default();

        let raw_count = AudioEngine::collect_local_album_page(&mut collection, lines).unwrap();

        assert_eq!(raw_count, 5);
        assert_eq!(collection.physical.len(), 2);
        assert_eq!(collection.cue.len(), 1);
        assert_eq!(
            collection.physical[&album_id("Music/Artist/First", "First")].track_uris,
            vec!["Music/Artist/First/01.flac"]
        );
        assert_eq!(
            collection.physical[&album_id("Music/Artist/Second", "Second")].track_uris,
            vec!["Music/Artist/Second/02.opus"]
        );
        assert_eq!(
            collection.cue[&album_id("Music/Artist/Cue", "Cue")].track_uris,
            vec!["Music/Artist/Cue/image.cue/track0001"]
        );
        assert!(!collection
            .physical
            .keys()
            .any(|key| key.contains("rejected") || key.contains("artwork")));
    }

    #[test]
    fn list_directory_keeps_directories_and_only_emits_catalog_tracks() {
        let response = concat!(
            "directory: Music/Álbuns Com Espaço\n",
            "file: Music/01 válida.flac\n",
            "Title: Válida\n",
            "file: Music/program.prg\n",
            "Title: Rejeitada\n",
            "Artist: Artista Rejeitado\n",
            "file: Music/disc.iso\n",
            "Album: ISO Rejeitado\n",
            "file: Music/cover.WeBp\n",
            "Title: Artwork Rejeitado\n",
            "directory: Music/image.cue\n",
            "file: Music/image.cue/track0001\n",
            "Title: Faixa CUE\n",
            "file: Music/Único Espaço.MP3\n",
            "Artist: Artista Válido\n",
            "OK\n",
        );
        let (socket, server) = fake_album_mpd(vec![("lsinfo \"Music\"", response)]);
        let engine = AudioEngine::new(&socket, "");

        let items = engine.list_directory("Music").unwrap();

        server.join().unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| (item.item_type.as_str(), item.path.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("directory", "Music/Álbuns Com Espaço"),
                ("file", "Music/01 válida.flac"),
                ("directory", "Music/image.cue"),
                ("file", "Music/image.cue/track0001"),
                ("file", "Music/Único Espaço.MP3"),
            ]
        );
        assert_eq!(items[1].title.as_deref(), Some("Válida"));
        assert_eq!(items[3].title.as_deref(), Some("Faixa CUE"));
        assert_eq!(items[4].artist.as_deref(), Some("Artista Válido"));
        assert!(items.iter().all(|item| {
            item.title.as_deref() != Some("Rejeitada")
                && item.artist.as_deref() != Some("Artista Rejeitado")
                && item.album.as_deref() != Some("ISO Rejeitado")
                && item.title.as_deref() != Some("Artwork Rejeitado")
        }));
    }

    #[test]
    fn list_directory_at_preserves_instance_listing_and_errors() {
        let response = concat!(
            "directory: Music/Álbuns\n",
            "file: Music/one.flac\nTitle: One\nArtist: Artist\nAlbum: Album\nTime: 1:02\nduration: 99\n",
            "file: Music/cover.jpg\nTitle: Ignore\n",
            "file: Music/image.CUE/track0001\nTitle: Virtual\nduration: 12.5\nOK\n",
        );
        let path = "Music/Álbum com \"aspas\"";
        let command = "lsinfo \"Music/Álbum com \\\"aspas\\\"\"";
        let (socket, server) = fake_album_mpd(vec![(command, response), (command, response)]);
        let engine = AudioEngine::new(&socket, "");
        let old = engine.list_directory(path).unwrap();
        let detached = AudioEngine::list_directory_at(&socket, path).unwrap();
        assert_eq!(serde_json::to_value(&old).unwrap(), serde_json::to_value(&detached).unwrap());
        assert_eq!(detached.len(), 3);
        assert_eq!(detached[1].duration, Some(62.0));
        assert_eq!(detached[2].duration, Some(12.5));
        server.join().unwrap();

        let ack = "ACK [50@0] {lsinfo} directory not found\n";
        let (socket, server) = fake_album_mpd(vec![("lsinfo", ack), ("lsinfo", ack)]);
        let engine = AudioEngine::new(&socket, "");
        assert_eq!(engine.list_directory(" ").unwrap_err(), AudioEngine::list_directory_at(&socket, " ").unwrap_err());
        server.join().unwrap();
        for invalid in ["folder\nkill", "folder\rkill", "folder\0kill"] {
            assert_eq!(engine.list_directory(invalid).unwrap_err(), AudioEngine::list_directory_at(&socket, invalid).unwrap_err());
        }
        assert_eq!(engine.list_directory("Music").unwrap_err(), AudioEngine::list_directory_at(&socket, "Music").unwrap_err());
    }

    #[test]
    fn physical_tracks_win_over_a_duplicate_cue_album() {
        let collection = collect_album_pages(vec![cue_fixture_lines(
            "FLAC/Donovan/Album", "Donovan Album", 20, 20,
        )]);
        let membership = AudioEngine::canonical_local_album_map(collection.clone());
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
        assert_eq!(
            membership[&album_id("FLAC/Donovan/Album", "Donovan Album")]
                .track_uris,
            (1..=20)
                .map(|index| format!("FLAC/Donovan/Album/{index:02}.flac"))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn single_image_cue_wins_without_becoming_a_one_track_album() {
        let collection = collect_album_pages(vec![cue_fixture_lines(
            "Music/Artist/Album", "Album", 1, 10,
        )]);
        let membership = AudioEngine::canonical_local_album_map(collection.clone());
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
        assert_eq!(
            membership[&album_id("Music/Artist/Album", "Album")].track_uris,
            (1..=10)
                .map(|index| format!("Music/Artist/Album/image.cue/track{index:04}"))
                .collect::<Vec<_>>()
        );
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
        assert_eq!(
            album.track_uris,
            (1..=10)
                .map(|index| format!("Music/Artist/Album/image.cue/track{index:04}"))
                .collect::<Vec<_>>()
        );
        assert!(!album
            .track_uris
            .contains(&"Music/Artist/Album/01.flac".to_string()));
        assert_eq!(
            album.track_uris.iter().collect::<HashSet<_>>().len(),
            album.track_uris.len()
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
    fn album_membership_separates_logical_albums_in_the_same_folder() {
        let mut map = AudioEngine::canonical_local_album_map(collect_album_pages(vec![
            album_test_lines(&[
                "file: Music/Shared/first-a.flac",
                "Title: First A",
                "Album: First",
                "duration: 61.5",
                "file: Music/Shared/second.flac",
                "Title: Second",
                "Album: Second",
                "file: Music/Shared/first-b.flac",
                "Title: First B",
                "Album: First",
            ]),
        ]));

        assert_eq!(
            map[&album_id("Music/Shared", "First")].track_uris,
            vec![
                "Music/Shared/first-a.flac".to_string(),
                "Music/Shared/first-b.flac".to_string(),
            ]
        );
        assert_eq!(
            map[&album_id("Music/Shared", "Second")].track_uris,
            vec!["Music/Shared/second.flac".to_string()]
        );

        let first = map
            .remove(&album_id("Music/Shared", "First"))
            .unwrap()
            .into_disc_tracks();
        let second = map
            .remove(&album_id("Music/Shared", "Second"))
            .unwrap()
            .into_disc_tracks();
        assert_eq!(first.len(), 1);
        assert_eq!(
            first[0]
                .tracks
                .iter()
                .map(|track| track.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Music/Shared/first-a.flac", "Music/Shared/first-b.flac"]
        );
        assert_eq!(first[0].tracks[0].title.as_deref(), Some("First A"));
        assert_eq!(first[0].tracks[0].duration, Some(61.5));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].tracks[0].path, "Music/Shared/second.flac");
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
        assert_eq!(
            album.track_uris,
            vec![
                "Artist/Album/CD1/track.flac".to_string(),
                "Artist/Album/CD1/track2.flac".to_string(),
                "Artist/Album/CD2/track.flac".to_string(),
            ]
        );
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
        let mut expected = (0..500)
            .map(|index| format!("Artist/Album/CD1/{index}.flac"))
            .collect::<Vec<_>>();
        expected.push("Artist/Album/CD2/one.flac".to_string());
        assert_eq!(album.track_uris, expected);
        assert_eq!(
            album.track_uris.iter().collect::<HashSet<_>>().len(),
            album.track_uris.len()
        );
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

    #[test]
    fn source_info_album_limit_precedes_find_and_uses_canonical_identity() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base 'Music/Shared')\"", "songs: 33\nOK\n"),
        ]);
        assert!(AudioEngine::get_source_album_tracks_at(&path, &album_id("Music/Shared", "First")).is_err());
        server.join().unwrap();
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base 'Music/Shared')\"", "songs: 3\nOK\n"),
            ("find \"(base 'Music/Shared')\" window 0:3", "file: Music/Shared/a.flac\nAlbum: First\nfile: Music/Shared/b.flac\nAlbum: Other\nfile: Music/Shared/c.flac\nAlbum: First\nOK\n"),
        ]);
        let groups = AudioEngine::get_source_album_tracks_at(&path, &album_id("Music/Shared", "First")).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(groups.iter().map(|g| g.tracks.len()).sum::<usize>(), 2);
        assert!(groups.iter().flat_map(|g| &g.tracks).all(|t| t.album.as_deref() == Some("First")));
    }

    #[test]
    fn local_album_track_query_returns_only_the_selected_canonical_album() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base 'Music/Shared')\"", "songs: 3\nOK\n"),
            (
                "find \"(base 'Music/Shared')\" window 0:3",
                "file: Music/Shared/first-a.flac\nTitle: First A\nAlbum: First\nduration: 61.5\nfile: Music/Shared/other.flac\nTitle: Other\nAlbum: Other\nfile: Music/Shared/first-b.flac\nTitle: First B\nAlbum: First\nOK\n",
            ),
        ]);

        let groups = AudioEngine::get_local_album_tracks_at(
            &path,
            &album_id("Music/Shared", "First"),
        )
        .unwrap()
        .unwrap();

        server.join().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0]
                .tracks
                .iter()
                .map(|track| track.path.as_str())
                .collect::<Vec<_>>(),
            vec!["Music/Shared/first-a.flac", "Music/Shared/first-b.flac"]
        );
        assert_eq!(groups[0].tracks[0].title.as_deref(), Some("First A"));
        assert_eq!(groups[0].tracks[0].duration, Some(61.5));
    }

    #[test]
    fn manual_check_tracks_use_one_database_response_and_only_previously_confirmed_files() {
        let (path, server) = fake_album_mpd(vec![(
            "find \"(base 'Music/Shared')\"",
            "file: Music/Shared/first.flac\nTitle: First\nAlbum: First\nduration: 61.5\nfile: Music/Shared/new.flac\nTitle: New\nAlbum: First\nfile: Music/Shared/other.flac\nAlbum: Other\nOK\n",
        )]);
        let known = HashSet::from(["Music/Shared/first.flac".to_owned(), "Music/Shared/other.flac".to_owned()]);
        let groups = AudioEngine::get_checked_album_tracks_at(&path, &album_id("Music/Shared", "First"), &known).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tracks.len(), 1);
        assert_eq!(groups[0].tracks[0].path, "Music/Shared/first.flac");
        assert_eq!(groups[0].tracks[0].duration, Some(61.5));
    }

    #[test]
    fn manual_check_tracks_keep_multidisc_cue_and_reject_unconfirmed_only_albums() {
        let (path, server) = fake_album_mpd(vec![(
            "find \"(base 'Artist/Album')\"",
            "file: Artist/Album/CD2/disc.cue/track0001\nTitle: Two\nAlbum: Album CD2\nDisc: 2\nfile: Artist/Album/CD1/one.flac\nTitle: One\nAlbum: Album CD1\nDisc: 1\nOK\n",
        )]);
        let known = HashSet::from(["Artist/Album/CD2/disc.cue/track0001".to_owned(), "Artist/Album/CD1/one.flac".to_owned()]);
        let groups = AudioEngine::get_checked_album_tracks_at(&path, &album_id("Artist/Album", "Album"), &known).unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(groups.len(), 2);
        let (baseline_path, baseline_server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base 'Artist/Album')\"", "songs: 2\nOK\n"),
            ("find \"(base 'Artist/Album')\" window 0:2", "file: Artist/Album/CD2/disc.cue/track0001\nTitle: Two\nAlbum: Album CD2\nDisc: 2\nfile: Artist/Album/CD1/one.flac\nTitle: One\nAlbum: Album CD1\nDisc: 1\nOK\n"),
        ]);
        let baseline = AudioEngine::get_local_album_tracks_at(&baseline_path, &album_id("Artist/Album", "Album")).unwrap().unwrap();
        baseline_server.join().unwrap();
        assert_eq!(serde_json::to_value(&groups).unwrap(), serde_json::to_value(&baseline).unwrap());
        assert_eq!(groups.iter().flat_map(|g| g.tracks.iter().map(|t| t.path.clone())).collect::<HashSet<_>>(), known);
        let (path, server) = fake_album_mpd(vec![("find \"(base 'Music/New')\"", "file: Music/New/new.flac\nAlbum: New\nOK\n")]);
        assert!(AudioEngine::get_checked_album_tracks_at(&path, &album_id("Music/New", "New"), &known).unwrap().is_none());
        server.join().unwrap();
    }

    #[test]
    fn local_album_track_query_scopes_multidisc_grouping_to_the_album_root() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base 'Artist/Album')\"", "songs: 2\nOK\n"),
            (
                "find \"(base 'Artist/Album')\" window 0:2",
                "file: Artist/Album/CD2/two.flac\nTitle: Two\nAlbum: Album CD2\nfile: Artist/Album/CD1/one.flac\nTitle: One\nAlbum: Album CD1\nOK\n",
            ),
        ]);

        let groups = AudioEngine::get_local_album_tracks_at(
            &path,
            &album_id("Artist/Album", "Album"),
        )
        .unwrap()
        .unwrap();

        server.join().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].disc.number, 1);
        assert_eq!(groups[0].tracks[0].path, "Artist/Album/CD1/one.flac");
        assert_eq!(groups[1].disc.number, 2);
        assert_eq!(groups[1].tracks[0].path, "Artist/Album/CD2/two.flac");
    }

    #[test]
    fn local_album_track_query_preserves_cue_membership_within_its_scope() {
        let (path, server) = fake_album_mpd(vec![
            ("status", "state: stop\nOK\n"),
            ("count \"(base 'Artist/Album')\"", "songs: 3\nOK\n"),
            (
                "find \"(base 'Artist/Album')\" window 0:3",
                "file: Artist/Album/image.flac\nAlbum: Album\nfile: Artist/Album/image.cue/track0001\nTitle: One\nAlbum: Album\nfile: Artist/Album/image.cue/track0002\nTitle: Two\nAlbum: Album\nOK\n",
            ),
        ]);

        let groups = AudioEngine::get_local_album_tracks_at(
            &path,
            &album_id("Artist/Album", "Album"),
        )
        .unwrap()
        .unwrap();

        server.join().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].disc.folder_path, "Artist/Album/image.cue");
        assert_eq!(
            groups[0]
                .tracks
                .iter()
                .map(|track| track.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Artist/Album/image.cue/track0001",
                "Artist/Album/image.cue/track0002",
            ]
        );
    }

    #[test]
    fn local_album_scope_validates_ids_and_escapes_mpd_filter_arguments() {
        let folder = "Music/O'Brien \"Mix\"\\Back";
        let id = album_id(folder, "Album");
        assert_eq!(AudioEngine::local_album_scope(&id).unwrap(), folder);
        assert_eq!(
            AudioEngine::local_album_base_filter(folder).unwrap(),
            "\"(base 'Music/O\\\\'Brien \\\"Mix\\\"\\\\\\\\Back')\""
        );

        for invalid in [
            "999:Music:album".to_string(),
            "05:Music:album".to_string(),
            album_id("../Music", "Album"),
            album_id("Music/Bad\nPath", "Album"),
        ] {
            assert!(AudioEngine::local_album_scope(&invalid).is_err());
        }
    }

    #[test]
    fn owned_direct_volume_socket_accepts_only_the_expected_process() {
        let (path, server) = fake_album_mpd(vec![("setvol 0", "OK\n")]);
        AudioEngine::send_owned_command_to_socket(&path, std::process::id(), "setvol 0").unwrap();
        server.join().unwrap();
        let id=ALBUM_TEST_ID.fetch_add(1,Ordering::Relaxed);
        let path=std::env::temp_dir().join(format!("sonante-owned-volume-{}-{id}.sock",std::process::id()));
        let listener=UnixListener::bind(&path).unwrap();
        let server=thread::spawn(move || {let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();let mut bytes=[0u8;256];assert_eq!(std::io::Read::read(&mut stream,&mut bytes).unwrap(),0,"foreign server must receive no command");});
        assert_eq!(AudioEngine::send_owned_command_to_socket(path.to_str().unwrap(),std::process::id()+1,"setvol 100"),Err(crate::direct_volume::STALE.into()));
        server.join().unwrap();std::fs::remove_file(path).unwrap();
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

    fn fake_short_mpd(
        responses: Vec<(&'static str, Option<&'static str>)>,
    ) -> (String, thread::JoinHandle<()>) {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("/tmp/sm{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            for (expected, response) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.23.5\n").unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut command)
                    .unwrap();
                assert_eq!(command, format!("{expected}\n"));
                if let Some(response) = response {
                    stream.write_all(response.as_bytes()).unwrap();
                }
            }
            std::fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    fn fake_slow_mpd_response(
        expected: &'static str,
        delay: Duration,
    ) -> (String, thread::JoinHandle<()>) {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("/tmp/smt{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.23.5\n").unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command, format!("{expected}\n"));
            thread::sleep(delay);
            std::fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    pub(super) fn fake_mpd_script(
        responses: Vec<(String, Option<String>)>,
    ) -> (String, thread::JoinHandle<()>) {
        let id = ALBUM_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!("/tmp/smp{}-{id}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            for (expected, response) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.23.5\n").unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut actual = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    assert!(!line.is_empty(), "comando MPD simulado terminou antes do esperado");
                    actual.push_str(&line);
                    if !actual.starts_with("command_list_begin\n")
                        || line == "command_list_end\n"
                    {
                        break;
                    }
                }
                assert_eq!(actual.trim_end(), expected);
                if let Some(response) = response {
                    stream.write_all(response.as_bytes()).unwrap();
                }
            }
            std::fs::remove_file(path).unwrap();
        });
        (socket_path, server)
    }

    fn queue_status_response(
        playlist_version: u64,
        playlist_length: usize,
        state: PlaybackState,
        current_index: Option<usize>,
        elapsed: Option<f64>,
    ) -> String {
        let state = match state {
            PlaybackState::Stopped => "stop",
            PlaybackState::Paused => "pause",
            PlaybackState::Playing => "play",
        };
        let mut response = format!(
            "playlist: {playlist_version}\nplaylistlength: {playlist_length}\nstate: {state}\n"
        );
        if let Some(index) = current_index {
            response.push_str(&format!("song: {index}\n"));
        }
        if let Some(elapsed) = elapsed {
            response.push_str(&format!("elapsed: {elapsed}\n"));
        }
        response.push_str("OK\n");
        response
    }

    fn playlistinfo_response(uris: &[&str]) -> String {
        let mut response = String::new();
        for uri in uris {
            response.push_str(&format!("file: {uri}\nTitle: ignored metadata\n"));
        }
        response.push_str("OK\n");
        response
    }

    #[test]
    fn rollback_snapshot_capture_is_read_only_and_uses_real_mpd_uris() {
        let before = queue_status_response(
            7,
            2,
            PlaybackState::Playing,
            Some(1),
            Some(12.0),
        );
        let after = queue_status_response(
            7,
            2,
            PlaybackState::Playing,
            Some(1),
            Some(12.5),
        );
        let uris = [
            "Music/Álbum/01 faixa.flac",
            "https://plex.invalid/stream?id=2",
        ];
        let (socket, server) = fake_mpd_script(vec![
            ("status".to_string(), Some(before)),
            (
                "playlistinfo".to_string(),
                Some(playlistinfo_response(&uris)),
            ),
            ("status".to_string(), Some(after)),
        ]);

        let snapshot = AudioEngine::capture_mpd_playback_snapshot(&socket).unwrap();

        assert_eq!(snapshot.uris, uris.map(str::to_string));
        assert_eq!(snapshot.current_index, Some(1));
        assert_eq!(snapshot.elapsed, 12.5);
        assert_eq!(snapshot.state, PlaybackState::Playing);
        assert_eq!(snapshot.playlist_version, 7);
        server.join().unwrap();
    }

    #[test]
    fn rollback_snapshot_capture_supports_empty_stopped_paused_and_playing() {
        let cases = [
            (PlaybackState::Stopped, Vec::<&str>::new(), None, None),
            (
                PlaybackState::Stopped,
                vec!["one.flac", "two.flac"],
                Some(1),
                Some(3.0),
            ),
            (
                PlaybackState::Paused,
                vec!["one.flac"],
                Some(0),
                Some(4.5),
            ),
            (
                PlaybackState::Playing,
                vec!["one.flac"],
                Some(0),
                Some(5.5),
            ),
        ];
        for (state, uris, current_index, elapsed) in cases {
            let status = queue_status_response(11, uris.len(), state, current_index, elapsed);
            let (socket, server) = fake_mpd_script(vec![
                ("status".to_string(), Some(status.clone())),
                (
                    "playlistinfo".to_string(),
                    Some(playlistinfo_response(&uris)),
                ),
                ("status".to_string(), Some(status)),
            ]);

            let snapshot = AudioEngine::capture_mpd_playback_snapshot(&socket).unwrap();

            assert_eq!(snapshot.state, state);
            assert_eq!(snapshot.uris, uris.iter().map(|uri| uri.to_string()).collect::<Vec<_>>());
            assert_eq!(snapshot.current_index, current_index);
            assert_eq!(snapshot.elapsed, elapsed.unwrap_or(0.0));
            server.join().unwrap();
        }
    }

    #[test]
    fn rollback_snapshot_capture_rejects_concurrent_playlist_change() {
        let (socket, server) = fake_mpd_script(vec![
            (
                "status".to_string(),
                Some(queue_status_response(
                    20,
                    1,
                    PlaybackState::Paused,
                    Some(0),
                    Some(8.0),
                )),
            ),
            (
                "playlistinfo".to_string(),
                Some(playlistinfo_response(&["one.flac"])),
            ),
            (
                "status".to_string(),
                Some(queue_status_response(
                    21,
                    1,
                    PlaybackState::Paused,
                    Some(0),
                    Some(8.0),
                )),
            ),
        ]);

        let error = AudioEngine::capture_mpd_playback_snapshot(&socket)
            .err()
            .unwrap();

        assert!(error.contains("mudou durante a captura"));
        server.join().unwrap();
    }

    #[test]
    fn rollback_snapshot_restore_rebuilds_queue_position_and_paused_state() {
        for state in [PlaybackState::Paused, PlaybackState::Playing] {
            let snapshot = MpdPlaybackSnapshot {
                uris: vec![
                    "Music/uma faixa.flac".to_string(),
                    "Music/duas \\\"aspas\\\".flac".to_string(),
                ],
                current_index: Some(1),
                elapsed: 42.567,
                state,
                playlist_version: 30,
            };
            let (socket, server) = fake_mpd_script(vec![
                (
                    "command_list_begin\nclear\nadd \"Music/uma faixa.flac\"\nadd \"Music/duas \\\\\\\"aspas\\\\\\\".flac\"\nplay 1\npause 1\ncommand_list_end".to_string(),
                    Some("OK\n".to_string()),
                ),
                ("seekcur 42.567".to_string(), Some("OK\n".to_string())),
            ]);

            AudioEngine::restore_mpd_playback_snapshot(&socket, &snapshot).unwrap();
            server.join().unwrap();
        }

        let stopped = MpdPlaybackSnapshot {
            uris: vec!["one.flac".to_string()],
            current_index: Some(0),
            elapsed: 9.0,
            state: PlaybackState::Stopped,
            playlist_version: 31,
        };
        let (socket, server) = fake_mpd_script(vec![(
            "command_list_begin\nclear\nadd \"one.flac\"\ncommand_list_end".to_string(),
            Some("OK\n".to_string()),
        )]);
        AudioEngine::restore_mpd_playback_snapshot(&socket, &stopped).unwrap();
        server.join().unwrap();

        let empty = MpdPlaybackSnapshot {
            uris: Vec::new(),
            current_index: None,
            elapsed: 0.0,
            state: PlaybackState::Stopped,
            playlist_version: 32,
        };
        let (socket, server) = fake_mpd_script(vec![(
            "command_list_begin\nclear\ncommand_list_end".to_string(),
            Some("OK\n".to_string()),
        )]);
        AudioEngine::restore_mpd_playback_snapshot(&socket, &empty).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn rollback_snapshot_seek_failure_happens_after_paused_batch() {
        let snapshot = MpdPlaybackSnapshot {
            uris: vec!["Music/track.flac".to_string()],
            current_index: Some(0),
            elapsed: 12.345,
            state: PlaybackState::Playing,
            playlist_version: 33,
        };
        let (socket, server) = fake_mpd_script(vec![
            (
                "command_list_begin\nclear\nadd \"Music/track.flac\"\nplay 0\npause 1\ncommand_list_end"
                    .to_string(),
                Some("OK\n".to_string()),
            ),
            (
                "seekcur 12.345".to_string(),
                Some("ACK [50@0] {seekcur} failure\n".to_string()),
            ),
        ]);

        let error = AudioEngine::restore_mpd_playback_snapshot(&socket, &snapshot).unwrap_err();
        assert!(error.contains("ACK [50@0] {seekcur}"));
        server.join().unwrap();
    }

    #[test]
    fn rollback_snapshot_capture_propagates_ack_eof_and_transport_failures() {
        let (socket, server) = fake_mpd_script(vec![(
            "status".to_string(),
            Some("ACK [5@0] {status} failed\n".to_string()),
        )]);
        assert!(AudioEngine::capture_mpd_playback_snapshot(&socket)
            .err()
            .unwrap()
            .starts_with("ACK"));
        server.join().unwrap();

        let (socket, server) = fake_mpd_script(vec![("status".to_string(), None)]);
        assert!(AudioEngine::capture_mpd_playback_snapshot(&socket)
            .err()
            .unwrap()
            .contains("encerrada"));
        server.join().unwrap();

        let missing = format!("/tmp/smp{}-capture-missing.sock", std::process::id());
        assert!(AudioEngine::capture_mpd_playback_snapshot(&missing)
            .err()
            .unwrap()
            .contains("conectar"));
    }

    #[test]
    fn rollback_snapshot_restore_reports_stale_ack_eof_and_transport_failures() {
        let snapshot = MpdPlaybackSnapshot {
            uris: vec!["stale.flac".to_string()],
            current_index: None,
            elapsed: 0.0,
            state: PlaybackState::Stopped,
            playlist_version: 40,
        };
        let command = "command_list_begin\nclear\nadd \"stale.flac\"\ncommand_list_end";

        let (socket, server) = fake_mpd_script(vec![(
            command.to_string(),
            Some("ACK [50@2] {add} No such song\n".to_string()),
        )]);
        let error = AudioEngine::restore_mpd_playback_snapshot(&socket, &snapshot).unwrap_err();
        assert!(error.contains("ACK [50@2] {add}"));
        server.join().unwrap();

        let (socket, server) = fake_mpd_script(vec![(command.to_string(), None)]);
        assert!(AudioEngine::restore_mpd_playback_snapshot(&socket, &snapshot)
            .unwrap_err()
            .contains("encerrada"));
        server.join().unwrap();

        let missing = format!("/tmp/smp{}-restore-missing.sock", std::process::id());
        assert!(AudioEngine::restore_mpd_playback_snapshot(&missing, &snapshot)
            .unwrap_err()
            .contains("conectar"));
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

    fn get_test_local_cover(
        socket_path: &str,
        fixture: &Path,
        path: &str,
    ) -> Result<Option<String>, String> {
        let source_id = Path::new(path)
            .components()
            .next()
            .and_then(|component| match component {
                std::path::Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .expect("test local path has a source ID");
        let physical_root = fixture.join(source_id);
        let library = fixture.join("library");
        std::fs::create_dir_all(&physical_root).unwrap();
        std::fs::create_dir_all(&library).unwrap();
        let source_link = library.join(source_id);
        if std::fs::symlink_metadata(&source_link).is_err() {
            std::os::unix::fs::symlink(&physical_root, &source_link).unwrap();
        }
        AudioEngine::get_local_cover_at(
            socket_path,
            &library,
            path,
            &[physical_root.to_string_lossy().into_owned()],
        )
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
        let cover = get_test_local_cover("/nonexistent/mpd.socket", &dir, "Music").unwrap();
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
    fn managed_local_cover_uses_configured_root_and_supports_webp_sidecar() {
        let dir = cover_test_dir();
        let root = dir.join("Music");
        let album = root.join("Album");
        let library = dir.join("library");
        let staging = dir.join("library.next");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir(&library).unwrap();
        std::fs::write(album.join("cover.webp"), b"webp cover").unwrap();
        crate::local_media::mirror::build_filtered_staging(
            &staging,
            &library,
            std::slice::from_ref(&root),
            "cover-managed",
        )
        .unwrap();
        std::fs::remove_dir(&library).unwrap();
        std::fs::rename(staging, &library).unwrap();
        let folders = vec![root.to_string_lossy().into_owned()];

        assert_eq!(
            AudioEngine::get_local_cover_at(
                "/nonexistent/mpd.socket",
                &library,
                "Music/Album",
                &folders,
            )
            .unwrap(),
            Some(format!(
                "data:image/webp;base64,{}",
                BASE64.encode(b"webp cover")
            ))
        );
        assert!(AudioEngine::get_local_cover_at(
            "/nonexistent/mpd.socket",
            &library,
            &album.to_string_lossy(),
            &folders,
        )
        .is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn conventional_jpeg_and_png_sidecars_remain_supported() {
        for (name, mime) in [("cover.jpeg", "image/jpeg"), ("cover.png", "image/png")] {
            let dir = cover_test_dir();
            std::fs::write(dir.join(name), b"cover").unwrap();
            assert_eq!(
                find_folder_cover_path(&dir),
                Some(format!("data:{mime};base64,{}", BASE64.encode(b"cover")))
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn local_cover_read_accepts_exact_limit() {
        let bytes = vec![7; MAX_LOCAL_COVER_BYTES];
        let result = read_limited_local_cover(bytes.as_slice(), bytes.len() as u64).unwrap();
        assert_eq!(result, bytes);
    }

    #[test]
    fn local_cover_read_rejects_oversized_metadata_without_reading() {
        struct MustNotRead;
        impl Read for MustNotRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("Oversized cover must be rejected before reading");
            }
        }
        assert!(read_limited_local_cover(MustNotRead, MAX_LOCAL_COVER_BYTES as u64 + 1).is_none());
    }

    #[test]
    fn local_cover_read_bounds_growth_after_metadata() {
        let dir = cover_test_dir();
        let path = dir.join("cover.jpg");
        std::fs::write(&path, b"small").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let size_hint = file.metadata().unwrap().len();
        // Deterministically grow the same opened file after capturing its metadata.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_LOCAL_COVER_BYTES as u64 + 4096)
            .unwrap();
        let mut file = file;
        assert!(read_limited_local_cover(&mut file, size_hint).is_none());
        assert_eq!(
            std::io::Seek::stream_position(&mut file).unwrap(),
            MAX_LOCAL_COVER_BYTES as u64 + 1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn local_cover_read_accepts_shrink_after_metadata_and_rejects_read_error() {
        assert_eq!(
            read_limited_local_cover(&b"small"[..], 1024).unwrap(),
            b"small"
        );
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("simulated cover read failure"))
            }
        }
        assert!(read_limited_local_cover(FailedRead, 5).is_none());
    }

    #[test]
    fn local_cover_skips_oversized_or_invalid_candidates_for_valid_fallback() {
        let dir = cover_test_dir();
        std::fs::File::create(dir.join("cover.jpg"))
            .unwrap()
            .set_len(MAX_LOCAL_COVER_BYTES as u64 + 1)
            .unwrap();
        std::fs::create_dir(dir.join("cover.jpeg")).unwrap();
        std::fs::write(dir.join("cover.png"), b"fallback cover").unwrap();
        assert_eq!(
            find_folder_cover_path(&dir),
            Some(format!(
                "data:image/png;base64,{}", BASE64.encode(b"fallback cover")
            ))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn generic_cover_resolution_preserves_plex_local_mount_absolute_paths() {
        let dir = cover_test_dir();
        let album = dir.join("Plex Album");
        std::fs::create_dir(&album).unwrap();
        std::fs::write(album.join("cover.jpg"), b"plex cover").unwrap();
        let engine = AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: Vec::new(),
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
        };

        assert_eq!(
            engine.resolve_cover(&album.to_string_lossy()),
            Some(format!(
                "data:image/jpeg;base64,{}",
                BASE64.encode(b"plex cover")
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
            let cover = get_test_local_cover(&socket, &dir, "Music").unwrap();
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
            assert!(get_test_local_cover(&socket, &dir, "Music").is_err());
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
        assert!(get_test_local_cover(&socket, &dir, folder)
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
            get_test_local_cover(&socket, &dir, "Music").unwrap(),
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
                queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
        };
        assert!(AudioEngine::update_library(&audio.socket_path).is_err());
        server.join().unwrap();
    }

    #[test]
    fn targeted_library_update_quotes_only_the_relative_mpd_uri_and_returns_job_id() {
        let uri = "Música/Álbum com espaço/faixa \\\"mix\\\".flac";
        let (socket, server) = fake_album_mpd(vec![(
            "update \"Música/Álbum com espaço/faixa \\\\\\\"mix\\\\\\\".flac\"",
            "updating_db: 42\nOK\n",
        )]);

        assert_eq!(AudioEngine::update_library_path(&socket, uri), Ok(42));
        server.join().unwrap();
    }

    #[test]
    fn targeted_library_update_rejects_non_relative_or_empty_scope_without_connecting() {
        for uri in [
            "",
            "/mnt/music/track.flac",
            "../music/track.flac",
            "Music/../track.flac",
            "https://example.invalid/track.flac",
            "HTTP:remote-track",
            "Music/X-Plex-Token=secret.flac",
            "Music/track.flac\nrescan",
        ] {
            assert_eq!(
                AudioEngine::update_library_path("/socket/must-not-be-used", uri),
                Err(TargetedLibraryUpdateErrorCode::InvalidUri),
                "{uri:?}"
            );
        }
    }

    #[test]
    fn update_job_parser_accepts_one_positive_id_and_rejects_invalid_responses() {
        assert_eq!(
            AudioEngine::parse_mpd_update_job_id(&["updating_db: 42".to_string()]),
            Ok(42)
        );
        assert_eq!(
            AudioEngine::parse_mpd_update_job_id(&[]),
            Err(MpdJobIdParseError::MissingJob)
        );
        assert_eq!(
            AudioEngine::parse_mpd_update_job_id(&[
                "updating_db: 1".to_string(),
                "updating_db: 2".to_string(),
            ]),
            Err(MpdJobIdParseError::DuplicateJob)
        );
        for value in ["", "0", "-1", "1.0", "abc", "18446744073709551616"] {
            assert_eq!(
                AudioEngine::parse_mpd_update_job_id(&[format!("updating_db: {value}")]),
                Err(MpdJobIdParseError::InvalidJob),
                "{value:?}"
            );
        }
        assert_eq!(
            AudioEngine::parse_mpd_update_job_id(&[
                "updating_db: 1".to_string(),
                "state: stop".to_string(),
            ]),
            Err(MpdJobIdParseError::ContradictoryResponse)
        );
        assert_eq!(
            AudioEngine::parse_mpd_update_job_id(&[
                "ACK [5@0] {update} failure".to_string(),
            ]),
            Err(MpdJobIdParseError::Ack)
        );
    }

    #[test]
    fn rescan_job_request_returns_real_job_id_and_rejects_invalid_responses() {
        let (socket, server) =
            fake_short_mpd(vec![("rescan", Some("updating_db: 73\nOK\n"))]);
        assert_eq!(AudioEngine::request_mpd_rescan_job(&socket), Ok(73));
        server.join().unwrap();

        for response in ["OK\n", "updating_db: invalid\nOK\n"] {
            let (socket, server) = fake_short_mpd(vec![("rescan", Some(response))]);
            assert!(AudioEngine::request_mpd_rescan_job(&socket).is_err());
            server.join().unwrap();
        }

        let (socket, server) = fake_short_mpd(vec![(
            "rescan",
            Some("ACK [5@0] {rescan} update failed\n"),
        )]);
        let error = AudioEngine::request_mpd_rescan_job(&socket).unwrap_err();
        assert!(error.starts_with("ACK [5@0] {rescan}"));
        server.join().unwrap();

        let (socket, server) = fake_short_mpd(vec![("rescan", None)]);
        let error = AudioEngine::request_mpd_rescan_job(&socket).unwrap_err();
        assert!(error.contains("encerrada antes da resposta final"));
        server.join().unwrap();

        let (socket, server) =
            fake_slow_mpd_response("rescan", Duration::from_millis(650));
        let error = AudioEngine::request_mpd_rescan_job(&socket).unwrap_err();
        assert!(error.contains("Falha ao ler resposta do MPD"));
        server.join().unwrap();
    }

    #[test]
    fn managed_update_remains_pending_until_completion_or_shutdown() {
        let (socket, server) = fake_short_mpd(vec![
            ("status", Some("updating_db: 42\nstate: play\nOK\n")),
            ("status", Some("updating_db: 42\nstate: play\nOK\n")),
            ("status", Some("state: play\nOK\n")),
        ]);
        assert_eq!(
            AudioEngine::wait_for_managed_mpd_update(
                &socket,
                Some(42),
                Duration::from_millis(10),
                || false,
            ),
            Ok(MpdRefreshCompletion::Completed)
        );
        server.join().unwrap();

        let (socket, server) =
            fake_short_mpd(vec![("status", Some("updating_db: 42\nstate: play\nOK\n"))]);
        let mut polls = 0;
        assert_eq!(
            AudioEngine::wait_for_managed_mpd_update(
                &socket,
                Some(42),
                Duration::from_millis(10),
                || {
                    polls += 1;
                    polls > 1
                },
            ),
            Err(())
        );
        server.join().unwrap();
    }

    #[test]
    fn managed_update_preserves_transport_protocol_and_unavailable_failures() {
        for (response, protocol) in [
            (None, false),
            (Some("updating_db: bad\nOK\n"), true),
            (Some("ACK [5@0] {status} refused\n"), true),
        ] {
            let (socket, server) = fake_short_mpd(vec![("status", response)]);
            let result = AudioEngine::wait_for_managed_mpd_update(
                &socket,
                Some(42),
                Duration::from_millis(10),
                || false,
            )
            .unwrap();
            assert!(if protocol {
                matches!(result, MpdRefreshCompletion::ProtocolError(_))
            } else {
                matches!(result, MpdRefreshCompletion::TransportError(_))
            });
            server.join().unwrap();
        }
        let missing = format!("/tmp/sm{}-managed-missing.sock", std::process::id());
        assert!(matches!(
            AudioEngine::wait_for_managed_mpd_update(
                &missing,
                Some(42),
                Duration::from_millis(10),
                || false,
            ),
            Ok(MpdRefreshCompletion::Unavailable(_))
        ));
    }

    #[test]
    fn bounded_update_wait_observes_active_job_then_completion() {
        let (socket, server) = fake_short_mpd(vec![
            ("status", Some("updating_db: 42\nstate: stop\nOK\n")),
            ("status", Some("state: stop\nOK\n")),
        ]);
        assert_eq!(
            AudioEngine::wait_for_mpd_update_job(
                &socket,
                42,
                Duration::from_millis(100),
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::Completed
        );
        server.join().unwrap();

        let (socket, server) = fake_short_mpd(vec![(
            "status",
            Some("state: play\nvolume: 50\nsong: 3\nOK\n"),
        )]);
        assert_eq!(
            AudioEngine::wait_for_mpd_update_job(
                &socket,
                42,
                Duration::from_millis(100),
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::Completed
        );
        server.join().unwrap();
    }

    #[test]
    fn bounded_idle_wait_observes_every_active_update_job() {
        let (socket, server) = fake_short_mpd(vec![
            ("status", Some("updating_db: 41\nstate: stop\nOK\n")),
            ("status", Some("updating_db: 42\nstate: stop\nOK\n")),
            ("status", Some("state: stop\nOK\n")),
        ]);
        assert_eq!(
            AudioEngine::wait_for_mpd_idle(
                &socket,
                Duration::from_millis(100),
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::Completed
        );
        server.join().unwrap();

        let (socket, server) = fake_short_mpd(vec![(
            "status",
            Some("updating_db: 43\nstate: stop\nOK\n"),
        )]);
        assert_eq!(
            AudioEngine::wait_for_mpd_idle(
                &socket,
                Duration::ZERO,
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::Timeout
        );
        server.join().unwrap();
    }

    #[test]
    fn bounded_update_wait_distinguishes_timeout_transport_protocol_and_unavailable() {
        let (socket, server) =
            fake_short_mpd(vec![("status", Some("updating_db: 42\nOK\n"))]);
        assert_eq!(
            AudioEngine::wait_for_mpd_update_job(
                &socket,
                42,
                Duration::ZERO,
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::Timeout
        );
        server.join().unwrap();

        let (socket, server) = fake_short_mpd(vec![("status", None)]);
        assert!(matches!(
            AudioEngine::wait_for_mpd_update_job(
                &socket,
                42,
                Duration::from_millis(100),
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::TransportError(_)
        ));
        server.join().unwrap();

        for response in [
            "ACK [5@0] {status} failure\n",
            "updating_db:\nOK\n",
            "updating_db:42\nOK\n",
            "updating_db: \nOK\n",
            "updating_db: -1\nOK\n",
            "updating_db: 0\nOK\n",
            "updating_db: abc\nOK\n",
            "updating_db: 1.0\nOK\n",
            "updating_db: 18446744073709551616\nOK\n",
            "updating_db: 1\nupdating_db: 2\nOK\n",
            "updating_db: 1\nupdating_db:broken\nOK\n",
        ] {
            let (socket, server) = fake_short_mpd(vec![("status", Some(response))]);
            assert!(matches!(
                AudioEngine::wait_for_mpd_update_job(
                    &socket,
                    42,
                    Duration::from_millis(100),
                    Duration::from_millis(1),
                ),
                MpdRefreshCompletion::ProtocolError(_)
            ));
            server.join().unwrap();
        }

        let missing = format!("/tmp/sm{}-missing.sock", std::process::id());
        assert!(matches!(
            AudioEngine::wait_for_mpd_update_job(
                &missing,
                42,
                Duration::from_millis(100),
                Duration::from_millis(1),
            ),
            MpdRefreshCompletion::Unavailable(_)
        ));
    }

    #[test]
    fn raw_catalog_audit_reports_only_invalid_song_uris_with_bounded_samples() {
        let clean = AudioEngine::audit_raw_local_catalog(&[
            "file: Music/Album/one.flac".to_string(),
            "file: Music/Album/two.dsf".to_string(),
            "file: Music/Album/image.cue/track0001".to_string(),
        ])
        .unwrap();
        assert!(clean.is_clean());
        assert_eq!(clean.total_files, 3);

        let invalid = [
            "Music/Album/program.prg",
            "Music/Album/disc.iso",
            "Music/Album/cover.jpg",
            "Music/Album/image.cue",
            "Music/Album/unknown.bin",
            "Music//Album/track.flac",
            "/Music/Album/track.flac",
            "Music/../Album/track.flac",
            "Music/Album/extra-1.txt",
            "Music/Album/extra-2.txt",
        ];
        let lines = invalid
            .iter()
            .map(|uri| format!("file: {uri}"))
            .collect::<Vec<_>>();
        let audit = AudioEngine::audit_raw_local_catalog(&lines).unwrap();
        assert!(!audit.is_clean());
        assert_eq!(audit.total_files, invalid.len());
        assert_eq!(audit.invalid_count, invalid.len());
        assert_eq!(
            audit.invalid_uris,
            [
                "Music/Album/program.prg",
                "Music/Album/disc.iso",
                "Music/Album/cover.jpg",
                "Music/Album/image.cue",
                "Music/Album/unknown.bin",
                "<invalid local URI>",
                "<invalid local URI>",
                "<invalid local URI>",
            ]
        );
        assert!(AudioEngine::audit_raw_local_catalog(&["file:".to_string()]).is_err());
        assert!(AudioEngine::audit_raw_local_catalog(&[
            "ACK [5@0] {find} failure".to_string(),
        ])
        .is_err());
    }

    #[test]
    fn raw_catalog_audit_can_read_catalog_from_fake_mpd() {
        let (socket, server) = fake_short_mpd(vec![(
            "listall",
            Some("file: Music/Album/track.flac\nOK\n"),
        )]);
        let audit = AudioEngine::audit_mpd_local_catalog_at(&socket).unwrap();
        assert!(audit.is_clean());
        assert_eq!(audit.total_files, 1);
        server.join().unwrap();
    }

    #[test]
    fn targeted_library_update_distinguishes_ack_from_unavailable_mpd() {
        let (socket, server) = fake_album_mpd(vec![(
            "update \"Music/track.flac\"",
            "ACK [5@0] {update} failure\n",
        )]);
        assert_eq!(
            AudioEngine::update_library_path(&socket, "Music/track.flac"),
            Err(TargetedLibraryUpdateErrorCode::UpdateCommandFailed)
        );
        server.join().unwrap();

        assert_eq!(
            AudioEngine::update_library_path(
                "/tmp/sonante-targeted-update-does-not-exist.sock",
                "Music/track.flac"
            ),
            Err(TargetedLibraryUpdateErrorCode::MpdUnavailable)
        );
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
    fn local_album_genres_preserve_cue_discs_identity_and_unknown_metadata() {
        let lines = album_test_lines(&[
            "file: A/Multi/CD1/image.flac",
            "Album: Same",
            "AlbumArtist: Artist",
            "Genre: Rock",
            "file: A/Multi/CD1/image.cue/track0001",
            "Album: Same",
            "AlbumArtist: Artist",
            "Genre: Jazz",
            "file: A/Multi/CD1/image.cue/track0002",
            "Album: Same",
            "AlbumArtist: Artist",
            "Genre: rock",
            "file: A/Multi/CD2/one.flac",
            "Album: Same",
            "AlbumArtist: Artist",
            "Genre: Blues",
            "Genre: Jazz",
            "file: B/Multi/one.flac",
            "Album: Same",
            "AlbumArtist: Artist",
            "Genre: Reggae",
            "file: A/Unknown/one.flac",
            "Genre: ",
            "Genre: bad\nvalue",
        ]);
        let sources = ["A".to_string(), "B".to_string()].into_iter().collect();
        let albums = AudioEngine::finalize_local_album_collection(
            collect_album_pages(vec![lines.clone()]),
            &sources,
        );
        let baseline = AudioEngine::finalize_local_album_collection(
            collect_album_pages(vec![lines
                .into_iter()
                .filter(|line| !line.starts_with("Genre: "))
                .collect()]),
            &sources,
        );
        assert_eq!(albums.len(), baseline.len());
        for album in &albums {
            let previous = baseline
                .iter()
                .find(|previous| previous.id == album.id)
                .unwrap();
            assert_eq!(album.track_count, previous.track_count);
            assert_eq!(album.folder_path, previous.folder_path);
            assert_eq!(
                serde_json::to_value(&album.discs).unwrap(),
                serde_json::to_value(&previous.discs).unwrap()
            );
        }
        let multi = albums
            .iter()
            .find(|album| album.source_id.as_deref() == Some("A") && album.title == "Same")
            .unwrap();
        assert_eq!(multi.track_count, 3);
        assert_eq!(multi.discs.len(), 2);
        assert!(multi
            .discs
            .iter()
            .any(|disc| disc.folder_path.ends_with("image.cue")));
        let genres: HashSet<_> = multi
            .genres
            .iter()
            .map(|genre| genre.to_lowercase())
            .collect();
        assert_eq!(
            genres,
            ["rock", "jazz", "blues"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        let other = albums
            .iter()
            .find(|album| album.source_id.as_deref() == Some("B"))
            .unwrap();
        assert_ne!(multi.id, other.id);
        assert_eq!(other.genres, ["Reggae"]);
        let unknown = albums
            .iter()
            .find(|album| album.folder_path == "A/Unknown")
            .unwrap();
        assert!(unknown.genres.is_empty());
        let mut legacy = serde_json::to_value(multi).unwrap();
        legacy.as_object_mut().unwrap().remove("genres");
        let restored: LocalAlbum = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored.id, multi.id);
        assert!(restored.genres.is_empty());
    }

    #[test]
    fn local_album_genres_union_across_pages_and_title_variants() {
        let first = album_test_lines(&[
            "file: A/Album/1.flac",
            "Album: Album",
            "Genre: Folk",
            "file: A/Album/2.flac",
            "Album: Album",
            "Genre: Jazz",
            "file: A/Album/3.flac",
            "Album: Album",
            "file: A/Album/4.flac",
            "Album: Album",
        ]);
        let second = album_test_lines(&["file: A/Album/5.flac", "Album: Typo", "Genre: Rock"]);
        let albums = AudioEngine::finalize_local_album_collection(
            collect_album_pages(vec![first, second]),
            &HashSet::new(),
        );
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 5);
        assert_eq!(
            albums[0].genres.iter().cloned().collect::<HashSet<_>>(),
            ["Folk", "Jazz", "Rock"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
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
    fn dominant_title_merge_keeps_the_merged_folder_membership_only() {
        let tracks = [
            ("Music/Album/main-1.flac", Some("Canonical"), None, None),
            ("Music/Album/main-2.flac", Some("Canonical"), None, None),
            ("Music/Album/main-3.flac", Some("Canonical"), None, None),
            ("Music/Album/main-4.flac", Some("Canonical"), None, None),
            ("Music/Album/main-5.flac", Some("Canonical"), None, None),
            ("Music/Album/main-6.flac", Some("Canonical"), None, None),
            ("Music/Album/main-7.flac", Some("Canonical"), None, None),
            ("Music/Album/main-8.flac", Some("Canonical"), None, None),
            ("Music/Album/variant-a.flac", Some("Variant"), None, None),
            ("Music/Album/variant-b.flac", Some("Variant"), None, None),
            ("Music/Other/unrelated.flac", Some("Other"), None, None),
        ];
        let map = resolved_album_tracks(&tracks);
        let membership = &map[&album_id("Music/Album", "Canonical")].track_uris;

        assert_eq!(membership.len(), 10);
        assert!(membership.iter().all(|uri| uri.starts_with("Music/Album/")));
        assert!(!membership.contains(&"Music/Other/unrelated.flac".to_string()));
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

    #[test]
    fn addition_completion_refuses_partial_queue_or_unpersisted_cache() {
        let mut audio = engine_with_track("old/confirmed.flac");
        audio.ensure_queue_persisted().unwrap();
        audio.cache_persistence_pending = true;
        assert_eq!(audio.ensure_queue_persisted().unwrap_err(), "PLAYBACK_PERSISTENCE_FAILED");
        audio.cache_persistence_pending = false;
        audio.mark_queue_uncertain();
        assert_eq!(audio.ensure_queue_persisted().unwrap_err(), "PLAYBACK_QUEUE_UNCERTAIN");
    }

    #[test]
    fn addition_retirement_preserves_inflight_queue_and_current_cache() {
        use crate::initial_plex_playback::{Gate, check_effect};
        let gate = Arc::new(Gate::default());
        gate.publish("addition-test".into(), "unused".into(), std::process::id()).unwrap();
        let session = gate.current().unwrap().unwrap();
        let (started, ready) = std::sync::mpsc::channel();
        let (resume, continuing) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let lease = session.enter().unwrap();
            let mut audio = engine_with_track("old/previous.flac");
            let mut track = engine_with_track("old/current.flac").queue.remove(0);
            track.thumb = Some("data:image/png;base64,AA==".into());
            let mut cache = Vec::new();
            audio.play_tracks_with_io(vec![track], vec!["old/current.flac".into()], 0, |_| {
                started.send(()).unwrap(); continuing.recv().unwrap();
                check_effect()?; Ok(Vec::new())
            }, |audio| { cache = AudioEngine::encode_queue_cache(&audio.queue)?; Ok(()) }).unwrap();
            audio.ensure_queue_persisted().unwrap();
            session.confirm_queue(vec!["old/current.flac".into()]).unwrap();
            drop(lease);
            (audio.queue, cache)
        });
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        let retiring = gate.clone();
        let (done, retired) = std::sync::mpsc::channel();
        let closer = std::thread::spawn(move || { retiring.retire().unwrap(); done.send(()).unwrap(); });
        assert!(retired.recv_timeout(Duration::from_millis(20)).is_err());
        resume.send(()).unwrap();
        let (queue, cache) = worker.join().unwrap();
        retired.recv_timeout(Duration::from_secs(1)).unwrap(); closer.join().unwrap();
        assert_eq!(queue[0].uri, "old/current.flac");
        let cached = AudioEngine::decode_queue_cache(cache.as_slice()).unwrap().0;
        assert_eq!(cached[0].uri, queue[0].uri);
        assert!(!cache.windows("old/previous.flac".len()).any(|value| value == b"old/previous.flac"));
        assert!(gate.current().unwrap().is_none());
    }

    #[test]
    fn manual_check_real_direct_guard_allows_queue_start_and_cache_commit() {
        use crate::{direct_playback, direct_volume};
        let transactions = crate::ConfigTransactionState::new();
        let operations = transactions.operations.clone();
        let check = operations.manual_check_playback.clone();
        let gate = direct_playback::Gate::new(operations.clone());
        let mut volume = 100;
        let mut state = "stop";
        let mut commands = Vec::new();
        let mut io = |command: &str| -> Result<Vec<String>, String> {
            commands.push(command.to_owned());
            for line in command.lines() {
                if line == "setvol 0" { volume = 0; }
                if line == "setvol 100" { volume = 100; }
                if line.starts_with("play ") { state = "play"; }
            }
            Ok(if command.lines().any(|line| line == "status") { vec![format!("volume: {volume}"), format!("state: {state}"), "playlist: 1".into()] } else { vec![] })
        };
        let volume_session = direct_volume::Session::initialize(&mut io).unwrap();
        let mut config = crate::config::AppConfig::default();
        config.first_run = false;
        config.audio_output_type = "alsa".into();
        config.alsa_device = "hw:CARD=Fixture,DEV=0".into();
        config.direct_volume_choice = config.direct_context();
        gate.commit_choice(&config);
        gate.bind(&config, 17, Some(volume_session));
        let owner = operations.begin().unwrap();
        let session = check.begin(&owner, &operations, &["file: old/current.flac".into()].into()).unwrap();
        let audio = Mutex::new(engine_with_track("old/previous.flac"));
        let mut track = engine_with_track("old/current.flac").queue.remove(0);
        track.thumb = Some("data:image/png;base64,AA==".into());
        let mut cache = Vec::new();
        let token = crate::admit_or_validate_playback(&audio, &transactions, None).unwrap();
        let result = crate::finish_playback_intent(&audio, &transactions, &token, |audio|
            audio.play_tracks_with_io(vec![track], vec!["old/current.flac".into()], 0,
                |command| gate.send_with(command, |pid, cmd| { assert_eq!(pid, Some(17)); io(cmd) }),
                |audio| { cache = AudioEngine::encode_queue_cache(&audio.queue)?; Ok(()) }));
        assert_eq!(result, Ok(()), "real Direct command admission must accept its own explicit Check");
        audio.lock().unwrap().ensure_queue_persisted().unwrap();
        assert_eq!(AudioEngine::decode_queue_cache(cache.as_slice()).unwrap().0[0].uri, "old/current.flac");
        assert!(commands.iter().any(|c| c.contains("play 0")));
        assert_eq!(volume, 100);
        assert_eq!(state, "play");
        session.confirm();
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
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

    fn engine_with_logical_resume() -> AudioEngine {
        let mut engine = engine_with_two_tracks();
        engine.restored_resume = Some(PlaybackResumeSnapshot {
            queue_index: 1,
            elapsed: 17.25,
            state: PlaybackState::Paused,
        });
        engine
    }

    fn logical_recovery_script(
        uris: &[String],
        state: PlaybackState,
        index: Option<usize>,
        elapsed: f64,
    ) -> Vec<(String, Option<String>)> {
        let status = queue_status_response(34, uris.len(), state, index, Some(elapsed));
        let mut playlist = String::new();
        for (pos, uri) in uris.iter().enumerate() {
            playlist.push_str(&format!("file: {uri}\nPos: {pos}\nId: {}\n", pos + 171));
        }
        playlist.push_str("OK\n");
        let mut script = Vec::new();
        for _ in 0..2 {
            script.extend([
                ("status".into(), Some(status.clone())),
                ("playlistinfo".into(), Some(playlist.clone())),
                ("status".into(), Some(status.clone())),
            ]);
        }
        script.push(("status".into(), Some(status)));
        script
    }

    #[test]
    fn reserved_logical_resume_capture_is_atomic_and_blocks_concurrent_player_commands() {
        let audio = Arc::new(Mutex::new(engine_with_logical_resume()));
        let (owner, captured) = {
            let mut engine = audio.lock().unwrap();
            let owner = engine.reserve_queue_structure().unwrap();
            let captured = engine.capture_reserved_logical_queue(&owner).unwrap();
            assert!(engine.reserve_queue_structure().is_none());
            assert_eq!(captured.tracks, engine.get_queue());
            assert_eq!(Some(captured.resume), engine.restored_resume);
            // Owner can re-read the frozen snapshot under AudioState.
            assert_eq!(
                engine
                    .capture_reserved_logical_queue(&owner)
                    .unwrap()
                    .resume,
                captured.resume
            );
            (owner, captured)
        };
        let concurrent = Arc::clone(&audio);
        thread::spawn(move || {
            let mut engine = concurrent.lock().unwrap();
            // Invalid socket proves rejection happens before MPD/cache I/O.
            let prefix = "LOCAL_SOURCE_REMOVAL_LOGICAL_RESUME_RESERVED";
            assert!(engine.play(false).unwrap_err().starts_with(prefix));
            assert!(engine.pause().unwrap_err().starts_with(prefix));
            assert!(engine
                .toggle_play_pause(false)
                .unwrap_err()
                .starts_with(prefix));
            assert!(engine.play_index(0, false).unwrap_err().starts_with(prefix));
            assert!(engine.next(false).unwrap_err().starts_with(prefix));
            assert!(engine.previous(false).unwrap_err().starts_with(prefix));
            assert!(engine.seek(4.0, false).unwrap_err().starts_with(prefix));
            assert!(engine
                .persist_shutdown_resume()
                .unwrap_err()
                .starts_with(prefix));
            assert!(engine
                .prepare_device_switch()
                .unwrap_err()
                .cause
                .starts_with(prefix));
            assert_eq!(Some(captured.resume), engine.restored_resume);
            assert_eq!(captured.tracks, engine.get_queue());
        })
        .join()
        .unwrap();
        let mut engine = audio.lock().unwrap();
        drop(owner); // RAII release does not acquire AudioState.
        engine.ensure_startup_queue_ready().unwrap();
        let owner = engine.reserve_queue_structure().unwrap();
        engine.capture_reserved_logical_queue(&owner).unwrap();
    }

    #[test]
    fn reserved_logical_resume_invalid_cursor_or_foreign_owner_never_arms_cursor_fence() {
        for fault in 0..6 {
            let mut engine = engine_with_logical_resume();
            match fault {
                0 => engine.restored_resume = None,
                1 => engine.restored_resume.as_mut().unwrap().queue_index = 2,
                2 => engine.restored_resume.as_mut().unwrap().elapsed = -1.0,
                3 => engine.restored_resume.as_mut().unwrap().elapsed = f64::NAN,
                4 => engine.restored_resume.as_mut().unwrap().elapsed = f64::INFINITY,
                _ => engine.restored_resume.as_mut().unwrap().state = PlaybackState::Stopped,
            }
            let owner = engine.reserve_queue_structure().unwrap();
            assert!(engine.capture_reserved_logical_queue(&owner).is_err());
            engine.ensure_logical_resume_available().unwrap();
        }
        let engine = engine_with_logical_resume();
        let mut other = engine_with_logical_resume();
        let foreign = other.reserve_queue_structure().unwrap();
        assert!(engine.capture_reserved_logical_queue(&foreign).is_err());
        engine.ensure_logical_resume_available().unwrap();
    }

    #[test]
    fn ordinary_queue_reservation_does_not_freeze_unaffected_logical_resume() {
        let mut engine = engine_with_logical_resume();
        let owner = engine.reserve_queue_structure().unwrap();
        engine.ensure_logical_resume_available().unwrap();
        let uri = &engine.queue[1].uri;
        let (socket, server) = fake_mpd_script(vec![
            ("play 1".into(), Some("OK\n".into())),
            (
                "status".into(),
                Some("state: play\nsong: 1\nelapsed: 0\nOK\n".into()),
            ),
        ]);
        let queue = engine.get_queue();
        assert!(!uri.is_empty());
        engine.socket_path = socket;
        engine.restored_resume.as_mut().unwrap().elapsed = 0.0;
        engine.play(false).unwrap();
        assert!(engine.restored_resume.is_none());
        assert_eq!(engine.get_queue(), queue);
        server.join().unwrap();
        drop(owner);
    }

    #[test]
    fn reserved_logical_resume_recovery_restores_cursor_cache_without_play_or_seek() {
        for state in [PlaybackState::Playing, PlaybackState::Paused] {
            let mut engine = engine_with_logical_resume();
            engine.restored_resume.as_mut().unwrap().state = state;
            // Legitimate duplicate CUE occurrences and a Plex stable identity.
            for track in &mut engine.queue {
                track.uri = "Keep/Imagem.CUE/track0001".into();
                track.thumb = Some("data:image/png;base64,ARTWORK".into());
            }
            let mut plex = plex_track();
            plex.uri = "https://fixture.invalid/file?X-Plex-Token=SECRET".into();
            engine.queue.push(plex);
            let uris = engine
                .queue
                .iter()
                .map(|t| t.uri.clone())
                .collect::<Vec<_>>();
            let expected = MpdPlaybackSnapshot {
                uris: uris.clone(),
                current_index: None,
                elapsed: 0.0,
                state: PlaybackState::Stopped,
                playlist_version: 31,
            };
            let mut commands = "command_list_begin\nclear\n".to_owned();
            for uri in &uris {
                commands.push_str(&format!(
                    "add {}\n",
                    AudioEngine::quote_mpd_argument(uri).unwrap()
                ));
            }
            commands.push_str("command_list_end");
            let mut script = vec![(commands, Some("OK\n".into()))];
            script.extend(logical_recovery_script(
                &uris,
                PlaybackState::Stopped,
                None,
                0.0,
            ));
            let (socket, server) = fake_mpd_script(script);
            engine.socket_path = socket;
            let mut owner = engine.reserve_queue_structure().unwrap();
            let captured = engine.capture_reserved_logical_queue(&owner).unwrap();
            let effect = engine.begin_reserved_queue_effect(&mut owner).unwrap();
            AudioEngine::restore_mpd_playback_snapshot(&engine.socket_path, &expected).unwrap();
            let verified = effect
                .confirm_reserved_logical_queue_restore(&captured, &expected)
                .unwrap();
            drop(effect);
            engine
                .apply_verified_reserved_queue(&owner, Vec::new())
                .unwrap();
            let cache = engine
                .apply_verified_reserved_logical_queue(&owner, verified)
                .unwrap();
            assert_eq!(engine.restored_resume, Some(captured.resume));
            assert_eq!(engine.get_queue().len(), 3);
            let saved = String::from_utf8_lossy(&cache);
            for secret in ["SECRET", "https://", "ARTWORK", "thumb", "plex_image"] {
                assert!(!saved.contains(secret));
            }
            let (decoded, resume, _) =
                AudioEngine::decode_queue_cache_with_resume(cache.as_slice()).unwrap();
            assert_eq!(decoded.len(), 3);
            assert_eq!(resume, Some(captured.resume));
            assert!(engine.play(false).is_err()); // Protected until durable completion.
            owner.release_after_confirmed_completion();
            drop(owner);
            engine.ensure_startup_queue_ready().unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn reserved_logical_resume_unproven_recovery_or_encoding_failure_never_publishes() {
        for fault in ["uri", "state", "index", "elapsed", "error", "serialize"] {
            let mut engine = engine_with_logical_resume();
            let uris = engine
                .queue
                .iter()
                .map(|t| t.uri.clone())
                .collect::<Vec<_>>();
            let mut actual = uris.clone();
            if fault == "uri" {
                actual[0] = "Other/one.flac".into();
            }
            let state = if fault == "state" {
                PlaybackState::Playing
            } else {
                PlaybackState::Stopped
            };
            let index = if matches!(fault, "state" | "index") {
                Some(0)
            } else {
                None
            };
            let elapsed = if fault == "elapsed" { 4.0 } else { 0.0 };
            let mut script = logical_recovery_script(&actual, state, index, elapsed);
            if fault == "error" {
                script
                    .last_mut()
                    .unwrap()
                    .1
                    .as_mut()
                    .unwrap()
                    .insert_str(0, "error: /private/SECRET\n");
            }
            let (socket, server) = fake_mpd_script(script);
            engine.socket_path = socket;
            let mut owner = engine.reserve_queue_structure().unwrap();
            let captured = engine.capture_reserved_logical_queue(&owner).unwrap();
            let expected = MpdPlaybackSnapshot {
                uris,
                current_index: None,
                elapsed: 0.0,
                state: PlaybackState::Stopped,
                playlist_version: 31,
            };
            let effect = engine.begin_reserved_queue_effect(&mut owner).unwrap();
            let result = effect.confirm_reserved_logical_queue_restore_with(
                &captured,
                &expected,
                |tracks, resume| {
                    if fault == "serialize" {
                        Err("simulated encoding SECRET".into())
                    } else {
                        AudioEngine::encode_queue_cache_with_resume(tracks, resume)
                    }
                },
            );
            assert_eq!(
                result.err().unwrap(),
                "LOCAL_SOURCE_REMOVAL_QUEUE_RECOVERY_UNCONFIRMED"
            );
            assert_eq!(engine.get_queue(), captured.tracks);
            assert_eq!(engine.restored_resume, Some(captured.resume));
            assert_eq!(engine.startup_restore, StartupQueueRestore::Failed);
            drop(effect);
            drop(owner);
            assert!(engine.reserve_queue_structure().is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn reserved_logical_resume_snapshot_and_proof_cannot_cross_reservation_owners() {
        let mut engine = engine_with_logical_resume();
        let owner = engine.reserve_queue_structure().unwrap();
        let captured = engine.capture_reserved_logical_queue(&owner).unwrap();
        drop(owner);
        let mut next = engine.reserve_queue_structure().unwrap();
        let current = engine.capture_reserved_logical_queue(&next).unwrap();
        let expected = MpdPlaybackSnapshot {
            uris: engine.queue.iter().map(|t| t.uri.clone()).collect(),
            current_index: None,
            elapsed: 0.0,
            state: PlaybackState::Stopped,
            playlist_version: 31,
        };
        let effect = engine.begin_reserved_queue_effect(&mut next).unwrap();
        assert!(effect
            .confirm_reserved_logical_queue_restore(&captured, &expected)
            .is_err());
        drop(effect);
        // Private capability construction here tests the owner's apply boundary.
        let wrong = VerifiedLogicalQueueRestore {
            tracks: captured.tracks,
            resume: captured.resume,
            cache: vec![],
            identity: captured.identity,
        };
        assert!(engine
            .apply_verified_reserved_logical_queue(&next, wrong)
            .is_err());
        assert_eq!(engine.restored_resume, Some(current.resume));
    }

    #[test]
    fn reserved_logical_resume_mpd_ack_or_eof_preserves_cursor_and_protection() {
        for response in [Some("ACK [50@0] {status} /private/SECRET\n".into()), None] {
            let mut engine = engine_with_logical_resume();
            let (socket, server) = fake_mpd_script(vec![("status".into(), response)]);
            engine.socket_path = socket;
            let mut owner = engine.reserve_queue_structure().unwrap();
            let captured = engine.capture_reserved_logical_queue(&owner).unwrap();
            let expected = MpdPlaybackSnapshot {
                uris: engine.queue.iter().map(|track| track.uri.clone()).collect(),
                state: PlaybackState::Stopped,
                current_index: None,
                elapsed: 0.0,
                playlist_version: 31,
            };
            let effect = engine.begin_reserved_queue_effect(&mut owner).unwrap();
            assert_eq!(
                effect
                    .confirm_reserved_logical_queue_restore(&captured, &expected)
                    .err()
                    .unwrap(),
                "LOCAL_SOURCE_REMOVAL_QUEUE_RECOVERY_UNCONFIRMED"
            );
            assert_eq!(engine.get_queue(), captured.tracks);
            assert_eq!(engine.restored_resume, Some(captured.resume));
            drop(effect);
            drop(owner);
            assert!(engine.play(false).is_err());
            assert!(engine.reserve_queue_structure().is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn queue_structure_reservation_excludes_mutations_and_releases_without_audio_lock() {
        let mut engine = engine_with_track("Álbum/faixa.flac");
        engine.startup_restore = StartupQueueRestore::Pending;
        assert!(engine.reserve_queue_structure().is_none());
        engine.startup_restore = StartupQueueRestore::Ready;
        let reservation = engine.reserve_queue_structure().unwrap();
        assert!(engine.reserve_queue_structure().is_none());
        assert!(engine.clear_queue().unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        assert!(engine.play_tracks(engine.get_queue(), vec!["Álbum/faixa.flac".into()], 0)
            .unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        assert!(engine.restore_after_device_switch(None, &["Álbum/faixa.flac".into()])
            .unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        engine.startup_restore = StartupQueueRestore::Pending;
        assert!(engine.restore_startup_queue(&["Álbum/faixa.flac".into()])
            .unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        engine.startup_restore = StartupQueueRestore::Ready;
        assert!(engine.play_uris(vec!["Álbum/faixa.flac".into()], 0).unwrap_err()
            .starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        let audio = Mutex::new(engine);
        let mut engine = audio.lock().unwrap();
        // Drop must not need AudioState, even when another command owns it.
        drop(reservation);
        let reservation = engine.reserve_queue_structure().unwrap();
        drop(reservation);
        let (socket, server) = fake_mpd_script(vec![(
            "command_list_begin\nclear\nadd \"Álbum/faixa.flac\"\nplay 0\ncommand_list_end".into(),
            Some("OK\n".into()),
        )]);
        engine.socket_path = socket;
        engine.play_uris(vec!["Álbum/faixa.flac".into()], 0).unwrap();
        assert_eq!(engine.get_queue()[0].uri, "Álbum/faixa.flac");
        server.join().unwrap();
    }

    #[test]
    fn destructive_queue_owner_is_required_and_uncertainty_survives_drop() {
        let mut engine = engine_with_track("Remove/Imagem.CUE/track0001");
        let mut other = engine_with_track("Keep/one.flac");
        let mut foreign = other.reserve_queue_structure().unwrap();
        assert!(engine.begin_reserved_queue_effect(&mut foreign).is_err());
        assert!(engine
            .apply_verified_reserved_queue(&foreign, vec![])
            .is_err());
        assert_eq!(engine.get_queue().len(), 1);
        let mut owner = engine.reserve_queue_structure().unwrap();
        engine.begin_reserved_queue_effect(&mut owner).unwrap();
        assert!(engine.play_index(0, false).is_err());
        assert!(!engine.queue_has_logical_resume());
        drop(owner);
        assert!(engine.reserve_queue_structure().is_none());
        assert!(engine.clear_queue().is_err());
        assert_eq!(engine.get_queue().len(), 1);

        let mut engine = engine_with_track("Keep/one.flac");
        engine.restored_resume = Some(PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 17.0,
            state: PlaybackState::Paused,
        });
        assert!(engine.queue_has_logical_resume());
        let mut owner = engine.reserve_queue_structure().unwrap();
        engine.begin_reserved_queue_effect(&mut owner).unwrap();
        engine
            .apply_verified_reserved_queue(&owner, vec![])
            .unwrap();
        owner.release_after_confirmed_completion();
        drop(owner);
        assert!(engine.reserve_queue_structure().is_some());
        assert!(!engine.queue_has_logical_resume());
    }

    #[test]
    fn reserved_queue_recovery_rejects_unproven_identity_position_state_and_output_error() {
        const STATUS: &str =
            "playlist: 34\nplaylistlength: 1\nstate: pause\nsong: 0\nelapsed: 17\nOK\n";
        const QUEUE: &str = "file: Remove/Imagem.CUE/track0001\nPos: 0\nId: 171\nOK\n";
        for fault in ["uri", "elapsed", "state", "error"] {
            let actual_status = match fault {
                "elapsed" => STATUS.replace("elapsed: 17", "elapsed: 0"),
                "state" => STATUS.replace("state: pause", "state: play"),
                _ => STATUS.to_owned(),
            };
            let actual_queue = if fault == "uri" {
                QUEUE.replace("track0001", "track0002")
            } else {
                QUEUE.to_owned()
            };
            let mut script = Vec::new();
            for _ in 0..2 {
                script.extend([
                    ("status".into(), Some(actual_status.clone())),
                    ("playlistinfo".into(), Some(actual_queue.clone())),
                    ("status".into(), Some(actual_status.clone())),
                ]);
            }
            if fault == "error" {
                script.push((
                    "status".into(),
                    Some("error: output unavailable\nOK\n".into()),
                ));
            }
            let (socket, server) = fake_mpd_script(script);
            let mut engine = engine_with_track("Remove/Imagem.CUE/track0001");
            engine.socket_path = socket;
            let snapshot = MpdPlaybackSnapshot {
                uris: vec![engine.queue[0].uri.clone()],
                current_index: Some(0),
                elapsed: 17.0,
                state: PlaybackState::Paused,
                playlist_version: 31,
            };
            let mut owner = engine.reserve_queue_structure().unwrap();
            let effect = engine.begin_reserved_queue_effect(&mut owner).unwrap();
            assert!(effect
                .confirm_reserved_queue_restore(engine.get_queue(), &snapshot, false)
                .is_err());
            assert_eq!(engine.startup_restore, StartupQueueRestore::Failed);
            server.join().unwrap();
        }
    }

    #[test]
    fn removal_queue_structure_accepts_duplicates_cue_and_playback_transitions() {
        let playlist = "file: Keep/Álbum/a.flac\nPos: 0\nId: 7\nfile: Keep/Álbum/a.flac\nPos: 1\nId: 8\nfile: Keep/Imagem.CUE/track0001\nPos: 2\nId: 9\nOK\n";
        let mut captures = Vec::new();
        for (before, after) in [("play", "pause"), ("pause", "stop"), ("stop", "play")] {
            let (socket, server) = fake_mpd_script(vec![
                ("status".into(), Some(queue_status_response(31, 3, AudioEngine::parse_playback_state(before).unwrap(), Some(0), Some(12.0)))),
                ("playlistinfo".into(), Some(playlist.into())),
                ("status".into(), Some(queue_status_response(31, 3, AudioEngine::parse_playback_state(after).unwrap(), Some(2), Some(0.0)))),
            ]);
            captures.push(AudioEngine::capture_mpd_queue_structure(&socket).unwrap());
            server.join().unwrap();
        }
        assert_eq!(captures[0], captures[1]);
        assert_eq!(captures[1], captures[2]);
        assert_eq!(captures[0].entries.iter().map(|entry| (entry.pos, entry.id)).collect::<Vec<_>>(),
            [(0, 7), (1, 8), (2, 9)]);
    }

    #[test]
    fn queue_structure_reservation_keeps_playback_controls_available() {
        let mut engine = engine_with_two_tracks();
        let uris = engine.get_queue().iter().map(|track| track.uri.clone()).collect::<Vec<_>>();
        let reservation = engine.reserve_queue_structure().unwrap();
        let (socket, server) = fake_mpd_script(vec![
            ("play 1".into(), Some("OK\n".into())),
            ("next".into(), Some("OK\n".into())),
            ("previous".into(), Some("OK\n".into())),
            ("seekcur 4.0".into(), Some("OK\n".into())),
            ("status".into(), Some("state: play\nOK\n".into())),
            ("pause 1".into(), Some("OK\n".into())),
            ("status".into(), Some("state: pause\nOK\n".into())),
            ("pause 0".into(), Some("OK\n".into())),
            ("status".into(), Some("state: play\nOK\n".into())),
        ]);
        engine.socket_path = socket;
        engine.play_index(1, false).unwrap();
        engine.next(false).unwrap();
        engine.previous(false).unwrap();
        engine.seek(4.0, false).unwrap();
        engine.pause().unwrap();
        engine.play(false).unwrap();
        assert_eq!(engine.playback_state().unwrap(), PlaybackState::Playing);
        assert_eq!(engine.get_queue().iter().map(|track| track.uri.clone()).collect::<Vec<_>>(), uris);
        assert!(engine.clear_queue().unwrap_err().starts_with("LOCAL_SOURCE_REMOVAL_QUEUE_RESERVED"));
        drop(reservation);
        server.join().unwrap();
    }

    #[test]
    fn removal_queue_structure_rejects_ambiguous_identity_and_query_errors_safely() {
        for response in [
            "file: Keep/a.flac\nPos: 0\n", "file: Keep/a.flac\nId: 7\n",
            "file: Keep/a.flac\nPos: 1\nId: 7\n",
            "file: Keep/a.flac\nPos: 0\nPos: 0\nId: 7\n",
            "file: Keep/a.flac\nPos: 0\nId: 7\nId: 7\n",
            "file: Keep/a.flac\nPos: 0\nId: 7\nfile: Keep/b.flac\nPos: 1\nId: 7\n",
            "Pos: 0\nId: 7\nfile: Keep/a.flac\n", "file: Keep/a.flac\nPos: -1\nId: 7\n",
        ] {
            let lines = response.lines().map(str::to_string).collect::<Vec<_>>();
            assert_eq!(AudioEngine::parse_mpd_queue_entries(&lines), Err(MpdQueueStructureError::Invalid));
        }
        let status = queue_status_response(31, 1, PlaybackState::Playing, Some(0), Some(1.0));
        let (socket, server) = fake_mpd_script(vec![
            ("status".into(), Some(status)),
            ("playlistinfo".into(), Some("file: Keep/a.flac\nPos: 0\nId: 7\nOK\n".into())),
            ("status".into(), Some(queue_status_response(32, 1, PlaybackState::Playing, Some(0), Some(2.0)))),
        ]);
        assert_eq!(AudioEngine::capture_mpd_queue_structure(&socket), Err(MpdQueueStructureError::Changed));
        server.join().unwrap();
        for response in [Some("ACK [5@0] {status} https://secret.invalid/?token=PRIVATE\n".into()), None] {
            let (socket, server) = fake_mpd_script(vec![("status".into(), response)]);
            let error = AudioEngine::capture_mpd_queue_structure(&socket).unwrap_err();
            assert_eq!(error, MpdQueueStructureError::QueryFailed);
            assert!(!error.to_string().contains("PRIVATE"));
            server.join().unwrap();
        }
        assert_eq!(AudioEngine::capture_mpd_queue_structure("/no-test-mpd"), Err(MpdQueueStructureError::QueryFailed));
    }

    #[test]
    fn playback_error_classification_requires_known_output_context() {
        for (message, expected) in [
            (
                "Failed to open ALSA device \"hw:0\": Device or resource busy",
                PlaybackDiagnostic::OutputBusy,
            ),
            (
                "Failed to open ALSA device \"hw:0\": No such device",
                PlaybackDiagnostic::OutputUnavailable,
            ),
            (
                "Failed to open ALSA device \"hw:0\": No such device or address",
                PlaybackDiagnostic::OutputUnavailable,
            ),
            (
                "Failed to open ALSA device \"hw:0\": No such file or directory",
                PlaybackDiagnostic::OutputUnavailable,
            ),
            (
                "All audio outputs are disabled",
                PlaybackDiagnostic::OutputUnavailable,
            ),
            (
                "Failed to decode file: Device or resource busy",
                PlaybackDiagnostic::PlaybackError,
            ),
            (
                "Failed to open ALSA device \"hw:0\": Invalid argument",
                PlaybackDiagnostic::PlaybackError,
            ),
            ("Unknown failure", PlaybackDiagnostic::PlaybackError),
        ] {
            assert_eq!(PlaybackDiagnostic::from_mpd_error(message), expected);
        }
    }

    #[test]
    fn playback_error_after_accepted_play_is_reported_then_cleared() {
        let (socket, server) = fake_mpd_script(vec![
            ("status".into(), Some("state: stop\nOK\n".into())),
            ("play".into(), Some("OK\n".into())),
            ("status".into(), Some("state: pause\nsong: 0\nelapsed: 16\nerror: Failed to open \"Sonante Output\" (alsa); Failed to open ALSA device \"hw:0\": Device or resource busy\nOK\n".into())),
            ("currentsong".into(), Some("Pos: 0\nOK\n".into())),
            ("status".into(), Some("state: play\nsong: 0\nelapsed: 17\nOK\n".into())),
            ("currentsong".into(), Some("Pos: 0\nOK\n".into())),
        ]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        let queue_before = serde_json::to_string(&engine.queue).unwrap();
        engine.play(false).unwrap();
        let failed = engine.get_status().unwrap();
        assert_eq!(failed.state, "pause");
        assert_eq!(failed.elapsed, 16.0);
        assert_eq!(failed.playback_error, Some(PlaybackDiagnostic::OutputBusy));
        let resumed = engine.get_status().unwrap();
        assert_eq!(resumed.state, "play");
        assert_eq!(resumed.elapsed, 17.0);
        assert_eq!(resumed.playback_error, None);
        assert_eq!(serde_json::to_string(&engine.queue).unwrap(), queue_before);
        server.join().unwrap();
    }

    #[test]
    fn playback_error_observation_never_suppresses_auxiliary_or_primary_errors() {
        let errors = [
            "Failed to write to FIFO /private/fifo: Broken pipe",
            "Failed to open \"Sonante Output\" (alsa); Failed to open ALSA device \"hw:private\": Invalid argument",
        ];
        let engine = engine_with_track("local/fixture.flac");
        let queue_before = serde_json::to_string(&engine.queue).unwrap();
        let mut snapshots = Vec::new();
        for error in errors {
            let status = engine
                .get_status_with_io(
                    true,
                    true,
                    |_| None,
                    |command| {
                        Ok(match command {
                            "status" => vec![
                                "state: play".into(),
                                "song: 0".into(),
                                "elapsed: 17".into(),
                                format!("error: {error}"),
                            ],
                            "currentsong" => vec!["Pos: 0".into()],
                            _ => panic!("observation must only read status/current song"),
                        })
                    },
                )
                .unwrap();
            assert_eq!(status.state, "play");
            assert_eq!(status.elapsed, 17.0);
            assert_eq!(
                status.playback_error,
                Some(PlaybackDiagnostic::PlaybackError)
            );
            snapshots.push(status);
        }
        assert_eq!(snapshots[0].current_media, snapshots[1].current_media);
        assert_eq!(serde_json::to_string(&engine.queue).unwrap(), queue_before);
        // The fixture accepts status/currentsong only: no clearerror, transport,
        // queue or output commands are permitted by an observation.
    }

    #[test]
    fn playback_error_is_safe_in_play_pause_and_stop_for_local_and_plex() {
        for state in ["play", "pause", "stop"] {
            for plex in [false, true] {
                let (socket, server) = fake_mpd_script(vec![
                    ("status".into(), Some(format!("state: {state}\nsong: 0\nelapsed: 16\nerror: Failed to decode /private/Álbum secreto.flac https://user:password@example.invalid/file?X-Plex-Token=SECRET\nOK\n"))),
                    ("currentsong".into(), Some("Pos: 0\nOK\n".into())),
                ]);
                let mut engine = engine_with_track("local/faixa.flac");
                if plex {
                    engine.queue = vec![plex_track()];
                }
                engine.socket_path = socket;
                let status = engine.get_status().unwrap();
                assert_eq!(status.state, state);
                assert_eq!(status.elapsed, 16.0);
                assert_eq!(
                    status.playback_error,
                    Some(PlaybackDiagnostic::PlaybackError)
                );
                assert_eq!(
                    status.current_media,
                    engine.current_media_for_queue_index(Some(0))
                );
                let json = serde_json::to_string(&status).unwrap();
                for sensitive in [
                    "/private/",
                    "Álbum secreto",
                    "https://",
                    "password",
                    "X-Plex-Token",
                    "SECRET",
                ] {
                    assert!(!json.contains(sensitive));
                    assert!(!format!("{:?}", status.playback_error).contains(sensitive));
                }
                server.join().unwrap();
            }
        }
    }

    #[test]
    fn playback_error_survives_neutral_stop_and_logical_resume_and_mpris() {
        for logical_resume in [false, true] {
            let (socket, server) = fake_mpd_script(vec![
                (
                    "status".into(),
                    Some("state: stop\nerror: All audio outputs are disabled\nOK\n".into()),
                ),
                (
                    "status".into(),
                    Some("state: stop\nerror: All audio outputs are disabled\nOK\n".into()),
                ),
                ("status".into(), Some("state: stop\nOK\n".into())),
            ]);
            let mut engine = engine_with_track("local/faixa.flac");
            engine.socket_path = socket;
            if logical_resume {
                engine.restored_resume = Some(PlaybackResumeSnapshot {
                    queue_index: 0,
                    elapsed: 16.0,
                    state: PlaybackState::Paused,
                });
            }
            let status = engine.get_status().unwrap();
            assert_eq!(status.state, if logical_resume { "pause" } else { "stop" });
            assert_eq!(
                status.playback_error,
                Some(PlaybackDiagnostic::OutputUnavailable)
            );
            let mpris = engine.get_mpris_status().unwrap();
            assert_eq!(mpris.state, "stop");
            assert_eq!(mpris.playback_error, status.playback_error);
            assert_eq!(engine.get_status().unwrap().playback_error, None);
            server.join().unwrap();
        }
    }

    #[test]
    fn playback_error_codes_never_serialize_raw_output_messages() {
        for suffix in [
            "Device or resource busy",
            "No such device",
            "Unexpected failure",
        ] {
            let raw = format!("Failed to open ALSA device \"/private/SECRET https://user:password@example.invalid/?X-Plex-Token=SECRET\": {suffix}");
            let diagnostic = PlaybackDiagnostic::from_mpd_error(&raw);
            let code = match diagnostic {
                PlaybackDiagnostic::OutputBusy => "output_busy",
                PlaybackDiagnostic::OutputUnavailable => "output_unavailable",
                PlaybackDiagnostic::PlaybackError => "playback_error",
                PlaybackDiagnostic::QueueUncertain => "queue_uncertain",
                PlaybackDiagnostic::PersistenceFailed => "persistence_failed",
                PlaybackDiagnostic::FormatUnsupported => "format_unsupported",
            };
            assert_eq!(
                serde_json::to_string(&diagnostic).unwrap(),
                format!("{{\"code\":\"{code}\"}}")
            );
        }
    }

    #[test]
    fn playback_error_does_not_convert_mpd_ack_to_success() {
        let (socket, server) = fake_mpd_script(vec![(
            "status".into(),
            Some("ACK [5@0] {status} simulated failure\n".into()),
        )]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        assert!(engine.get_status().is_err());
        server.join().unwrap();
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

    // A rejected cache must never be opened or migrated through its link.
    #[test]
    fn rejected_queue_cache_symlink_is_not_loaded_or_migrated() {
        use std::os::unix::fs::symlink;
        let path = get_queue_cache_path().unwrap(); // cfg(test): private temp, never HOME
        let target = path.with_file_name("synthetic-cache-target.json");
        let queue = engine_with_track("Synthetic/track.flac").queue;
        let bytes = serde_json::to_vec(&queue).unwrap(); // legacy payload would request migration
        crate::persistence::atomic_write_private(&target, &bytes, "synthetic cache").unwrap();
        symlink(&target, &path).unwrap();
        assert!(
            crate::persistence::prepare_private_file_for_load(&path, "queue_cache.json").is_err()
        );
        let (loaded, resume) = AudioEngine::load_queue_cache();
        assert!(loaded.is_empty());
        assert!(resume.is_none());
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(std::fs::read_link(&path).unwrap(), target);
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&target).unwrap();
    }

    #[test]
    fn queue_cache_load_absent_then_regular_preserves_resume_without_rewrite_or_play() {
        let path = get_queue_cache_path().unwrap();
        assert!(!path.exists());
        let (queue, resume) = AudioEngine::load_queue_cache();
        assert!(queue.is_empty() && resume.is_none());
        assert!(!path.exists(), "absence must not create a cache file");
        let queue = engine_with_track("Synthetic/regular.flac").queue;
        let resume = PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 17.5,
            state: PlaybackState::Paused,
        };
        let bytes = AudioEngine::encode_queue_cache_with_resume(&queue, Some(&resume)).unwrap();
        crate::persistence::atomic_write_private(&path, &bytes, "queue_cache.json").unwrap();
        // new() only loads/arms startup state; no socket or playback is started.
        let engine = AudioEngine::new("/no-test-mpd", "/no-test-music");
        assert_eq!(engine.queue.len(), 1);
        assert_eq!(engine.queue[0].uri, "Synthetic/regular.flac");
        assert_eq!(engine.restored_resume.as_ref(), Some(&resume));
        assert!(engine.startup_queue_pending());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn queue_cache_load_rejects_non_regular_entry_without_replacing_it() {
        let path = get_queue_cache_path().unwrap();
        std::fs::create_dir_all(&path).unwrap();
        let sentinel = path.join("synthetic-sentinel");
        std::fs::write(&sentinel, b"preserved fixture").unwrap();
        assert!(
            crate::persistence::prepare_private_file_for_load(&path, "queue_cache.json").is_err()
        );
        let (queue, resume) = AudioEngine::load_queue_cache();
        assert!(queue.is_empty() && resume.is_none());
        assert!(path.is_dir());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"preserved fixture");
        std::fs::remove_file(sentinel).unwrap();
        std::fs::remove_dir(path).unwrap();
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
        assert!(engine.play_index(0, false).is_err());
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
        assert!(engine.play_index(0, false).is_err());
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
    fn telemetry_preserves_cached_artwork_media_and_pause_play_without_filesystem_lookup() {
        for state in ["pause", "play"] {
            let mut engine = engine_with_track("UnavailableNAS/faixa.flac");
            engine.queue[0].thumb = Some("data:image/png;base64,cached".into());
            let query = |command: &str| -> Result<Vec<String>, String> {
                Ok(if command == "status" {
                    vec![
                        format!("state: {state}"),
                        "song: 0".into(),
                        "elapsed: 1.0".into(),
                        "duration: 10.0".into(),
                    ]
                } else {
                    assert_eq!(command, "currentsong");
                    vec!["file: UnavailableNAS/faixa.flac".into(), "Pos: 0".into()]
                })
            };
            let status = engine.get_telemetry_status_with(query).unwrap();
            assert_eq!(status.state, state);
            assert_eq!(
                status.thumb.as_deref(),
                Some("data:image/png;base64,cached")
            );
            assert_eq!(status.elapsed, 1.0);
            assert!(status.current_media.is_some());
            engine.queue[0].thumb = None;
            let status = engine.get_telemetry_status_with(query).unwrap();
            assert!(status.thumb.is_none());
            assert_eq!(status.state, state);
            // Normal artwork lookup is separately injected; telemetry bypasses it.
            let reads = std::cell::Cell::new(0);
            engine
                .get_status_with_io(
                    true,
                    true,
                    |_| {
                        reads.set(reads.get() + 1);
                        None
                    },
                    query,
                )
                .unwrap();
            assert!(reads.get() > 0);
        }
    }
    #[test]
    fn telemetry_failure_recovery_and_logical_resume_do_not_play_or_restart() {
        let mut engine = engine_with_track("UnavailableNAS/faixa.flac");
        for error in [
            "controlled timeout",
            "ACK [5@0] {status} unavailable",
            "connection reset",
        ] {
            assert_eq!(
                engine
                    .get_telemetry_status_with(|command| {
                        assert_eq!(command, "status");
                        Err(error.into())
                    })
                    .unwrap_err(),
                error
            );
        }
        engine.restored_resume = Some(PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 16.0,
            state: PlaybackState::Paused,
        });
        let status = engine
            .get_telemetry_status_with(|command| {
                assert_eq!(command, "status");
                Ok(vec!["state: stop".into(), "volume: 70".into()])
            })
            .unwrap();
        assert_eq!(status.state, "pause");
        assert_eq!(status.elapsed, 16.0);
        assert!(status.current_media.is_some());
        assert!(status.thumb.is_none());
        engine.restored_resume = None;
        let status = engine
            .get_telemetry_status_with(|command| {
                assert_eq!(command, "status");
                Ok(vec!["state: stop".into()])
            })
            .unwrap();
        assert_eq!(status.state, "stop");
    }

    #[test]
    fn mpris_status_preserves_media_fields_without_cover_lookup() {
        let (socket, server) = fake_album_mpd(vec![
            (
                "status",
                "state: play\nsong: 0\nelapsed: 1.0\nduration: 10.0\nOK\n",
            ),
            (
                "currentsong",
                "file: local/faixa.flac\nTitle: Faixa\nArtist: Artista\nAlbum: Álbum\nPos: 0\nOK\n",
            ),
        ]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.queue[0].thumb = Some("data:image/jpeg;base64,AAAA".into());

        let status = engine.get_mpris_status().unwrap();

        assert_eq!(status.title, "Faixa");
        assert_eq!(status.artist, "Artista");
        assert_eq!(status.album, "Álbum");
        assert_eq!(status.duration, 10.0);
        assert!(status.current_media.is_some());
        assert!(status.thumb.is_none());
        assert!(status.plex_image.is_none());
        server.join().unwrap();
    }

    #[test]
    fn mpris_status_reports_raw_mpd_stop_instead_of_logical_resume() {
        let (socket, server) =
            fake_album_mpd(vec![("status", "state: stop\nvolume: 70\nOK\n")]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 201.4,
            state: PlaybackState::Playing,
        });

        let status = engine.get_mpris_status().unwrap();

        assert_eq!(status.state, "stop");
        assert_eq!(status.elapsed, 0.0);
        assert!(status.current_media.is_none());
        server.join().unwrap();
    }

    #[test]
    fn play_after_resume_selects_then_seeks_and_queue_click_selects() {
        let (socket, server) = fake_album_mpd(vec![("play 0", "OK\n"), ("seekcur 201.4", "OK\n"), ("status", "state: play\nOK\n"), ("play 0", "OK\n")]);
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 201.4, state: PlaybackState::Paused });
        engine.toggle_play_pause(false).unwrap();
        assert!(engine.restored_resume.is_none());
        engine.play_index(0, false).unwrap();
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
        assert!(engine.toggle_play_pause(false).is_ok());
        assert!(engine.restored_resume.is_none());
        server.join().unwrap();

        let (socket, server) = fake_album_mpd(vec![("play 0", "OK\n"), ("seekcur 9.0", "ACK [5@0] {seekcur} failure\n")]);
        engine.socket_path = socket;
        engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 9.0, state: PlaybackState::Paused });
        assert!(engine.toggle_play_pause(false).is_err());
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
        engine.toggle_play_pause(false).unwrap();
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
    fn direct_queue_index_sends_only_play_and_preserves_queue() {
        for track in [
            engine_with_track("local/Álbum com espaço/terceira.flac").queue[0].clone(),
            plex_track(),
        ] {
            let (socket, server) = fake_album_mpd(vec![("play 2", "OK\n")]);
            let mut engine = engine_with_track("local/primeira.flac");
            engine.socket_path = socket;
            engine.queue = vec![engine.queue[0].clone(), engine.queue[0].clone(), track];
            engine.restored_resume = Some(PlaybackResumeSnapshot {
                queue_index: 0,
                elapsed: 9.0,
                state: PlaybackState::Paused,
            });
            let queue = engine.queue.clone();

            engine.play_index(2, false).unwrap();

            assert_eq!(engine.queue, queue);
            assert!(engine.restored_resume.is_none());
            server.join().unwrap();
        }
    }

    #[test]
    fn shared_queue_index_with_progress_does_not_recover() {
        for track in [
            engine_with_track("local/Álbum com espaço/terceira.flac").queue[0].clone(),
            plex_track(),
        ] {
            let (result, cursor) = shared_queue_index_script(
                vec![
                    ("play 2", Some("OK\n")),
                    ("status", Some("state: play\nelapsed: 0.10\nOK\n")),
                    ("status", Some("state: play\nelapsed: 0.19\nOK\n")),
                ],
                track,
            );
            result.unwrap();
            assert!(cursor.is_none());
        }
    }

    #[test]
    fn shared_queue_index_stall_recovers_once_without_replaying_or_seeking() {
        for track in [
            engine_with_track("local/Álbum com espaço/terceira.flac").queue[0].clone(),
            plex_track(),
        ] {
            let (result, cursor) = shared_queue_index_script(
                vec![
                    ("play 2", Some("OK\n")),
                    ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
                    ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
                    ("pause 1", Some("OK\n")),
                    ("pause 0", Some("OK\n")),
                    ("status", Some("state: play\nelapsed: 0.10\nOK\n")),
                    ("status", Some("state: play\nelapsed: 0.19\nOK\n")),
                ],
                track,
            );
            result.unwrap();
            assert!(cursor.is_none());
        }
    }

    #[test]
    fn shared_queue_index_persistent_stall_is_an_error_after_one_recovery() {
        let (result, cursor) = shared_queue_index_script(
            vec![
                ("play 2", Some("OK\n")),
                ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
                ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
                ("pause 1", Some("OK\n")),
                ("pause 0", Some("OK\n")),
                ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
                ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
            ],
            plex_track(),
        );
        assert!(result.unwrap_err().contains("permaneceu congelada"));
        assert!(cursor.is_none());
    }

    #[test]
    fn shared_queue_index_propagates_ack_and_eof_at_each_step() {
        let script = [
            ("play 2", Some("OK\n")),
            ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
            ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
            ("pause 1", Some("OK\n")),
            ("pause 0", Some("OK\n")),
            ("status", Some("state: play\nelapsed: 0.10\nOK\n")),
            ("status", Some("state: play\nelapsed: 0.19\nOK\n")),
        ];
        for fail_at in 0..script.len() {
            for failure in [Some("ACK [5@0] {test} simulated failure\n"), None] {
                let mut responses = script[..=fail_at].to_vec();
                responses[fail_at].1 = failure;
                let (result, cursor) = shared_queue_index_script(responses, plex_track());
                let error = result.unwrap_err();
                assert!(
                    error.contains(if failure.is_some() {
                        "ACK"
                    } else {
                        "Conexão MPD encerrada"
                    }),
                    "step {fail_at}: {error}"
                );
                // An accepted selection invalidates the old resume cursor even
                // if verification fails; a play without a confirmed OK preserves it.
                assert_eq!(cursor.is_some(), fail_at == 0);
                if let Some(cursor) = cursor {
                    assert_eq!(cursor.queue_index, 0);
                    assert_eq!(cursor.elapsed, 9.0);
                }
            }
        }
    }

    #[test]
    fn shared_queue_index_invalid_progress_never_recovers() {
        for response in [
            "state: stop\nOK\n",
            "state: pause\nelapsed: 0.0\nOK\n",
            "state: play\nelapsed: NaN\nOK\n",
        ] {
            let (result, cursor) = shared_queue_index_script(
                vec![
                    ("play 2", Some("OK\n")),
                    ("status", Some("state: play\nelapsed: 0.0\nOK\n")),
                    ("status", Some(response)),
                ],
                plex_track(),
            );
            assert!(result.is_err());
            assert!(cursor.is_none());
        }
    }

    #[test]
    fn shared_queue_index_rejects_unready_or_invalid_selection_before_io() {
        let mut engine = engine_with_track("local/faixa.flac");
        engine.socket_path = "/socket/must-not-be-used".into();
        for (state, expected) in [
            (StartupQueueRestore::Pending, "ainda está sendo restaurada"),
            (StartupQueueRestore::Failed, "não pôde ser restaurada"),
        ] {
            engine.startup_restore = state;
            assert!(engine.play_index(0, true).unwrap_err().contains(expected));
        }
        engine.startup_restore = StartupQueueRestore::Ready;
        assert!(engine
            .play_index(usize::MAX, true)
            .unwrap_err()
            .contains("Índice fora da fila"));
        engine.queue.clear();
        assert!(engine
            .play_index(0, true)
            .unwrap_err()
            .contains("Índice fora da fila"));
    }

    fn shared_queue_index_script(
        script: Vec<(&'static str, Option<&'static str>)>,
        track: TrackMetadata,
    ) -> (Result<(), String>, Option<PlaybackResumeSnapshot>) {
        let (socket, server) = fake_mpd_script(
            script
                .into_iter()
                .map(|(command, response)| (command.to_string(), response.map(str::to_string)))
                .collect(),
        );
        let mut engine = engine_with_track("local/primeira.flac");
        engine.socket_path = socket;
        engine.queue = vec![engine.queue[0].clone(), engine.queue[0].clone(), track];
        engine.restored_resume = Some(PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 9.0,
            state: PlaybackState::Paused,
        });
        let queue = engine.queue.clone();
        let result = engine.play_index(2, true);
        assert_eq!(engine.queue, queue);
        server.join().unwrap();
        (result, engine.restored_resume)
    }

    #[test]
    fn direct_next_and_previous_send_only_the_transport_command() {
        for command in ["next", "previous"] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, sleeps) =
                scripted_track_change(&mut engine, command, false, vec![Ok(vec![])]);

            result.unwrap();
            assert_eq!(commands, vec![command]);
            assert!(sleeps.is_empty());
        }
    }

    #[test]
    fn shared_playing_track_change_with_progress_needs_no_recovery() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, sleeps) = scripted_track_change(
            &mut engine,
            "next",
            true,
            vec![
                seek_status("play", 41.0),
                Ok(vec![]),
                seek_status("play", 0.10),
                seek_status("play", 0.19),
            ],
        );

        result.unwrap();
        assert_eq!(commands, vec!["status", "next", "status", "status"]);
        assert_eq!(sleeps, vec![SHARED_PROGRESS_SAMPLE_DELAY; 2]);
    }

    #[test]
    fn shared_stalled_next_and_previous_recover_once() {
        for command in ["next", "previous"] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, sleeps) = scripted_track_change(
                &mut engine,
                command,
                true,
                vec![
                    seek_status("play", 41.0),
                    Ok(vec![]),
                    seek_status("play", 0.10),
                    seek_status("play", 0.10),
                    Ok(vec![]),
                    Ok(vec![]),
                    seek_status("play", 0.10),
                    seek_status("play", 0.19),
                ],
            );

            result.unwrap();
            assert_eq!(
                commands,
                vec![
                    "status", command, "status", "status", "pause 1", "pause 0", "status",
                    "status",
                ]
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|candidate| candidate.as_str() == "pause 1")
                    .count(),
                1
            );
            assert_eq!(sleeps, vec![SHARED_PROGRESS_SAMPLE_DELAY; 3]);
        }
    }

    #[test]
    fn shared_track_change_still_frozen_after_recovery_is_an_error() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_track_change(
            &mut engine,
            "next",
            true,
            vec![
                seek_status("play", 41.0),
                Ok(vec![]),
                seek_status("play", 0.10),
                seek_status("play", 0.10),
                Ok(vec![]),
                Ok(vec![]),
                seek_status("play", 0.10),
                seek_status("play", 0.10),
            ],
        );

        assert!(result.unwrap_err().contains("permaneceu congelada"));
        assert_eq!(
            commands
                .iter()
                .filter(|candidate| candidate.as_str() == "pause 1")
                .count(),
            1
        );
    }

    #[test]
    fn shared_paused_or_stopped_track_change_does_not_force_playback() {
        for (command, state) in [("next", "pause"), ("previous", "stop")] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, sleeps) = scripted_track_change(
                &mut engine,
                command,
                true,
                vec![seek_status(state, 12.0), Ok(vec![])],
            );

            result.unwrap();
            assert_eq!(commands, vec!["status", command]);
            assert!(sleeps.is_empty());
            assert!(!commands.iter().any(|candidate| candidate.starts_with("pause")));
        }
    }

    #[test]
    fn shared_track_change_propagates_pause_and_resume_failures() {
        for (responses, failed_command) in [
            (
                vec![
                    seek_status("play", 41.0),
                    Ok(vec![]),
                    seek_status("play", 0.10),
                    seek_status("play", 0.10),
                    Err("ACK simulado no pause".into()),
                ],
                "pause 1",
            ),
            (
                vec![
                    seek_status("play", 41.0),
                    Ok(vec![]),
                    seek_status("play", 0.10),
                    seek_status("play", 0.10),
                    Ok(vec![]),
                    Err("ACK simulado no resume".into()),
                ],
                "pause 0",
            ),
        ] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, _) =
                scripted_track_change(&mut engine, "previous", true, responses);

            assert!(result.is_err());
            assert_eq!(commands.last().map(String::as_str), Some(failed_command));
        }
    }

    #[test]
    fn shared_track_change_that_stops_is_an_error_without_recovery() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_track_change(
            &mut engine,
            "next",
            true,
            vec![
                seek_status("play", 41.0),
                Ok(vec![]),
                seek_status("stop", 0.0),
                seek_status("stop", 0.0),
            ],
        );

        assert!(result.unwrap_err().contains("parou inesperadamente"));
        assert!(!commands.iter().any(|candidate| candidate.starts_with("pause")));
    }

    #[test]
    fn shared_track_change_rejects_invalid_elapsed() {
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_track_change(
            &mut engine,
            "next",
            true,
            vec![
                seek_status("play", 41.0),
                Ok(vec![]),
                seek_status("play", f64::NAN),
            ],
        );

        assert!(result.unwrap_err().contains("elapsed inválido"));
        assert_eq!(commands, vec!["status", "next", "status"]);
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
    fn seek_rejection_preserves_ack_without_recovery_or_queue_mutation() {
        for shared in [false, true] {
            let mut engine = engine_with_track("local/fixture.flac");
            let before = serde_json::to_string(&engine.queue).unwrap();
            let mut responses = Vec::new();
            if shared {
                responses.push(seek_status("play", 57.4));
            }
            responses.push(Err("ACK [50@0] {seekcur} Not seekable".into()));
            let (result, commands, sleeps) = scripted_seek(&mut engine, 62.5, shared, responses);
            assert_eq!(result.unwrap_err(), "ACK [50@0] {seekcur} Not seekable");
            assert_eq!(
                commands
                    .iter()
                    .filter(|cmd| cmd.starts_with("seekcur"))
                    .count(),
                1
            );
            assert!(!commands
                .iter()
                .any(|cmd| cmd.starts_with("pause") || cmd == "play" || cmd == "clearerror"));
            assert_eq!(serde_json::to_string(&engine.queue).unwrap(), before);
            assert!(sleeps.is_empty());
        }
    }

    #[test]
    fn seek_stopped_shared_refusal_and_invalid_target_never_force_playback() {
        let mut engine = engine_with_track("local/fixture.flac");
        let (result, commands, sleeps) = scripted_seek(
            &mut engine,
            62.5,
            true,
            vec![
                seek_status("stop", 0.0),
                Err("ACK [55@0] {seekcur} Not playing".into()),
            ],
        );
        assert!(result.is_err());
        assert_eq!(commands, vec!["status", "seekcur 62.5"]);
        assert!(sleeps.is_empty());
        for invalid in [f64::NAN, f64::INFINITY, -1.0] {
            let (result, commands, sleeps) = scripted_seek(&mut engine, invalid, true, vec![]);
            assert!(result.is_err());
            assert!(commands.is_empty());
            assert!(sleeps.is_empty());
        }
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

    fn seek_recovery_fixture(
        target: f64,
        early: [f64; 2],
        tail: f64,
    ) -> Vec<Result<Vec<String>, String>> {
        vec![
            seek_status("play", 11.337),
            Ok(vec![]),
            seek_status("play", early[0]),
            seek_status("play", early[1]),
            seek_status("play", tail),
            seek_status("play", tail),
            seek_status("play", tail),
            Ok(vec![]),
            seek_status("pause", tail),
            Ok(vec![]),
            seek_status("play", target + 0.935),
            seek_status("play", target + 1.574),
            seek_status("play", target + 2.075),
            seek_status("play", target + 3.079),
        ]
    }

    #[test]
    fn shared_playing_seek_with_progress_needs_no_recovery() {
        // Healthy/irregular/coarse updates: early samples alone never confirm.
        for samples in [[113.98, 114.06, 114.5, 114.9], [114.0, 114.0, 114.0, 114.5]] {
            let mut engine = engine_with_track("local/faixa.flac");
            let mut script = vec![seek_status("play", 57.4), Ok(vec![])];
            script.extend(samples.into_iter().map(|value| seek_status("play", value)));
            let (result, commands, sleeps) = scripted_seek(&mut engine, 114.0, true, script);
            result.unwrap();
            assert_eq!(
                commands,
                [
                    "status",
                    "seekcur 114.0",
                    "status",
                    "status",
                    "status",
                    "status"
                ]
            );
            assert_eq!(sleeps.iter().sum::<Duration>(), Duration::from_millis(960));
        }
    }

    #[test]
    fn shared_playing_frozen_seek_recovers_once_with_pause_resume() {
        let mut engine = engine_with_track("local/faixa.flac");
        let before = engine.queue.clone();
        let (result, commands, sleeps) = scripted_seek(
            &mut engine,
            114.0,
            true,
            seek_recovery_fixture(114.0, [113.98, 113.98], 113.98),
        );
        result.unwrap();
        assert_eq!(
            commands,
            [
                "status",
                "seekcur 114.0",
                "status",
                "status",
                "status",
                "status",
                "status",
                "pause 1",
                "status",
                "pause 0",
                "status",
                "status",
                "status",
                "status"
            ]
        );
        assert_eq!(
            commands
                .iter()
                .filter(|cmd| cmd.as_str() == "pause 1")
                .count(),
            1
        );
        assert_eq!(
            commands
                .iter()
                .filter(|cmd| cmd.as_str() == "status")
                .count(),
            11
        );
        assert_eq!(sleeps.iter().sum::<Duration>(), Duration::from_millis(1840));
        assert_eq!(engine.queue, before);
    }

    #[test]
    fn shared_seek_received_sequences_recover_once_and_require_tail_continuity() {
        // Received early/tail data; songid and the successful post-recovery
        // continuation are synthetic. Only player1's recovery was observed.
        for (target, early, tail) in [
            (43.0, [43.456, 43.456], 43.456),
            (106.5, [106.568, 106.659], 106.956),
            (133.0, [133.022, 133.091], 133.456),
        ] {
            let mut engine = engine_with_track("local/fixture.flac");
            let (result, commands, _) = scripted_seek(
                &mut engine,
                target,
                true,
                seek_recovery_fixture(target, early, tail),
            );
            result.unwrap();
            assert_eq!(
                commands
                    .iter()
                    .filter(|cmd| cmd.starts_with("seekcur"))
                    .count(),
                1
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|cmd| cmd.as_str() == "pause 1")
                    .count(),
                1
            );
            assert_eq!(
                commands
                    .iter()
                    .filter(|cmd| cmd.as_str() == "pause 0")
                    .count(),
                1
            );
            assert!(!commands
                .iter()
                .any(|cmd| cmd == "play" || cmd == "clearerror" || cmd.contains("output")));
        }
    }

    #[test]
    fn shared_seek_recovery_failures_are_explicit_and_bounded() {
        for (index, expected_command) in [(7, "pause 1"), (9, "pause 0")] {
            let mut script = seek_recovery_fixture(114.0, [113.98, 113.98], 113.98);
            script[index] = Err("ACK [5@0] {pause} failure".into());
            script.truncate(index + 1);
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, script);
            assert!(result.unwrap_err().contains("ACK [5@0]"));
            assert_eq!(commands.last().map(String::as_str), Some(expected_command));
        }
        // A second brief advance followed by a frozen tail is failure, not a
        // second recovery or an OK based only on the recovery's first 80 ms.
        let mut script = seek_recovery_fixture(114.0, [113.98, 113.98], 113.98);
        script[12] = seek_status("play", 115.6);
        script[13] = seek_status("play", 115.6);
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, script);
        assert!(result.unwrap_err().contains("permaneceu congelada"));
        assert_eq!(
            commands
                .iter()
                .filter(|cmd| cmd.as_str() == "pause 1")
                .count(),
            1
        );
        // The existing recovery's own failed progress check remains explicit.
        let mut script = seek_recovery_fixture(114.0, [113.98, 113.98], 113.98);
        script[11] = script[10].clone();
        script.truncate(12);
        let (result, _, _) = scripted_seek(&mut engine, 114.0, true, script);
        assert!(result.unwrap_err().contains("permaneceu congelada"));
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
            ],
        );
        assert!(result.unwrap_err().contains("parou inesperadamente"));
        assert!(!commands.iter().any(|cmd| cmd.starts_with("pause")));
    }

    #[test]
    fn shared_seek_state_identity_and_error_changes_never_authorize_resume() {
        for index in [2, 3, 4, 5, 6, 8, 10, 11, 12, 13] {
            for change in [
                "pause",
                "play",
                "stop",
                "songid",
                "error",
                "missing_id",
                "position",
            ] {
                let mut script = seek_recovery_fixture(114.0, [113.98, 113.98], 113.98);
                let mut lines = script[index].clone().unwrap();
                match change {
                    "pause" => lines[0] = "state: pause".into(),
                    "play" => lines[0] = "state: play".into(),
                    "stop" => lines[0] = "state: stop".into(),
                    "songid" => lines[2] = "songid: 99".into(),
                    "error" => lines.push("error: synthetic failure".into()),
                    "missing_id" => {
                        lines.pop();
                    }
                    "position" => lines[1] = "elapsed: 10.0".into(),
                    _ => unreachable!(),
                }
                // pause is the expected state immediately before our pause 0.
                if index == 8 && change == "pause" {
                    continue;
                }
                if change == "play" && index != 8 {
                    continue;
                }
                // Position checking compares adjacent samples: the first seek
                // sample is compared to the target, the tail to its predecessor.
                if change == "position" && index == 2 {
                    continue;
                }
                script[index] = Ok(lines);
                script.truncate(index + 1);
                let mut engine = engine_with_track("local/faixa.flac");
                let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, script);
                assert!(result.is_err(), "index={index} change={change}");
                if index <= 6 {
                    assert!(!commands.iter().any(|cmd| cmd.starts_with("pause")));
                }
                if index == 8 {
                    assert!(!commands.iter().any(|cmd| cmd == "pause 0"));
                }
                assert!(
                    commands
                        .iter()
                        .filter(|cmd| cmd.as_str() == "pause 1")
                        .count()
                        <= 1
                );
                assert!(
                    commands
                        .iter()
                        .filter(|cmd| cmd.as_str() == "pause 0")
                        .count()
                        <= 1
                );
            }
        }
    }

    #[test]
    fn shared_seek_incomplete_error_or_changed_position_is_not_recovered() {
        for lines in [
            vec!["state: play".into(), "elapsed: 1.0".into()],
            vec![
                "state: play".into(),
                "elapsed: 1.0".into(),
                "songid: 42".into(),
                "error: synthetic".into(),
            ],
        ] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, vec![Ok(lines)]);
            assert!(result.is_err());
            assert_eq!(commands, ["status"]);
        }
        for samples in [[90.0, 90.1], [114.1, 114.0]] {
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, _) = scripted_seek(
                &mut engine,
                114.0,
                true,
                vec![
                    seek_status("play", 57.4),
                    Ok(vec![]),
                    seek_status("play", samples[0]),
                    seek_status("play", samples[1]),
                ],
            );
            assert!(result.is_err());
            assert!(!commands.iter().any(|cmd| cmd.starts_with("pause")));
        }
    }

    #[test]
    fn shared_seek_io_failures_do_not_become_success_or_retries() {
        for index in [2, 3, 4, 5, 6, 8, 10, 11, 12, 13] {
            let mut script = seek_recovery_fixture(114.0, [113.98, 113.98], 113.98);
            script[index] = Err("timeout fixture".into());
            script.truncate(index + 1);
            let mut engine = engine_with_track("local/faixa.flac");
            let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, script);
            assert!(result.unwrap_err().contains("timeout fixture"));
            assert_eq!(commands.len(), index + 1);
        }
    }

    #[test]
    fn shared_seek_deadline_is_checked_for_commands_and_buffered_responses() {
        let started = Instant::now();
        assert_eq!(
            AudioEngine::seek_deadline(started, true, &seek_status("play", 1.0)),
            Some(started + SHARED_SEEK_BUDGET)
        );
        for state in ["pause", "stop"] {
            assert_eq!(
                AudioEngine::seek_deadline(started, true, &seek_status(state, 1.0)),
                None
            );
        }
        assert_eq!(
            AudioEngine::seek_deadline(started, false, &seek_status("play", 1.0)),
            None
        );
        assert_eq!(
            AudioEngine::seek_deadline(started, true, &Err("timeout fixture".into())),
            None
        );
        let expired = Some(Instant::now() - Duration::from_secs(1));
        assert!(mpd_deadline_timeout(expired).is_err());
        assert_eq!(
            mpd_deadline_timeout(None).unwrap(),
            Duration::from_millis(500)
        );
        assert!(
            mpd_deadline_timeout(Some(Instant::now() + Duration::from_millis(20))).unwrap()
                <= Duration::from_millis(20)
        );
        let mut reader = std::io::Cursor::new(b"state: play\nOK\n");
        assert!(AudioEngine::read_mpd_response_before(&mut reader, expired)
            .unwrap_err()
            .contains("Prazo"));
        assert_eq!(reader.position(), 0);
        assert_eq!(
            AudioEngine::read_mpd_response(&mut reader).unwrap(),
            ["state: play"]
        );
        assert!(
            AudioEngine::send_command_to_socket_before("/missing/socket", "pause 0", expired)
                .unwrap_err()
                .contains("Prazo")
        );
    }

    #[test]
    fn shared_seek_expired_budget_before_resume_does_not_unpause() {
        let mut script = seek_recovery_fixture(114.0, [113.98, 113.98], 113.98);
        script[8] = Err("Prazo de confirmação do seek Shared esgotado.".into());
        script.truncate(9);
        let mut engine = engine_with_track("local/faixa.flac");
        let (result, commands, _) = scripted_seek(&mut engine, 114.0, true, script);
        assert!(result.unwrap_err().contains("Prazo"));
        assert_eq!(
            commands
                .iter()
                .filter(|cmd| cmd.as_str() == "pause 1")
                .count(),
            1
        );
        assert!(!commands.iter().any(|cmd| cmd == "pause 0" || cmd == "play"));
    }

    #[test]
    fn shared_seek_restored_same_position_never_contacts_mpd() {
        let mut engine = engine_with_track("local/faixa.flac");
        engine.restored_resume = Some(PlaybackResumeSnapshot {
            queue_index: 0,
            elapsed: 114.0,
            state: PlaybackState::Paused,
        });
        let before = engine.restored_resume;
        let (result, commands, sleeps) = scripted_seek(&mut engine, 114.0, true, vec![]);
        result.unwrap();
        assert!(commands.is_empty());
        assert!(sleeps.is_empty());
        assert_eq!(engine.restored_resume, before);
    }

    #[test]
    fn next_previous_and_queue_click_discard_restored_cursor() {
        for command in ["next", "previous", "play 0"] {
            let (socket, server) = fake_album_mpd(vec![(command, "OK\n")]);
            let mut engine = engine_with_track("local/faixa.flac");
            engine.socket_path = socket;
            engine.restored_resume = Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 9.0, state: PlaybackState::Paused });
            match command {
                "next" => engine.next(false).unwrap(),
                "previous" => engine.previous(false).unwrap(),
                _ => engine.play_index(0, false).unwrap(),
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
        };
        let status = PlaybackStatus {
            snapshot_consistent: true,
            processing: None,
            options: None,
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
            playback_error: None,
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
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
    fn eq_restore_partial_batch_marks_actual_queue_uncertain_without_resume(){
        let (path,server)=fake_album_mpd(vec![("command_list_begin","ACK [50@1] {add} fixture refused\n")]);let mut e=engine_with_track("fixture.wav");e.socket_path=path;let r=e.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();assert!(e.restore_eq_switch(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Playing}),&r).is_err());assert!(e.ensure_queue_coherent().is_err());server.join().unwrap();
    }

    #[test]
    fn eq_resume_actual_caller_requires_confirmed_cursor_queue_and_playing_origin(){
        let pause="playlist: 9\nplaylistlength: 1\nsong: 0\nelapsed: 17.000\nstate: pause\nOK\n";
        let play="playlist: 9\nplaylistlength: 1\nsong: 0\nelapsed: 17.001\nstate: play\nOK\n";
        let (path,server)=fake_album_mpd(vec![("status",pause),("status",pause),("playlistinfo","file: fixture.wav\nPos: 0\nId: 7\nOK\n"),("status",pause),("pause 0","OK\n"),("status",play)]);
        let mut engine=engine_with_track("fixture.wav");engine.socket_path=path;
        let resolved=engine.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();
        let watch=engine.watch_eq_transition();let saved=Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Playing});
        assert_eq!(engine.resume_eq_switch(saved,&resolved,&watch).unwrap(),"resumed");server.join().unwrap();
        for origin in [PlaybackState::Paused,PlaybackState::Stopped]{let mut e=engine_with_track("fixture.wav");let r=e.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();let w=e.watch_eq_transition();assert_eq!(e.resume_eq_switch(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:origin}),&r,&w).unwrap(),if origin==PlaybackState::Paused{"paused"}else{"stopped"});}
    }
    #[test]
    fn route_save_successive_save_preserves_logical_paused_cursor_without_resume(){
        let(path,server)=fake_album_mpd(vec![("status","playlist: 9\nplaylistlength: 2\nstate: stop\nOK\n")]);
        let mut engine=engine_with_track("fixture.wav");engine.queue.push(engine.queue[0].clone());engine.socket_path=path;engine.restored_resume=Some(PlaybackResumeSnapshot{queue_index:1,elapsed:17.,state:PlaybackState::Paused});
        let resolved=engine.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into();2]).unwrap();
        let saved=engine.prepare_reserved_route_save(&resolved).unwrap().unwrap();assert_eq!(saved.state,PlaybackState::Paused);assert_eq!(saved.queue_index,1);assert_eq!(saved.elapsed,17.);server.join().unwrap();
    }

    #[test]
    fn route_save_staging_has_no_play_or_seek_unlike_previous_restore() {
        for legacy in [true,false] {
            let id=ALBUM_TEST_ID.fetch_add(1,Ordering::Relaxed);let path=std::env::temp_dir().join(format!("route-stage-{}-{id}.sock",std::process::id()));let listener=UnixListener::bind(&path).unwrap();
            let server=thread::spawn(move||{
                let mut sent=Vec::new();
                for _ in 0..if legacy{2}else{1} {
                    let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();stream.write_all(b"OK MPD 0.24.15\n").unwrap();
                    let mut reader=BufReader::new(stream.try_clone().unwrap());let mut first=String::new();reader.read_line(&mut first).unwrap();sent.push(first.trim().to_string());
                    if first=="command_list_begin\n" {loop{let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0);sent.push(line.trim().to_string());if line=="command_list_end\n"{break}}}
                    stream.write_all(b"OK\n").unwrap();
                }
                std::fs::remove_file(path).unwrap();sent
            });
            let mut engine=engine_with_track("fixture.wav");engine.queue.push(engine.queue[0].clone());engine.socket_path=std::env::temp_dir().join(format!("route-stage-{}-{id}.sock",std::process::id())).to_string_lossy().into_owned();
            let resolved=engine.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into();2]).unwrap();let saved=Some(DeviceSwitchSnapshot{queue_index:1,elapsed:0.,state:PlaybackState::Playing});
            if legacy {engine.restore_after_reserved_device_switch(saved,&resolved).unwrap();}else{engine.restore_for_route_save(saved,&resolved).unwrap();assert_eq!(engine.restored_resume,Some(PlaybackResumeSnapshot{queue_index:1,elapsed:0.,state:PlaybackState::Paused}));}
            let sent=server.join().unwrap();assert_eq!(sent.iter().any(|c|c.starts_with("play ")),legacy);if !legacy {assert!(!sent.iter().any(|c|c.starts_with("seek")||c.starts_with("pause")));}assert_eq!(sent.iter().filter(|c|c.starts_with("add ")).count(),2);
        }
    }

    #[test]
    fn route_save_intervention_epoch_can_revoke_while_audio_mutex_is_held() {
        let state=AudioState::new(engine_with_track("fixture.wav"));
        let audio=state.0.lock().unwrap();let watch=audio.watch_eq_transition();
        state.note_transport_intervention();assert!(!watch.current());drop(audio);
    }
    #[test]
    fn route_save_shared_resume_reuses_verified_queue_cursor_and_owner() {
        let pause="playlist: 9\nplaylistlength: 1\nstate: stop\nOK\n";
        let play="playlist: 9\nplaylistlength: 1\nsong: 0\nelapsed: 17.001\nstate: play\nOK\n";
        let(path,server)=fake_album_mpd(vec![("status",pause),("status",pause),("playlistinfo","file: fixture.wav\nPos: 0\nId: 7\nOK\n"),("status",pause),("status",pause),("playlistinfo","file: fixture.wav\nPos: 0\nId: 7\nOK\n"),("status",pause),("status",pause),("seek 0 17.000","OK\n"),("status",play)]);
        let mut engine=engine_with_track("fixture.wav");engine.socket_path=path;
        let operations=Arc::new(crate::config_save_operation::Coordinator::new());
        let gate=Arc::new(crate::direct_playback::Gate::new(operations.clone()));let cfg=crate::config::AppConfig::default();gate.bind(&cfg,1,None);engine.attach_direct_playback(gate);
        let resolved=engine.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();let watch=engine.watch_eq_transition();engine.restored_resume=Some(PlaybackResumeSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Paused});let owner=operations.begin().unwrap();
        assert_eq!(engine.resume_saved_route(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Playing}),&resolved,&watch,&owner,|_|Ok(())).unwrap(),"resumed");server.join().unwrap();
        for origin in [PlaybackState::Paused,PlaybackState::Stopped] {
            assert_eq!(engine.resume_saved_route(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:origin}),&resolved,&watch,&owner,|_|Ok(())).unwrap(),if origin==PlaybackState::Paused{"paused"}else{"stopped"});
        }
    }

    #[test]
    fn eq_resume_human_transport_callers_revoke_even_if_transport_refused(){
        for action in 0..8{let mut e=engine_with_track("fixture.wav");let r=e.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();let w=e.watch_eq_transition();
            let _=match action{0=>e.pause(),1=>e.seek(1.,true),2=>e.next(true),3=>e.previous(true),4=>e.play(true),5=>e.toggle_play_pause(true),6=>e.play_index(0,true),_=>e.clear_queue()};
            assert!(!w.current(),"action {action}");assert_eq!(e.resume_eq_switch(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Playing}),&r,&w).unwrap(),"superseded");
        }
    }
    #[test]
    fn eq_resume_uncertain_or_changed_queue_never_reads_or_plays(){
        for uncertain in [false,true]{let mut e=engine_with_track("fixture.wav");let r=e.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();let w=e.watch_eq_transition();if uncertain{e.mark_queue_uncertain()}else{e.queue[0].uri="other.wav".into()}
            assert!(e.resume_eq_switch(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Playing}),&r,&w).is_err());}
    }
    #[test]
    fn eq_resume_position_change_refuses_before_pause_zero(){
        let (path,server)=fake_album_mpd(vec![("status","playlist: 9\nplaylistlength: 1\nsong: 0\nelapsed: 40\nstate: pause\nOK\n")]);let mut e=engine_with_track("fixture.wav");e.socket_path=path;let r=e.capture_device_switch_queue().unwrap().bind_resolved_uris(vec!["fixture.wav".into()]).unwrap();assert_eq!(e.resume_eq_switch(Some(DeviceSwitchSnapshot{queue_index:0,elapsed:17.,state:PlaybackState::Playing}),&r,&e.watch_eq_transition()).unwrap_err(),"EQ_RESUME_CONTEXT_CHANGED");server.join().unwrap();
    }

    #[test]
    fn device_switch_rejects_same_length_queue_replaced_during_uri_resolution() {
        let audio = Arc::new(Mutex::new(engine_with_track("original.flac")));
        let captured = audio.lock().unwrap().capture_device_switch_queue().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let resolver = thread::spawn(move || {
            started_tx.send(()).unwrap();
            continue_rx.recv().unwrap();
            let uris = captured
                .tracks()
                .iter()
                .map(|track| track.uri.clone())
                .collect::<Vec<_>>();
            captured.bind_resolved_uris(uris).unwrap()
        });
        started_rx.recv().unwrap();
        audio.lock().unwrap().queue = engine_with_track("replacement.flac").get_queue();
        continue_tx.send(()).unwrap();
        let resolved = resolver.join().unwrap();
        assert!(audio
            .lock()
            .unwrap()
            .prepare_reserved_device_switch(&resolved)
            .unwrap_err()
            .snapshot
            .is_none());
        let mut commands = Vec::new();
        let result = audio
            .lock()
            .unwrap()
            .restore_after_reserved_device_switch_with(
                None,
                &resolved,
                &mut |command| {
                    commands.push(command.to_string());
                    Ok(Vec::new())
                },
                &mut |_| {},
            );
        assert!(
            result.is_err(),
            "A same-length replacement must invalidate resolved URIs"
        );
        assert!(commands.is_empty(), "Refusal must precede MPD effects");
    }

    #[test]
    fn device_switch_rejects_changed_length_or_identity_before_mpd_io() {
        for change in 0..3 {
            let mut engine = engine_with_two_tracks();
            let captured = engine.capture_device_switch_queue().unwrap();
            let uris = captured
                .tracks()
                .iter()
                .map(|track| track.uri.clone())
                .collect();
            let resolved = captured.bind_resolved_uris(uris).unwrap();
            match change {
                0 => {
                    engine.queue.pop();
                }
                1 => {
                    engine.queue.push(engine.queue[0].clone());
                }
                _ => {
                    engine.queue.swap(0, 1);
                }
            }
            let error = engine
                .prepare_reserved_device_switch(&resolved)
                .unwrap_err();
            assert!(error.snapshot.is_none());
            assert!(engine
                .restore_after_reserved_device_switch_with(
                    None,
                    &resolved,
                    &mut |_| panic!("stale queue must not reach MPD"),
                    &mut |_| {},
                )
                .is_err());
            assert!(engine.reserve_queue_structure().is_none());
            drop(resolved);
            assert!(engine.reserve_queue_structure().is_some());
        }
    }

    #[test]
    fn device_switch_binding_failure_releases_without_changing_queue() {
        for uris in [vec![], vec!["".into()], vec!["bad\nuri".into()]] {
            let mut engine = engine_with_track("valid.flac");
            let before = engine.get_queue();
            let captured = engine.capture_device_switch_queue().unwrap();
            assert!(captured.bind_resolved_uris(uris).is_err());
            assert_eq!(engine.get_queue(), before);
            assert!(engine.reserve_queue_structure().is_some());
        }
        let mut engine = engine_with_track("valid.flac");
        let captured = engine.capture_device_switch_queue().unwrap();
        let resolved = captured
            .bind_resolved_uris(vec!["valid.flac".into()])
            .unwrap();
        let foreign = engine_with_track("valid.flac");
        assert!(foreign
            .restore_after_reserved_device_switch_with(
                None,
                &resolved,
                &mut |_| panic!("foreign owner must not reach MPD"),
                &mut |_| {},
            )
            .is_err());
    }

    #[test]
    fn device_switch_reserved_restore_preserves_play_pause_stop_and_duplicate_cue() {
        for state in [
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Stopped,
        ] {
            let mut engine = engine_with_track("Álbum/imagem.CUE/track0001");
            engine.queue.push(engine.queue[0].clone());
            let before = engine.get_queue();
            let captured = engine.capture_device_switch_queue().unwrap();
            let resolved = captured
                .bind_resolved_uris(vec![before[0].uri.clone(); 2])
                .unwrap();
            let mpd_state = match state {
                PlaybackState::Playing => "play",
                PlaybackState::Paused => "pause",
                PlaybackState::Stopped => "stop",
            };
            let mut script = vec![(
                "status".into(),
                Some(format!("state: {mpd_state}\nsong: 1\nelapsed: 9\nOK\n")),
            )];
            if state != PlaybackState::Stopped {
                script.push(("stop".into(), Some("OK\n".into())));
            }
            let selection = if state == PlaybackState::Stopped {
                ""
            } else {
                "play 1\n"
            };
            script.push((format!("command_list_begin\nclear\nadd \"Álbum/imagem.CUE/track0001\"\nadd \"Álbum/imagem.CUE/track0001\"\n{selection}command_list_end"), Some("OK\n".into())));
            if state != PlaybackState::Stopped {
                script.extend([
                    ("seekcur 9.0".into(), Some("OK\n".into())),
                    ("pause 1".into(), Some("OK\n".into())),
                ]);
            }
            let (socket, server) = fake_mpd_script(script);
            engine.socket_path = socket;
            let playback = engine.prepare_reserved_device_switch(&resolved).unwrap();
            assert_eq!(playback.unwrap().state, state);
            assert!(engine.clear_queue().is_err());
            engine
                .restore_after_reserved_device_switch(playback, &resolved)
                .unwrap();
            assert_eq!(engine.get_queue(), before);
            assert!(engine.reserve_queue_structure().is_none());
            drop(resolved);
            assert!(engine.reserve_queue_structure().is_some());
            server.join().unwrap();
        }
    }

    #[test]
    fn device_switch_empty_queue_still_holds_and_validates_reservation() {
        let mut engine = engine_with_track("unused");
        engine.queue.clear();
        let captured = engine.capture_device_switch_queue().unwrap();
        let resolved = captured.bind_resolved_uris(vec![]).unwrap();
        let (socket, server) =
            fake_mpd_script(vec![("status".into(), Some("state: stop\nOK\n".into()))]);
        engine.socket_path = socket;
        assert!(engine
            .prepare_reserved_device_switch(&resolved)
            .unwrap()
            .is_none());
        engine
            .restore_after_reserved_device_switch(None, &resolved)
            .unwrap();
        assert!(engine.play_uris(vec!["new.flac".into()], 0).is_err());
        assert!(engine.reserve_queue_structure().is_none());
        drop(resolved);
        assert!(engine.reserve_queue_structure().is_some());
        server.join().unwrap();
    }

    #[test]
    fn device_switch_rollback_retains_owner_after_intermediate_mpd_failure() {
        let mut engine = engine_with_track("valid.flac");
        let before = engine.get_queue();
        let captured = engine.capture_device_switch_queue().unwrap();
        let resolved = captured
            .bind_resolved_uris(vec!["valid.flac".into()])
            .unwrap();
        let playback = Some(DeviceSwitchSnapshot {
            queue_index: 0,
            elapsed: 0.0,
            state: PlaybackState::Paused,
        });
        let batch = "command_list_begin\nclear\nadd \"valid.flac\"\nplay 0\ncommand_list_end";
        let (socket, server) = fake_mpd_script(vec![
            (batch.into(), Some("ACK [50@0] {add} rejected\n".into())),
            (batch.into(), Some("OK\n".into())),
            ("pause 1".into(), Some("OK\n".into())),
        ]);
        engine.socket_path = socket;
        assert!(engine
            .restore_after_reserved_device_switch(playback, &resolved)
            .is_err());
        assert!(engine.clear_queue().is_err());
        assert!(engine.reserve_queue_structure().is_none());
        // Rollback is allowed to use the same identity-bound capability.
        engine
            .restore_after_reserved_device_switch(playback, &resolved)
            .unwrap();
        assert_eq!(engine.get_queue(), before);
        assert!(engine.reserve_queue_structure().is_none());
        drop(resolved);
        assert!(engine.reserve_queue_structure().is_some());
        server.join().unwrap();
    }

    #[test]
    fn device_switch_failed_output_start_rolls_back_bound_queue_under_reservation() {
        for state in [
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Stopped,
        ] {
            for rollback_starts in [true, false] {
                let mut engine = engine_with_track("valid.flac");
                let before = engine.get_queue();
                let captured = engine.capture_device_switch_queue().unwrap();
                let resolved = captured
                    .bind_resolved_uris(vec!["valid.flac".into()])
                    .unwrap();
                let mpd_state = match state {
                    PlaybackState::Playing => "play",
                    PlaybackState::Paused => "pause",
                    PlaybackState::Stopped => "stop",
                };
                let mut script = vec![(
                    "status".into(),
                    Some(format!("state: {mpd_state}\nsong: 0\nelapsed: 0\nOK\n")),
                )];
                if state != PlaybackState::Stopped {
                    script.push(("stop".into(), Some("OK\n".into())));
                }
                if rollback_starts {
                    // Rollback must rebuild without selecting/starting a song.
                    script.push(("command_list_begin\nclear\nadd \"valid.flac\"\ncommand_list_end".into(), Some("OK\n".into())));
                }
                let (socket, server) = fake_mpd_script(script);
                engine.socket_path = socket;
                let playback = engine.prepare_reserved_device_switch(&resolved).unwrap();
                let audio = Mutex::new(engine);
                let error = crate::confirm_device_switch_start(
                    Err("simulated output startup failure".into()),
                    || {
                        crate::rollback_audio_switch_with(playback, &resolved, &audio, || {
                            let mut engine = audio
                                .try_lock()
                                .expect("restart must not retain AudioState");
                            assert!(engine.clear_queue().is_err());
                            assert!(engine.reserve_queue_structure().is_none());
                            if rollback_starts {
                                Ok(())
                            } else {
                                Err("simulated rollback startup failure".into())
                            }
                        })
                    },
                )
                .unwrap_err();
                assert!(error.contains("simulated output startup failure"));
                assert_eq!(
                    error.ends_with("Estado anterior restaurado."),
                    rollback_starts
                );
                assert_eq!(error.contains("Rollback também falhou"), !rollback_starts);
                let mut engine = audio.lock().unwrap();
                assert_eq!(engine.queue, before);
                assert_eq!(engine.get_queue().is_empty(), !rollback_starts);
                assert_eq!(engine.queue_coherent, rollback_starts);
                assert_eq!(engine.restored_resume, if rollback_starts && state != PlaybackState::Stopped {
                    Some(PlaybackResumeSnapshot { queue_index: 0, elapsed: 0.0, state: PlaybackState::Paused })
                } else { None });
                assert!(engine.reserve_queue_structure().is_none());
                drop(resolved);
                assert_eq!(engine.queue_structure_reserved.load(Ordering::SeqCst), 0);
                assert!(engine.admit_playback_intent().is_ok());
                assert_eq!(engine.reserve_queue_structure().is_some(), rollback_starts);
                server.join().unwrap();
            }
        }
    }

    #[test]
    fn device_switch_resolution_failure_releases_owner_before_preparation() {
        let mut engine = engine_with_track("");
        engine.queue[0] = plex_track();
        let before = engine.get_queue();
        let audio = Mutex::new(engine);
        let client = crate::plex::PlexClient::from_config(&crate::config::AppConfig::default());
        assert!(
            tauri::async_runtime::block_on(crate::resolve_device_switch_queue(&audio, &client))
                .is_err()
        );
        let mut engine = audio.lock().unwrap();
        assert_eq!(engine.get_queue(), before);
        assert!(engine.reserve_queue_structure().is_some());
    }

    #[test]
    fn device_switch_delayed_plex_resolution_keeps_audio_available_and_shutdown_serialized() {
        use std::net::TcpListener;
        use std::sync::mpsc::channel;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = crate::config::AppConfig {
            plex_url: format!("http://{}", listener.local_addr().unwrap()),
            plex_token: "FIXTURE_TOKEN".into(),
            plex_server_id: Some("server-1".into()),
            ..crate::config::AppConfig::default()
        };
        let client = crate::plex::PlexClient::playback_client_for_test(&config);
        let (reading_tx, reading_rx) = channel();
        let (response_tx, response_rx) = channel();
        let server = thread::spawn(move || {
            for request in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with(if request == 0 {
                    "GET /identity "
                } else {
                    "GET /library/sections "
                }));
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                }
                if request == 0 {
                    reading_tx.send(()).unwrap();
                    response_rx.recv().unwrap();
                }
                let body = if request == 0 {
                    r#"{"MediaContainer":{"machineIdentifier":"server-1"}}"#
                } else {
                    r#"{"MediaContainer":{}}"#
                };
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            }
        });
        let mut engine = engine_with_track("");
        engine.queue[0] = plex_track();
        let audio = Arc::new(Mutex::new(engine));
        let transactions = Arc::new(crate::ConfigTransactionState::new());
        let worker_audio = Arc::clone(&audio);
        let worker_transactions = Arc::clone(&transactions);
        let (resolved_tx, resolved_rx) = channel();
        let (release_tx, release_rx) = channel();
        let worker = thread::spawn(move || {
            let _transaction = worker_transactions.begin().unwrap();
            let resolved = tauri::async_runtime::block_on(crate::resolve_device_switch_queue(
                &worker_audio,
                &client,
            ))
            .unwrap();
            resolved_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(resolved);
        });
        reading_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        {
            let mut engine = audio
                .try_lock()
                .expect("Plex I/O must not retain AudioState");
            assert_eq!(
                engine.get_queue()[0].media_locator,
                plex_track().media_locator
            );
            assert!(engine.clear_queue().is_err());
            assert!(engine
                .play_tracks(vec![plex_track()], vec!["other.flac".into()], 0)
                .is_err());
            assert!(engine.play_uris(vec!["other.flac".into()], 0).is_err());
        }
        transactions.request_shutdown();
        assert!(transactions.begin().is_err());
        let shutdown_transactions = Arc::clone(&transactions);
        let (shutdown_tx, shutdown_rx) = channel();
        let shutdown = thread::spawn(move || {
            let _shutdown = shutdown_transactions.wait_for_active_transaction();
            shutdown_tx.send(()).unwrap();
        });
        assert!(shutdown_rx.try_recv().is_err());
        response_tx.send(()).unwrap();
        resolved_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(audio.lock().unwrap().reserve_queue_structure().is_none());
        assert!(shutdown_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        shutdown_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        shutdown.join().unwrap();
        server.join().unwrap();
        assert!(audio.lock().unwrap().reserve_queue_structure().is_some());
    }

    #[test]
    fn queue_restore_uses_fresh_uri_without_mutating_stable_plex_queue() {
        let engine = AudioEngine {
            socket_path: String::new(),
            music_dir: String::new(),
            queue: vec![plex_track()],
            restored_resume: None,
            startup_restore: StartupQueueRestore::Ready,
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
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
    ) -> (Result<DeviceSwitchRestoreOutcome, String>, Vec<String>) {
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
            queue_structure_reserved: Arc::new(AtomicU8::new(0)),
            queue_coherent: true, cache_persistence_pending: false, playback_intent: 0, eq_intervention:Arc::new(AtomicU64::new(0)), direct_playback:None,
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

    #[test]
    fn catalog_tag_presence_raw_values_precede_fallback_and_transformations() {
        let map = AudioEngine::canonical_local_album_collectors(collect_album_pages(vec![album_test_lines(&[
            "file: Synthetic/Fallback/one.flac",
            "file: Synthetic/Blank/two.flac", "Album:   ", "Artist: ", "AlbumArtist:   ", "Date:    ", "Genre:   ",
            "file: Synthetic/Raw/three.flac", "Album: Raw", "Artist: A", "AlbumArtist: A", "Date:     2004", "Genre: bad\nvalue",
        ])]));
        let fallback = &map[&album_id("Synthetic/Fallback", "Fallback")];
        assert_eq!(fallback.title, "Fallback");
        assert_eq!(fallback.display_artist(), "Artista Desconhecido");
        assert_eq!(fallback.tag_presence.evaluated_track_count, 1);
        assert_eq!(fallback.tag_presence.missing, LocalAlbumMissingTags { album: 1, artist: 1, album_artist: 1, date: 1, genre: 1 });
        let blank = &map[&album_id("Synthetic/Blank", "   ")];
        assert_eq!(blank.title, "  ");
        assert_eq!(blank.year.as_deref(), Some("   "));
        assert_eq!(blank.tag_presence, fallback.tag_presence);
        let raw = &map[&album_id("Synthetic/Raw", "Raw")];
        assert_eq!(raw.year.as_deref(), Some("    "));
        assert!(raw.genres.is_empty());
        assert_eq!(raw.tag_presence.missing, LocalAlbumMissingTags::default());
    }

    #[test]
    fn catalog_tag_presence_counts_each_field_once_per_accepted_track_and_resets_pages() {
        let collection = collect_album_pages(vec![album_test_lines(&[
            "file: S/Album/one.flac", "Album: Album", "Album:   ", "Album: Album",
            "Artist:   ", "Artist: A", "AlbumArtist: A", "Date: 2001-03-04", "Genre: Jazz", "Genre: Rock", "Genre: ",
        ]), album_test_lines(&[
            "file: ../rejected.flac", "Album: Album", "Artist: A", "AlbumArtist: A", "Date: 2001", "Genre: Jazz",
            "file: S/Album/two.flac", "Album: Album", "Artist: ", "Genre:   ",
        ])]);
        let map = AudioEngine::canonical_local_album_collectors(collection);
        let album = &map[&album_id("S/Album", "Album")];
        assert_eq!(album.track_count, 2);
        assert_eq!(album.tag_presence.evaluated_track_count, 2);
        assert_eq!(album.tag_presence.missing, LocalAlbumMissingTags { album: 0, artist: 1, album_artist: 1, date: 1, genre: 1 });
        assert_eq!(album.genres, ["Jazz", "Rock"]);
    }

    #[test]
    fn catalog_tag_presence_adds_title_variants_discs_and_collection_pages() {
        let mut first = Vec::new();
        for i in 0..4 {
            first.extend([format!("file: S/Box/CD1/{i}.flac"), "Album: Box".into(), "Artist: A".into(), "Date: 2001-01-02".into(), "Genre: Jazz".into()]);
        }
        let variant = album_test_lines(&["file: S/Box/CD1/five.flac", "Album: Typo", "Artist: A", "AlbumArtist: A", "Genre: Rock"]);
        let mut second = Vec::new();
        for i in 0..3 {
            second.extend([format!("file: S/Box/CD2/{i}.flac"), "Album: Box".into(), "Artist: A".into(), "AlbumArtist: A".into(), "Date: 2002".into()]);
        }
        let albums = AudioEngine::finalize_local_album_collection(collect_album_pages(vec![first, variant, second]), &HashSet::from(["S".into()]));
        assert_eq!(albums.len(), 1);
        let album = &albums[0];
        assert_eq!(album.id, album_id("S/Box", "Box"));
        assert_eq!(album.title, "Box");
        assert_eq!(album.track_count, 8);
        assert_eq!(album.discs.iter().map(|disc| disc.track_count).collect::<Vec<_>>(), [5, 3]);
        assert_eq!(album.year.as_deref(), Some("2001"));
        assert_eq!(album.genres, ["Jazz", "Rock"]);
        let evidence = album.tag_presence.as_ref().unwrap();
        assert_eq!(evidence.evaluated_track_count, 8);
        assert_eq!(evidence.missing, LocalAlbumMissingTags { album: 0, artist: 0, album_artist: 4, date: 1, genre: 3 });
    }

    #[test]
    fn catalog_tag_presence_keeps_only_canonical_physical_or_cue_evidence() {
        for (physical, cue, split_cue, choose_cue) in [(1, 2, false, true), (2, 3, false, false), (1, 1, false, false), (0, 3, false, true), (2, 0, false, false), (1, 2, true, false)] {
            let mut physical_page = Vec::new();
            for i in 0..physical {
                physical_page.extend([format!("file: S/Album/{i}.flac"), "Album: Album".into(), "Artist: A".into(), "AlbumArtist: A".into(), "Date: 2001".into()]);
            }
            let mut cue_page = Vec::new();
            for i in 0..cue {
                let sheet = if split_cue { i } else { 0 };
                cue_page.extend([format!("file: S/Album/image{sheet}.cue/track{:04}", i + 1), "Album: Album".into(), "Genre: CueGenre".into()]);
            }
            let albums = AudioEngine::finalize_local_album_collection(collect_album_pages(vec![physical_page, cue_page]), &HashSet::new());
            let album = &albums[0];
            let count = if choose_cue { cue } else { physical };
            assert_eq!(album.track_count, count);
            let evidence = album.tag_presence.as_ref().unwrap();
            assert_eq!(evidence.evaluated_track_count, count);
            assert_eq!(evidence.missing, if choose_cue { LocalAlbumMissingTags { album: 0, artist: cue, album_artist: cue, date: cue, genre: 0 } }
                else { LocalAlbumMissingTags { album: 0, artist: 0, album_artist: 0, date: 0, genre: physical } });
            // Existing display union still includes discarded CUE genres; it is not presence evidence.
            if cue > 0 { assert_eq!(album.genres, ["CueGenre"]); }
        }
    }

    #[test]
    fn catalog_tag_presence_payload_is_optional_and_serializes_raw_missing_counts() {
        let old = r#"{"id":"legacy","title":"Fallback","artist":"Displayed","year":"2001","folder_path":"S/Album","source_id":null,"track_count":1,"discs":[]}"#;
        let legacy: LocalAlbum = serde_json::from_str(old).unwrap();
        assert!(legacy.tag_presence.is_none());
        assert!(serde_json::to_value(legacy).unwrap().get("tag_presence").is_none());
        let albums = AudioEngine::finalize_local_album_collection(collect_album_pages(vec![album_test_lines(&["file: S/Fallback/one.flac"])]), &HashSet::new());
        let json = serde_json::to_value(&albums[0]).unwrap();
        assert_eq!(json["tag_presence"], serde_json::json!({"evaluated_track_count":1,"missing":{"album":1,"artist":1,"album_artist":1,"date":1,"genre":1}}));
        let roundtrip: LocalAlbum = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.tag_presence, albums[0].tag_presence);
    }

    #[test]
    fn managed_wait_does_not_complete_behind_an_earlier_active_job() {
        for jobs in [vec![Some(1), Some(1), Some(2), None],
            vec![Some(32768), Some(1), None], vec![Some(3), None]] {
            let expected_reads = jobs.len();
            let mut jobs = jobs.into_iter();
            let mut sleeps = 0;
            let result = AudioEngine::wait_for_managed_mpd_update_with(
                Some(2), Duration::from_millis(10), || false,
                || Ok(jobs.next().expect("stop reading after idle")),
                |_| sleeps += 1,
            );
            assert_eq!(result, Ok(MpdRefreshCompletion::Completed));
            assert_eq!(sleeps, expected_reads - 1);
        }
    }

    #[test]
    fn managed_pending_wait_crosses_virtual_hours_and_cancels_after_bounded_io() {
        use std::cell::Cell;
        let clock = Cell::new(Duration::ZERO);
        let reads = Cell::new(0);
        let result = AudioEngine::wait_for_managed_mpd_update_with(
            Some(42),
            Duration::from_millis(100),
            || false,
            || {
                reads.set(reads.get() + 1);
                clock.set(clock.get() + Duration::from_secs(3600));
                Ok(if reads.get() < 4 { Some(42) } else { None })
            },
            |delay| clock.set(clock.get() + delay),
        );
        assert_eq!(result, Ok(MpdRefreshCompletion::Completed));
        assert_eq!(reads.get(), 4);
        assert!(clock.get() > Duration::from_secs(14400));
        for cancelled in [false, true] {
            let closing = Cell::new(false);
            let reads = Cell::new(0);
            let result = AudioEngine::wait_for_managed_mpd_update_with(
                Some(42),
                Duration::from_millis(100),
                || closing.get(),
                || {
                    reads.set(reads.get() + 1);
                    closing.set(cancelled);
                    Err(MpdStatusQueryError::Transport(
                        "injected bounded I/O error".into(),
                    ))
                },
                |_| panic!("no sleep after cancellation/error"),
            );
            assert_eq!(reads.get(), 1);
            if cancelled {
                assert_eq!(result, Err(()));
            } else {
                assert!(matches!(
                    result,
                    Ok(MpdRefreshCompletion::TransportError(_))
                ));
            }
        }
    }

}

#[cfg(test)]
#[path = "audio_reliability_tests.rs"]
pub(crate) mod reliability_tests;

#[cfg(test)]
#[path = "audio_strict_tests.rs"]
mod strict_tests;

#[cfg(test)]
#[path="normal_direct_tests.rs"]
mod normal_direct_tests;

#[cfg(test)]
#[path="route_save_real_tests.rs"]
mod route_save_real_tests;
