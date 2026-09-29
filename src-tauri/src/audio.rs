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
    pub is_bitperfect: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrackMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub thumb: Option<String>,
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
    pub current_file: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub thumb: Option<String>,
    pub volume: i32,
    pub is_updating: bool,
}

fn get_queue_cache_path() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| {
            std::env::var("HOME").map(|h| Path::new(&h).join(".config"))
        })
        .ok()?;
    let dir = base.join("sonante");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("queue_cache.json"))
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
                        is_bitperfect: true,
                    });
                } else if is_hdmi {
                    if let Some(display_name) = check_hdmi_connected(card_num, dev_id) {
                        hdmi_devs.push(AudioDevice {
                            id: hw_id,
                            name: format!("{} — {} (hw:CARD={},DEV={})", card_label, display_name, card_id, dev_id),
                            is_bitperfect: true,
                        });
                    }
                } else {
                    onboard.push(AudioDevice {
                        id: hw_id,
                        name: format!("{} — {} (hw:CARD={},DEV={})", card_label, dev_label, card_id, dev_id),
                        is_bitperfect: true,
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

    fn save_queue_cache(&self) {
        if let Some(p) = get_queue_cache_path() {
            if let Ok(file) = std::fs::File::create(&p) {
                let _ = serde_json::to_writer(file, &self.queue);
            }
        }
    }

    fn load_queue_cache() -> Vec<TrackMetadata> {
        if let Some(p) = get_queue_cache_path() {
            if p.exists() {
                if let Ok(file) = std::fs::File::open(&p) {
                    if let Ok(q) = serde_json::from_reader(file) {
                        return q;
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

    fn send_command(&self, command: &str) -> Result<Vec<String>, String> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| format!("Falha ao conectar no socket MPD ({}): {}", self.socket_path, e))?;

        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| e.to_string())?;

        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut welcome = String::new();
        reader.read_line(&mut welcome).map_err(|e| e.to_string())?;

        let cmd = format!("{}\n", command);
        stream.write_all(cmd.as_bytes()).map_err(|e| e.to_string())?;

        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.is_empty() {
                break;
            }
            let trimmed = line.trim().to_string();
            if trimmed == "OK" {
                break;
            }
            if trimmed.starts_with("ACK") {
                return Err(trimmed);
            }
            lines.push(trimmed);
        }

        Ok(lines)
    }

    /// Prepara a troca de saída: captura a posição e para a reprodução sem destruir a fila
    pub fn prepare_device_switch(&mut self) -> Option<(usize, f64, bool)> {
        let status = self.get_status().ok()?;
        let is_playing = status.state == "play";
        let is_paused = status.state == "pause";

        if !is_playing && !is_paused && self.queue.is_empty() {
            let _ = self.send_command("stop");
            return None;
        }

        let mut cur_index = 0;
        if let Ok(lines) = self.send_command("currentsong") {
            for line in lines {
                if let Some((k, v)) = line.split_once(": ") {
                    if k == "Pos" {
                        if let Ok(idx) = v.parse::<usize>() {
                            cur_index = idx;
                        }
                    }
                }
            }
        }

        let elapsed = status.elapsed;
        let _ = self.send_command("stop");

        Some((cur_index, elapsed, is_playing))
    }

    /// Restaura a fila e retoma a reprodução exatamente no mesmo ponto após reiniciar o daemon
    pub fn restore_after_device_switch(&mut self, saved_state: Option<(usize, f64, bool)>) -> Result<(), String> {
        if self.queue.is_empty() {
            return Ok(());
        }

        let mut batch = String::from("command_list_begin\nclear\n");
        for track in &self.queue {
            batch.push_str(&format!("add \"{}\"\n", track.uri));
        }

        if let Some((idx, elapsed, was_playing)) = saved_state {
            let target_idx = idx.min(self.queue.len().saturating_sub(1));
            batch.push_str(&format!("play {}\n", target_idx));
            if elapsed > 0.5 {
                batch.push_str(&format!("seekcur {:.1}\n", elapsed));
            }
            if !was_playing {
                batch.push_str("pause 1\n");
            }
        }
        batch.push_str("command_list_end");

        self.send_command(&batch).map(|_| ())
    }

    pub fn play_tracks(&mut self, mut tracks: Vec<TrackMetadata>, start_index: usize) -> Result<(), String> {
        for track in &mut tracks {
            if track.thumb.is_none() {
                track.thumb = self.resolve_cover(&track.uri);
            }
        }

        self.queue = tracks;
        self.save_queue_cache();

        let mut batch = String::from("command_list_begin\nclear\n");
        for track in &self.queue {
            batch.push_str(&format!("add \"{}\"\n", track.uri));
        }
        batch.push_str(&format!("play {}\ncommand_list_end", start_index));

        self.send_command(&batch).map(|_| ())
    }

    pub fn play_uris(&mut self, uris: Vec<String>, start_index: usize) -> Result<(), String> {
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
                    uri: u,
                    duration: None,
                }
            })
            .collect();
        self.play_tracks(tracks, start_index)
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
        self.send_command(&format!("play {}", index)).map(|_| ())
    }

    pub fn clear_queue(&mut self) -> Result<(), String> {
        self.queue.clear();
        self.save_queue_cache();
        self.send_command("clear").map(|_| ())
    }

    pub fn list_directory(&self, path: &str) -> Result<Vec<LocalItem>, String> {
        let cmd = if path.trim().is_empty() {
            "lsinfo".to_string()
        } else {
            format!("lsinfo \"{}\"", path)
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
        let lines = match self.send_command("status") {
            Ok(l) => l,
            Err(_) => {
                return Ok(PlaybackStatus {
                    state: "disconnected".to_string(),
                    elapsed: 0.0,
                    duration: 0.0,
                    audio_format: String::new(),
                    current_file: String::new(),
                    title: String::new(),
                    artist: String::new(),
                    album: String::new(),
                    thumb: None,
                    volume: 100,
                    is_updating: false,
                });
            }
        };

        let mut state = "stop".to_string();
        let mut elapsed = 0.0;
        let mut duration = 0.0;
        let mut audio_format = String::new();
        let mut song_index: Option<usize> = None;
        let mut volume: i32 = 100;
        let mut is_updating = false;

        for line in lines {
            if let Some((k, v)) = line.split_once(": ") {
                match k {
                    "state" => state = v.to_string(),
                    "elapsed" => elapsed = v.parse::<f64>().unwrap_or(0.0),
                    "duration" => duration = v.parse::<f64>().unwrap_or(0.0),
                    "audio" => audio_format = v.to_string(),
                    "song" => song_index = v.parse::<usize>().ok(),
                    "volume" => volume = v.parse::<i32>().unwrap_or(100),
                    "updating_db" => is_updating = true,
                    _ => {}
                }
            }
        }

        // Se o MPD estiver parado e sem nenhuma faixa ativa, retorna estado neutro e limpo
        if state == "stop" && song_index.is_none() {
            return Ok(PlaybackStatus {
                state,
                elapsed: 0.0,
                duration: 0.0,
                audio_format,
                current_file: String::new(),
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                thumb: None,
                volume,
                is_updating,
            });
        }

        let song_lines = self.send_command("currentsong").unwrap_or_default();
        let mut current_file = String::new();
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
                        if song_index.is_none() {
                            song_index = v.parse::<usize>().ok();
                        }
                    }
                    _ => {}
                }
            }
        }

        let mut title = String::new();
        let mut artist = String::new();
        let mut album = String::new();
        let mut thumb = None;

        if let Some(idx) = song_index {
            if let Some(track) = self.queue.get(idx) {
                title = track.title.clone();
                artist = track.artist.clone();
                album = track.album.clone();
                thumb = track.thumb.clone();
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

        Ok(PlaybackStatus {
            state,
            elapsed,
            duration,
            audio_format,
            current_file,
            title,
            artist,
            album,
            thumb,
            volume,
            is_updating,
        })
    }
}

pub struct AudioState(pub Mutex<AudioEngine>);
