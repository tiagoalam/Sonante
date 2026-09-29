use crate::config::AppConfig;
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

pub struct MpdSupervisor {
    process: Option<Child>,
    socket_path: String,
}

impl MpdSupervisor {
    pub fn new(socket_path: &str) -> Self {
        Self {
            process: None,
            socket_path: socket_path.to_string(),
        }
    }

    pub fn sonante_config_dir() -> PathBuf {
        let mut path = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("sonante");
        path
    }

    pub fn library_dir() -> PathBuf {
        Self::sonante_config_dir().join("library")
    }

    pub fn sync_library_symlinks(folders: &[String]) -> Result<PathBuf, String> {
        let lib_dir = Self::library_dir();
        fs::create_dir_all(&lib_dir).map_err(|e| e.to_string())?;

        // 1. Limpa links existentes antigos
        if let Ok(entries) = fs::read_dir(&lib_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_symlink() || p.is_file() {
                    let _ = fs::remove_file(p);
                } else if p.is_dir() {
                    let _ = fs::remove_dir_all(p);
                }
            }
        }

        // 2. Deduplica caminhos idênticos informados pelo usuário
        let mut unique_folders = Vec::new();
        for f in folders {
            let p = Path::new(f);
            if p.exists() && p.is_dir() {
                let canonical = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                if !unique_folders.contains(&canonical) {
                    unique_folders.push(canonical);
                }
            }
        }

        let mut used_names = HashSet::new();

        for target_path in unique_folders {
            let base_name = target_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Musicas")
                .to_string();

            let mut final_name = base_name.clone();
            let mut counter = 2;
            while used_names.contains(&final_name) {
                final_name = format!("{} ({})", base_name, counter);
                counter += 1;
            }
            used_names.insert(final_name.clone());

            let symlink_path = lib_dir.join(&final_name);
            let _ = symlink(&target_path, &symlink_path);
        }

        Ok(lib_dir)
    }

    fn ensure_config_file(&self, cfg: &AppConfig) -> Result<PathBuf, String> {
        let dir = Self::sonante_config_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let lib_dir = Self::sync_library_symlinks(&cfg.local_folders)?;
        
        let conf_path = dir.join("mpd.conf");
        let db_path = dir.join("mpd.db");
        let state_path = dir.join("mpd.state");
        let dop_flag = if cfg.dop_enabled { "yes" } else { "no" };

        let audio_output_section = if cfg.audio_output_type == "pipewire" {
            r#"audio_output {
    type "pipewire"
    name "Sonante PipeWire"
    mixer_type "software"
}"#.to_string()
        } else {
            format!(
                r#"audio_output {{
    type "alsa"
    name "Sonante Output"
    device "{}"
    dop "{}"
    mixer_type "software"
}}"#,
                cfg.alsa_device,
                dop_flag
            )
        };
        
        let conf_content = format!(
            r#"music_directory "{}"
playlist_directory "{}"
db_file "{}"
state_file "{}"
restore_paused "yes"
log_file "/dev/null"
bind_to_address "{}"

auto_update "no"
follow_outside_symlinks "yes"
follow_inside_symlinks "yes"

audio_buffer_size "{}"
replaygain "{}"

decoder {{
    plugin "wildmidi"
    enabled "no"
}}

{}
"#,
            lib_dir.display(),
            dir.display(),
            db_path.display(),
            state_path.display(),
            self.socket_path,
            cfg.audio_buffer_size_kb,
            cfg.replay_gain,
            audio_output_section
        );

        fs::write(&conf_path, conf_content).map_err(|e| e.to_string())?;
        Ok(conf_path)
    }

    pub fn start(&mut self, cfg: &AppConfig) -> Result<(), String> {
        let _ = self.stop();

        if Path::new(&self.socket_path).exists() {
            let _ = fs::remove_file(&self.socket_path);
        }

        let conf_path = self.ensure_config_file(cfg)?;

        println!(
            "[Supervisor] Iniciando MPD apontando para {} (DoP: {}, Buffer: {} KB, Pastas: {})",
            cfg.alsa_device, cfg.dop_enabled, cfg.audio_buffer_size_kb, cfg.local_folders.len()
        );

        let child = Command::new("mpd")
            .arg("--no-daemon")
            .arg(&conf_path)
            .spawn()
            .map_err(|e| format!("Falha ao executar mpd: {}", e))?;

        self.process = Some(child);

        for _ in 0..50 {
            if Path::new(&self.socket_path).exists() {
                println!("[Supervisor] Instância do MPD iniciada com sucesso.");
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        Err("Tempo esgotado aguardando o socket do MPD inicializar.".to_string())
    }
    
    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            let pid = child.id();
            // Envia SIGTERM gracioso para que o MPD grave o mpd.state em disco
            let _ = Command::new("kill").arg(pid.to_string()).output();

            // Aguarda até 500ms para a persistência em disco finalizar
            let mut exited = false;
            for _ in 0..10 {
                if let Ok(Some(_)) = child.try_wait() {
                    exited = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            // Fallback forçado apenas se o processo travar
            if !exited {
                let _ = child.kill();
                let _ = child.wait();
            }
        } else {
            let _ = Command::new("killall").arg("mpd").output();
        }

        if Path::new(&self.socket_path).exists() {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}
