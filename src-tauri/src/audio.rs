use crate::plex::{contains_plex_token, legacy_plex_image_ref, PlexImageRef};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Paused,
    Playing,
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
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| Path::new(&h).join(".config")))
        .ok()?;
    Some(base.join("sonante").join("queue_cache.json"))
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
                if bytes.len() <= 8 * 1024 * 1024 {
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
}

impl AudioEngine {
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

    pub fn get_local_albums(&self) -> Result<Vec<LocalAlbum>, String> {
        let lines = self.send_command("listallinfo")?;
        use std::collections::HashMap;

        #[derive(Default)]
        struct AlbumCollector {
            title: String,
            artist: String,
            year: Option<String>,
            track_count: usize,
            folder_path: String,
        }

        let mut map: HashMap<String, AlbumCollector> = HashMap::new();
        let mut cur_file = String::new();
        let mut cur_album = None;
        let mut cur_artist = None;
        let mut cur_album_artist = None;
        let mut cur_date = None;

        let commit_track = |map: &mut HashMap<String, AlbumCollector>,
                           file: &str,
                           album: Option<String>,
                           artist: Option<String>,
                           album_artist: Option<String>,
                           date: Option<String>| {
            if file.is_empty() {
                return;
            }
            let p = Path::new(file);
            let parent_dir = p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
            let folder_name = p.parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Álbum Desconhecido")
                .to_string();

            let title = album.unwrap_or(folder_name);
            let final_artist = album_artist.or(artist).unwrap_or_else(|| "Artista Desconhecido".to_string());
            let key = format!("{}:::{}", final_artist.trim().to_lowercase(), title.trim().to_lowercase());

            let entry = map.entry(key).or_insert_with(|| AlbumCollector {
                title: title.clone(),
                artist: final_artist,
                year: date.clone(),
                track_count: 0,
                folder_path: parent_dir,
            });

            entry.track_count += 1;
            if entry.year.is_none() && date.is_some() {
                entry.year = date;
            }
        };

        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "file" => {
                        commit_track(&mut map, &cur_file, cur_album.take(), cur_artist.take(), cur_album_artist.take(), cur_date.take());
                        cur_file = v.to_string();
                    }
                    "Album" => cur_album = Some(v.to_string()),
                    "Artist" => cur_artist = Some(v.to_string()),
                    "AlbumArtist" => cur_album_artist = Some(v.to_string()),
                    "Date" => cur_date = Some(v.chars().take(4).collect::<String>()),
                    _ => {}
                }
            }
        }

        commit_track(&mut map, &cur_file, cur_album, cur_artist, cur_album_artist, cur_date);

        let mut albums: Vec<LocalAlbum> = map
            .into_iter()
            .filter(|(_, col)| col.track_count > 0 && !col.folder_path.is_empty())
            .map(|(key, col)| LocalAlbum {
                id: key,
                title: col.title,
                artist: col.artist,
                year: col.year,
                folder_path: col.folder_path,
                track_count: col.track_count,
            })
            .collect();

        albums.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        Ok(albums)
    }

    pub fn rescan_library(&self) -> Result<(), String> {
        self.send_command("rescan").map(|_| ())
    }

    fn save_queue_cache(&self) -> Result<(), String> {
        let path = get_queue_cache_path()
            .ok_or_else(|| "Não foi possível determinar o caminho do cache da fila.".to_string())?;
        let json = serde_json::to_vec(&self.queue)
            .map_err(|e| format!("Falha ao serializar o cache da fila: {}", e))?;
        crate::persistence::atomic_write_private(&path, &json, "queue_cache.json")
    }

    fn migrate_loaded_queue(queue: Vec<TrackMetadata>) -> Vec<TrackMetadata> {
        queue
            .into_iter()
            .filter_map(|mut track| {
                Self::sanitize_track_artwork(&mut track);
                match &track.media_locator {
                    Some(MediaLocator::Plex {
                        server_id,
                        part_key,
                        ..
                    }) if !server_id.trim().is_empty() && !part_key.trim().is_empty() => {
                        // A referência estável é a fonte de verdade; nunca mantenha uma URI
                        // autenticada que possa ter vindo de uma versão intermediária do cache.
                        track.uri.clear();
                        Some(track)
                    }
                    Some(MediaLocator::Plex { .. }) => None,
                    Some(MediaLocator::Local { uri }) if !Self::contains_plex_token(uri) => {
                        Some(track)
                    }
                    Some(MediaLocator::Local { .. }) => None,
                    None if !Self::contains_plex_token(&track.uri) => Some(track),
                    None => None,
                }
            })
            .collect()
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

    fn load_queue_cache() -> Vec<TrackMetadata> {
        if let Some(p) = get_queue_cache_path() {
            if let Err(error) =
                crate::persistence::prepare_private_file_for_load(&p, "queue_cache.json")
            {
                eprintln!("[Persistência] {}", error);
            }
            if p.exists() {
                if let Ok(file) = std::fs::File::open(&p) {
                    if let Ok(q) = serde_json::from_reader::<_, Vec<TrackMetadata>>(file) {
                        let migrated = Self::migrate_loaded_queue(q.clone());
                        if migrated != q {
                            if let Ok(json) = serde_json::to_vec(&migrated) {
                                if let Err(error) = crate::persistence::atomic_write_private(
                                    &p,
                                    &json,
                                    "queue_cache.json",
                                ) {
                                    eprintln!("[Persistência] {}", error);
                                }
                            }
                        }
                        return migrated;
                    }
                }
            }
        }
        Vec::new()
    }

    pub fn new(socket_path: &str, music_dir: &str) -> Self {
        let queue = Self::load_queue_cache();
        Self {
            socket_path: socket_path.to_string(),
            music_dir: music_dir.to_string(),
            queue,
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
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| format!("Falha ao conectar no socket MPD ({}): {}", self.socket_path, e))?;

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

    pub fn toggle_play_pause(&self) -> Result<(), String> {
        let status = self.get_status()?;
        match status.state.as_str() {
            "play" => self.send_command("pause 1").map(|_| ()),
            "pause" => self.send_command("pause 0").map(|_| ()),
            _ => self.send_command("play").map(|_| ()),
        }
    }

    pub fn next(&self) -> Result<(), String> {
        self.send_command("next").map(|_| ())
    }

    pub fn previous(&self) -> Result<(), String> {
        self.send_command("previous").map(|_| ())
    }

    pub fn seek(&self, seconds: f64) -> Result<(), String> {
        self.send_command(&format!("seekcur {:.1}", seconds)).map(|_| ())
    }

    pub fn get_queue(&self) -> Vec<TrackMetadata> {
        self.queue.clone()
    }

    pub fn play_index(&self, index: usize) -> Result<(), String> {
        if index >= self.queue.len() {
            return Err(format!(
                "Índice fora da fila: {} para {} faixa(s).",
                index,
                self.queue.len()
            ));
        }
        self.send_command(&format!("play {}", index)).map(|_| ())
    }

    pub fn clear_queue(&mut self) -> Result<(), String> {
        self.send_command("clear")?;
        self.queue.clear();
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
                file_path: Some("/srv/music/file.flac".to_string()),
            }),
            uri: String::new(),
            duration: Some(120.0),
        }
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

        let migrated = AudioEngine::migrate_loaded_queue(vec![legacy_plex, local.clone()]);

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

        let migrated = AudioEngine::migrate_loaded_queue(vec![legacy]);
        let json = serde_json::to_string(&migrated).unwrap();

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
