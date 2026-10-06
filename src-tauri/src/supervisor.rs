use crate::alsa_mixer::{self, MixerSelection};
use crate::audio::VolumeBackend;
use crate::config::AppConfig;
use crate::shared_volume::SharedVolumeBackend;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::CString;
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::fs::{symlink, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

pub struct MpdSupervisor {
    process: Option<Child>,
    socket_path: String,
    pid_path: PathBuf,
    owns_runtime_files: bool,
    owns_analyzer_fifo: bool,
    health: MpdHealth,
    shared_volume_backend: Option<SharedVolumeBackend>,
    volume_backend: VolumeBackend,
}

pub struct LibrarySyncResult {
    pub library_dir: PathBuf,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalLibrarySource {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum MpdHealth {
    Starting,
    Available,
    Unavailable(MpdUnavailableReason),
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MpdUnavailableReason {
    ProcessExited,
    SocketUnavailable,
    ProtocolUnavailable,
    StartupFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MpdProcessObservation {
    Running,
    NotRunning(MpdHealth),
}

#[derive(Debug, PartialEq, Eq)]
enum PidFileIdentity {
    Missing,
    Matches,
    Mismatch,
    Invalid,
}

impl MpdSupervisor {
    pub const ANALYZER_OUTPUT_NAME: &'static str = "Sonante Analyzer";
    pub fn new(socket_path: &str) -> Self {
        let socket_path = PathBuf::from(socket_path);
        let pid_path = socket_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("mpd.pid");
        Self {
            process: None,
            socket_path: socket_path.to_string_lossy().to_string(),
            pid_path,
            owns_runtime_files: false,
            owns_analyzer_fifo: false,
            health: MpdHealth::Starting,
            shared_volume_backend: None,
            volume_backend: VolumeBackend::Unavailable,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_runtime_paths(socket_path: &Path, pid_path: &Path) -> Self {
        Self {
            process: None,
            socket_path: socket_path.to_string_lossy().to_string(),
            pid_path: pid_path.to_path_buf(),
            owns_runtime_files: false,
            owns_analyzer_fifo: false,
            health: MpdHealth::Starting,
            shared_volume_backend: None,
            volume_backend: VolumeBackend::Unavailable,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_process_for_test(&mut self, child: Child, owns_runtime_files: bool) {
        self.process = Some(child);
        self.owns_runtime_files = owns_runtime_files;
        self.health = MpdHealth::Available;
    }

    #[cfg(test)]
    pub(crate) fn has_process_for_test(&self) -> bool {
        self.process.is_some()
    }

    pub(crate) fn shared_volume_backend(&self) -> Option<SharedVolumeBackend> {
        self.shared_volume_backend
    }

    pub(crate) fn volume_backend(&self) -> VolumeBackend {
        self.volume_backend
    }

    pub fn sonante_config_dir() -> PathBuf {
        crate::persistence::sonante_config_dir()
    }

    pub fn database_path() -> PathBuf {
        Self::sonante_config_dir().join("mpd.db")
    }

    pub fn library_dir() -> PathBuf {
        Self::sonante_config_dir().join("library")
    }

    pub fn runtime_dir() -> PathBuf {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| Self::sonante_config_dir().join("runtime"))
            .join("sonante")
    }

    pub fn socket_path() -> PathBuf {
        Self::runtime_dir().join("mpd.socket")
    }

    pub fn analyzer_fifo_path() -> PathBuf {
        Self::runtime_dir().join("analyzer.pcm")
    }

    fn analyzer_fifo_path_for_instance(&self) -> PathBuf {
        Path::new(&self.socket_path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("analyzer.pcm")
    }

    pub(crate) fn observe_health(&mut self) -> Result<MpdProcessObservation, String> {
        let (child_pid, child_status) = match self.process.as_mut() {
            Some(child) => (child.id(), child.try_wait()),
            None => return Ok(MpdProcessObservation::NotRunning(self.health.clone())),
        };

        match child_status {
            Ok(None) => Ok(MpdProcessObservation::Running),
            Ok(Some(_)) => {
                let pid_was_confirmed = matches!(
                    self.read_pid_identity(child_pid),
                    Ok(PidFileIdentity::Matches)
                );
                self.process.take();
                self.health = MpdHealth::Unavailable(MpdUnavailableReason::ProcessExited);
                if let Err(error) =
                    self.cleanup_owned_runtime_files(Some(child_pid), pid_was_confirmed)
                {
                    eprintln!(
                        "[Supervisor] Falha ao limpar runtime após término inesperado do MPD: {}",
                        error
                    );
                }
                Ok(MpdProcessObservation::NotRunning(self.health.clone()))
            }
            Err(error) => Err(format!(
                "Falha ao consultar o processo MPD controlado: {}",
                error
            )),
        }
    }

    pub(crate) fn mark_available(&mut self) -> MpdHealth {
        if self.process.is_some() {
            self.health = MpdHealth::Available;
        }
        self.health.clone()
    }

    pub(crate) fn mark_unavailable(&mut self, reason: MpdUnavailableReason) -> MpdHealth {
        self.health = MpdHealth::Unavailable(reason);
        self.health.clone()
    }

    pub fn sync_library_symlinks(folders: &[String]) -> Result<LibrarySyncResult, String> {
        crate::persistence::ensure_sonante_config_dir()?;
        let lib_dir = Self::library_dir();
        Self::reconcile_library_symlinks(&lib_dir, folders)
    }

    pub fn local_library_sources(
        folders: &[String],
    ) -> Result<Vec<LocalLibrarySource>, String> {
        Self::local_library_sources_in(&Self::library_dir(), folders)
    }

    fn local_library_sources_in(
        lib_dir: &Path,
        folders: &[String],
    ) -> Result<Vec<LocalLibrarySource>, String> {
        let configured_targets = folders
            .iter()
            .filter_map(|folder| {
                let path = Path::new(folder);
                path.is_dir().then_some(path)
            })
            .map(|path| {
                fs::canonicalize(path).map_err(|error| {
                    format!("Falha ao resolver uma pasta local configurada: {error}")
                })
            })
            .collect::<Result<HashSet<_>, _>>()?;

        let entries = match fs::read_dir(lib_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!("Falha ao listar fontes da biblioteca virtual: {error}"))
            }
        };
        let mut entries = entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Falha ao ler fonte da biblioteca virtual: {error}"))?;
        entries.sort_by_key(|entry| entry.file_name());

        let mut sources = Vec::new();
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!("Falha ao examinar fonte da biblioteca virtual: {error}")
            })?;
            if !metadata.file_type().is_symlink() {
                continue;
            }
            let Ok(target) = fs::canonicalize(&path) else {
                continue;
            };
            if !configured_targets.contains(&target) {
                continue;
            }
            let Some(id) = entry.file_name().to_str().and_then(Self::valid_source_id) else {
                continue;
            };
            sources.push(LocalLibrarySource {
                label: id.clone(),
                id,
            });
        }
        Ok(sources)
    }

    fn valid_source_id(value: &str) -> Option<String> {
        let mut components = Path::new(value).components();
        match (components.next(), components.next()) {
            (Some(std::path::Component::Normal(name)), None) if !name.is_empty() => {
                name.to_str().map(str::to_string)
            }
            _ => None,
        }
    }

    fn reconcile_library_symlinks(
        lib_dir: &Path,
        folders: &[String],
    ) -> Result<LibrarySyncResult, String> {
        match fs::symlink_metadata(lib_dir) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "A biblioteca virtual não é um diretório real: {}",
                    lib_dir.display()
                ))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                fs::create_dir(lib_dir).map_err(|e| {
                    format!(
                        "Falha ao criar biblioteca virtual ({}): {}",
                        lib_dir.display(),
                        e
                    )
                })?
            }
            Err(error) => {
                return Err(format!(
                    "Falha ao examinar biblioteca virtual ({}): {}",
                    lib_dir.display(),
                    error
                ))
            }
        }
        let mut unique_folders = Vec::new();
        let mut desired_targets = HashSet::new();
        for f in folders {
            let p = Path::new(f);
            if p.exists() && p.is_dir() {
                let canonical = fs::canonicalize(p).map_err(|e| {
                    format!("Falha ao resolver pasta local ({}): {}", p.display(), e)
                })?;
                if desired_targets.insert(canonical.clone()) {
                    unique_folders.push(canonical);
                }
            }
        }

        let mut entries = fs::read_dir(lib_dir)
            .map_err(|e| {
                format!(
                    "Falha ao listar a biblioteca virtual ({}): {}",
                    lib_dir.display(),
                    e
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Falha ao ler item da biblioteca virtual: {}", e))?;
        entries.sort_by_key(|entry| entry.file_name());

        let mut used_names = HashSet::new();
        let mut preserved_targets = HashSet::new();
        let mut changed = false;
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| {
                format!(
                    "Falha ao examinar item da biblioteca virtual ({}): {}",
                    path.display(),
                    e
                )
            })?;
            if !metadata.file_type().is_symlink() {
                used_names.insert(entry.file_name());
                continue;
            }
            let keep = match fs::canonicalize(&path) {
                Ok(target) => desired_targets.contains(&target) && preserved_targets.insert(target),
                Err(error) if error.kind() == ErrorKind::NotFound => false,
                Err(error) => {
                    return Err(format!(
                        "Falha ao resolver symlink da biblioteca ({}): {}",
                        path.display(),
                        error
                    ))
                }
            };
            if keep {
                used_names.insert(entry.file_name());
            } else {
                fs::remove_file(&path).map_err(|e| {
                    format!(
                        "Falha ao remover symlink antigo ({}): {}",
                        path.display(),
                        e
                    )
                })?;
                changed = true;
            }
        }

        for target_path in unique_folders {
            if preserved_targets.contains(&target_path) {
                continue;
            }
            let base_name = target_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Musicas")
                .to_string();

            let mut final_name = base_name.clone();
            let mut counter = 2;
            while used_names.contains(&std::ffi::OsString::from(final_name.as_str())) {
                final_name = format!("{} ({})", base_name, counter);
                counter += 1;
            }
            used_names.insert(std::ffi::OsString::from(final_name.as_str()));

            let symlink_path = lib_dir.join(&final_name);
            symlink(&target_path, &symlink_path).map_err(|e| {
                format!(
                    "Falha ao adicionar pasta à biblioteca virtual ({}): {}",
                    target_path.display(),
                    e
                )
            })?;
            changed = true;
        }

        Ok(LibrarySyncResult {
            library_dir: lib_dir.to_path_buf(),
            changed,
        })
    }

    fn ensure_config_file(
        &self,
        cfg: &AppConfig,
        shared_volume_backend: Option<SharedVolumeBackend>,
    ) -> Result<(PathBuf, VolumeBackend, LibrarySyncResult), String> {
        let dir = crate::persistence::ensure_sonante_config_dir()?;

        let library_sync = Self::sync_library_symlinks(&cfg.local_folders)?;

        let conf_path = dir.join("mpd.conf");
        let db_path = Self::database_path();
        let dop_flag = if cfg.dop_enabled { "yes" } else { "no" };
        let lib_dir_value = Self::escape_config_value(&library_sync.library_dir.to_string_lossy())?;
        let config_dir_value = Self::escape_config_value(&dir.to_string_lossy())?;
        let db_path_value = Self::escape_config_value(&db_path.to_string_lossy())?;
        let pid_path_value = Self::escape_config_value(&self.pid_path.to_string_lossy())?;
        let socket_path_value = Self::escape_config_value(&self.socket_path)?;
        let alsa_device_value = Self::escape_config_value(&cfg.alsa_device)?;
        let replay_gain_value = Self::escape_config_value(&cfg.replay_gain)?;
        let analyzer_fifo_path = self.analyzer_fifo_path_for_instance();
        if !analyzer_fifo_path.is_absolute() {
            return Err(format!(
                "O FIFO do analyzer precisa usar um caminho absoluto: {}",
                analyzer_fifo_path.display()
            ));
        }
        let analyzer_output_section = Self::analyzer_output_section(&analyzer_fifo_path)?;

        let is_shared = cfg.audio_output_type == "pipewire"
            || cfg.audio_output_type == "shared"
            || cfg.alsa_device == "default";

        let mixer_selection = if is_shared {
            MixerSelection::Software
        } else {
            alsa_mixer::detect_for_pcm(&cfg.alsa_device)
        };
        let volume_backend = Self::public_volume_backend(
            is_shared,
            &mixer_selection,
            shared_volume_backend,
        );
        let audio_output_section = Self::audio_output_section(
            is_shared,
            &alsa_device_value,
            dop_flag,
            &mixer_selection,
            shared_volume_backend,
        )?;

        // Sem state_file: o MPD inicia em modo neutro/stop sem tocar sozinho
        let conf_content = format!(
            r#"music_directory "{}"
playlist_directory "{}"
db_file "{}"
pid_file "{}"
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

{}
"#,
            lib_dir_value,
            config_dir_value,
            db_path_value,
            pid_path_value,
            socket_path_value,
            cfg.audio_buffer_size_kb,
            replay_gain_value,
            audio_output_section,
            analyzer_output_section
        );

        fs::write(&conf_path, conf_content).map_err(|e| e.to_string())?;
        Ok((conf_path, volume_backend, library_sync))
    }

    fn public_volume_backend(
        is_shared: bool,
        mixer_selection: &MixerSelection,
        shared_volume_backend: Option<SharedVolumeBackend>,
    ) -> VolumeBackend {
        if is_shared {
            return match shared_volume_backend {
                Some(SharedVolumeBackend::PipeWire) => VolumeBackend::PipeWire,
                Some(SharedVolumeBackend::MpdSoftware) | None => VolumeBackend::MpdSoftware,
            };
        }

        match mixer_selection {
            MixerSelection::Hardware { .. } => VolumeBackend::AlsaHardware,
            MixerSelection::Software => VolumeBackend::MpdSoftware,
        }
    }

    fn audio_output_section(
        is_shared: bool,
        alsa_device_value: &str,
        dop_flag: &str,
        mixer_selection: &MixerSelection,
        shared_volume_backend: Option<SharedVolumeBackend>,
    ) -> Result<String, String> {
        if is_shared {
            let mixer_type = match shared_volume_backend {
                Some(SharedVolumeBackend::PipeWire) => "none",
                Some(SharedVolumeBackend::MpdSoftware) | None => "software",
            };
            return Ok(format!(r#"audio_output {{
    type "alsa"
    name "Sonante Shared"
    device "default"
    mixer_type "{}"
}}"#, mixer_type));
        }

        let mixer_config = match mixer_selection {
            MixerSelection::Software => "    mixer_type \"software\"".to_string(),
            MixerSelection::Hardware {
                mixer_device,
                control,
            } => {
                let mixer_device = Self::escape_config_value(mixer_device)?;
                let mixer_control = Self::escape_config_value(&control.name)?;
                let mixer_index = if control.index == 0 {
                    String::new()
                } else {
                    format!("\n    mixer_index \"{}\"", control.index)
                };
                format!(
                    "    mixer_type \"hardware\"\n    mixer_device \"{}\"\n    mixer_control \"{}\"{}",
                    mixer_device, mixer_control, mixer_index
                )
            }
        };

        Ok(format!(
            r#"audio_output {{
    type "alsa"
    name "Sonante Output"
    device "{}"
    dop "{}"
{}
}}"#,
            alsa_device_value, dop_flag, mixer_config
        ))
    }

    fn analyzer_output_section(fifo_path: &Path) -> Result<String, String> {
        let fifo_path = Self::escape_config_value(&fifo_path.to_string_lossy())?;
        Ok(format!(
            r#"audio_output {{
    type "fifo"
    name "{}"
    path "{}"
    format "48000:16:2"
    enabled "no"
}}"#,
            Self::ANALYZER_OUTPUT_NAME,
            fifo_path
        ))
    }

    fn escape_config_value(value: &str) -> Result<String, String> {
        if value.contains(['\0', '\r', '\n']) {
            return Err(
                "Valor inválido para mpd.conf: NUL e quebras de linha não são permitidos."
                    .to_string(),
            );
        }
        Ok(value.replace('\\', "\\\\").replace('"', "\\\""))
    }

    pub fn start(&mut self, cfg: &AppConfig) -> Result<LibrarySyncResult, String> {
        let result = self.start_inner(cfg);
        match result {
            Ok(library_sync) => {
                self.health = MpdHealth::Available;
                Ok(library_sync)
            }
            Err(error) => {
                self.health = MpdHealth::Unavailable(MpdUnavailableReason::StartupFailed);
                Err(error)
            }
        }
    }

    fn start_inner(&mut self, cfg: &AppConfig) -> Result<LibrarySyncResult, String> {
        let is_shared = cfg.audio_output_type == "pipewire"
            || cfg.audio_output_type == "shared"
            || cfg.alsa_device == "default";
        let configured_as_pipewire =
            cfg.audio_output_type == "pipewire" || cfg.audio_output_type == "shared";
        let shared_volume_backend = is_shared.then(|| {
            SharedVolumeBackend::detect(is_shared, configured_as_pipewire)
        });

        self.stop()
            .map_err(|e| format!("Falha ao encerrar a instância anterior do MPD: {}", e))?;
        self.health = MpdHealth::Starting;

        crate::persistence::ensure_sonante_config_dir()?;

        let dir = Self::sonante_config_dir();
        // Remove arquivos de estado residuais para assegurar inicialização silenciosa
        let legacy_state = dir.join("mpd.state");
        if let Err(e) = fs::remove_file(&legacy_state) {
            if e.kind() != ErrorKind::NotFound {
                return Err(format!(
                    "Falha ao remover estado residual do MPD ({}): {}",
                    legacy_state.display(),
                    e
                ));
            }
        }

        let (conf_path, volume_backend, library_sync) =
            self.ensure_config_file(cfg, shared_volume_backend)?;
        self.prepare_runtime_files()?;

        println!(
            "[Supervisor] Iniciando MPD: dispositivo={}, saída={}, buffer={} KB",
            cfg.alsa_device, cfg.audio_output_type, cfg.audio_buffer_size_kb
        );

        let child = match Command::new("mpd")
            .arg("--no-daemon")
            .arg(&conf_path)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let failure = format!("Falha ao executar o processo do MPD: {}", error);
                return match self.cleanup_owned_runtime_files(None, false) {
                    Ok(()) => Err(failure),
                    Err(cleanup_error) => Err(format!("{}; cleanup: {}", failure, cleanup_error)),
                };
            }
        };

        self.process = Some(child);

        self.wait_for_startup(50, Duration::from_millis(50))?;
        if let Err(error) = crate::analyzer::set_output_enabled(
            &self.socket_path,
            Self::ANALYZER_OUTPUT_NAME,
            false,
        ) {
            eprintln!(
                "[Analyzer] Falha ao confirmar o output desabilitado após iniciar o MPD: {}",
                error
            );
        }
        self.shared_volume_backend = shared_volume_backend;
        self.volume_backend = volume_backend;
        Ok(library_sync)
    }

    fn prepare_runtime_files(&mut self) -> Result<(), String> {
        let socket_path = Path::new(&self.socket_path);
        let runtime_dir = socket_path.parent().ok_or_else(|| {
            format!(
                "O socket do MPD não possui diretório pai: {}",
                socket_path.display()
            )
        })?;
        fs::create_dir_all(runtime_dir).map_err(|e| {
            format!(
                "Falha ao criar diretório de runtime do MPD ({}): {}",
                runtime_dir.display(),
                e
            )
        })?;
        fs::set_permissions(runtime_dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
            format!(
                "Falha ao proteger diretório de runtime do MPD ({}): {}",
                runtime_dir.display(),
                e
            )
        })?;

        if socket_path.exists() {
            if UnixStream::connect(socket_path).is_ok() {
                return Err(format!(
                    "O socket do MPD já está em uso por uma instância não controlada: {}",
                    socket_path.display()
                ));
            }
            Self::remove_runtime_file(socket_path, "socket MPD stale")?;
        }

        Self::remove_runtime_file(&self.pid_path, "PID file MPD stale")?;
        let fifo_path = self.analyzer_fifo_path_for_instance();
        Self::prepare_analyzer_fifo_path(&fifo_path)?;
        self.owns_analyzer_fifo = true;
        let inode = fs::metadata(&fifo_path)
            .map_err(|e| format!("Falha ao consultar FIFO preparado: {}", e))?
            .ino();
        println!(
            "[Supervisor] Analyzer FIFO prepared before MPD spawn: path={}, inode={}, mode=0600.",
            fifo_path.display(),
            inode
        );
        Ok(())
    }

    pub(crate) fn prepare_analyzer_fifo_path(path: &Path) -> Result<(), String> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.file_type().is_fifo() {
                    return Err(format!(
                        "O caminho reservado ao analyzer não é um FIFO: {}",
                        path.display()
                    ));
                }
                Self::remove_runtime_file(path, "FIFO antigo do analyzer")?;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Falha ao inspecionar FIFO do analyzer ({}): {}",
                    path.display(),
                    error
                ));
            }
        }

        let path_bytes = path.as_os_str().as_encoded_bytes();
        let c_path = CString::new(path_bytes)
            .map_err(|_| "O caminho do FIFO do analyzer contém NUL.".to_string())?;
        let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        if result != 0 {
            return Err(format!(
                "Falha ao criar FIFO do analyzer ({}): {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            let cleanup = Self::remove_runtime_file(path, "FIFO inseguro do analyzer");
            let failure = format!(
                "Falha ao proteger FIFO do analyzer ({}): {}",
                path.display(),
                error
            );
            return match cleanup {
                Ok(()) => Err(failure),
                Err(cleanup_error) => Err(format!("{}; cleanup: {}", failure, cleanup_error)),
            };
        }
        Ok(())
    }

    fn wait_for_startup(&mut self, attempts: usize, delay: Duration) -> Result<(), String> {
        for _ in 0..attempts {
            let (child_pid, child_status) = {
                let child = self
                    .process
                    .as_mut()
                    .ok_or_else(|| "Processo MPD ausente durante a inicialização.".to_string())?;
                (child.id(), child.try_wait())
            };

            match child_status {
                Ok(Some(status)) => {
                    let message = format!(
                        "O processo MPD encerrou antes de criar o socket (status: {}).",
                        status
                    );
                    return Err(self.cleanup_after_start_failure(message));
                }
                Ok(None) => {}
                Err(e) => {
                    let message = format!(
                        "Falha ao consultar o processo MPD durante a inicialização: {}",
                        e
                    );
                    return Err(self.cleanup_after_start_failure(message));
                }
            }

            if Path::new(&self.socket_path).exists() {
                match self.validate_started_process(child_pid) {
                    Ok(true) => {
                        self.owns_runtime_files = true;
                        println!("[Supervisor] Instância do MPD iniciada com sucesso.");
                        return Ok(());
                    }
                    Ok(false) => {}
                    Err(e) => return Err(self.cleanup_after_start_failure(e)),
                }
            }

            thread::sleep(delay);
        }

        Err(self.cleanup_after_start_failure(
            "Tempo esgotado aguardando o socket do MPD inicializar.".to_string(),
        ))
    }

    fn validate_started_process(&self, expected_pid: u32) -> Result<bool, String> {
        match self.read_pid_identity(expected_pid)? {
            PidFileIdentity::Missing => return Ok(false),
            PidFileIdentity::Matches => {}
            PidFileIdentity::Mismatch => {
                return Err(format!(
                    "O PID file do MPD não pertence ao processo iniciado (PID esperado: {}).",
                    expected_pid
                ));
            }
            PidFileIdentity::Invalid => {
                return Err("O PID file do MPD contém uma identidade inválida.".to_string());
            }
        }

        let stream = match UnixStream::connect(&self.socket_path) {
            Ok(stream) => stream,
            Err(_) => return Ok(false),
        };
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| format!("Falha ao configurar timeout do handshake MPD: {}", e))?;
        let mut greeting = String::new();
        BufReader::new(stream)
            .read_line(&mut greeting)
            .map_err(|e| format!("Falha ao ler o handshake do MPD: {}", e))?;
        if !Self::is_valid_greeting(&greeting) {
            return Err("O socket criado não respondeu com um handshake MPD válido.".to_string());
        }

        Ok(true)
    }

    fn is_valid_greeting(greeting: &str) -> bool {
        greeting.starts_with("OK MPD ") && greeting.ends_with('\n')
    }

    fn cleanup_after_start_failure(&mut self, failure: String) -> String {
        match self.stop() {
            Ok(()) => failure,
            Err(cleanup_error) => {
                format!("{} Falha adicional no cleanup: {}", failure, cleanup_error)
            }
        }
    }

    fn read_pid_identity(&self, expected_pid: u32) -> Result<PidFileIdentity, String> {
        match fs::read_to_string(&self.pid_path) {
            Ok(content) => Ok(match content.trim().parse::<u32>() {
                Ok(pid) if pid == expected_pid => PidFileIdentity::Matches,
                Ok(_) => PidFileIdentity::Mismatch,
                Err(_) => PidFileIdentity::Invalid,
            }),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(PidFileIdentity::Missing),
            Err(e) => Err(format!(
                "Falha ao ler PID file do MPD ({}): {}",
                self.pid_path.display(),
                e
            )),
        }
    }

    fn can_request_socket_shutdown(
        has_owned_child: bool,
        owns_runtime_files: bool,
        pid_identity: &PidFileIdentity,
    ) -> bool {
        has_owned_child && owns_runtime_files && matches!(pid_identity, PidFileIdentity::Matches)
    }

    fn request_graceful_shutdown(&self, expected_pid: u32) -> Result<bool, String> {
        if !Path::new(&self.socket_path).exists() {
            return Ok(false);
        }

        let pid_identity = self.read_pid_identity(expected_pid)?;
        if !Self::can_request_socket_shutdown(true, self.owns_runtime_files, &pid_identity) {
            eprintln!(
                "[Supervisor] PID file inconsistente; o encerramento será feito pelo Child controlado."
            );
            return Ok(false);
        }

        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| format!("Falha ao conectar ao socket para encerrar o MPD: {}", e))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| format!("Falha ao configurar timeout de leitura do shutdown: {}", e))?;
        stream
            .set_write_timeout(Some(Duration::from_millis(200)))
            .map_err(|e| format!("Falha ao configurar timeout de shutdown do MPD: {}", e))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| format!("Falha ao preparar leitura do shutdown MPD: {}", e))?,
        );
        let mut greeting = String::new();
        let greeting_bytes = reader
            .read_line(&mut greeting)
            .map_err(|e| format!("Falha ao ler handshake durante shutdown do MPD: {}", e))?;
        if greeting_bytes == 0 || !Self::is_valid_greeting(&greeting) {
            return Err("Handshake inválido durante shutdown do MPD.".to_string());
        }
        stream
            .write_all(b"stop\n")
            .map_err(|e| format!("Falha ao solicitar parada do MPD: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("Falha ao concluir solicitação de parada do MPD: {}", e))?;
        Self::read_command_ok(&mut reader, "stop")?;
        stream
            .write_all(b"kill\n")
            .map_err(|e| format!("Falha ao solicitar shutdown gracioso do MPD: {}", e))?;
        stream
            .flush()
            .map_err(|e| format!("Falha ao concluir solicitação de shutdown do MPD: {}", e))?;
        Ok(true)
    }

    fn read_command_ok<R: BufRead>(reader: &mut R, command: &str) -> Result<(), String> {
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line).map_err(|e| {
                format!("Falha ao ler resposta ao comando {} do MPD: {}", command, e)
            })?;
            if bytes == 0 {
                return Err(format!(
                    "O MPD encerrou a conexão antes de confirmar o comando {}.",
                    command
                ));
            }
            let response = line.trim_end_matches(['\r', '\n']);
            if response == "OK" {
                return Ok(());
            }
            if response.starts_with("ACK") {
                return Err(format!("O MPD recusou o comando {}: {}", command, response));
            }
        }
    }

    fn wait_for_child_exit(
        child: &mut Child,
        attempts: usize,
        delay: Duration,
    ) -> Result<bool, String> {
        for _ in 0..attempts {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(true),
                Ok(None) => thread::sleep(delay),
                Err(e) => return Err(format!("Falha ao consultar término do processo MPD: {}", e)),
            }
        }
        Ok(false)
    }

    fn force_stop_child(child: &mut Child) -> Result<(), String> {
        if let Err(kill_error) = child.kill() {
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) | Err(_) => {
                    return Err(format!(
                        "Falha ao encerrar o processo MPD controlado: {}",
                        kill_error
                    ));
                }
            }
        }

        child
            .wait()
            .map(|_| ())
            .map_err(|e| format!("Falha ao aguardar o processo MPD encerrado: {}", e))
    }

    fn remove_runtime_file(path: &Path, description: &str) -> Result<(), String> {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!(
                "Falha ao remover {} ({}): {}",
                description,
                path.display(),
                e
            )),
        }
    }

    fn cleanup_owned_runtime_files(
        &mut self,
        expected_pid: Option<u32>,
        pid_was_confirmed: bool,
    ) -> Result<(), String> {
        let pid_is_still_owned = match expected_pid {
            Some(pid) => matches!(self.read_pid_identity(pid)?, PidFileIdentity::Matches),
            None => false,
        };
        let ownership_confirmed =
            self.owns_runtime_files || pid_was_confirmed || pid_is_still_owned;
        if !ownership_confirmed && !self.owns_analyzer_fifo {
            return Ok(());
        }

        let socket_result = if ownership_confirmed {
            Self::remove_runtime_file(Path::new(&self.socket_path), "socket MPD")
        } else {
            Ok(())
        };
        let pid_result = if ownership_confirmed && pid_is_still_owned {
            Self::remove_runtime_file(&self.pid_path, "PID file MPD")
        } else {
            Ok(())
        };
        let fifo_path = self.analyzer_fifo_path_for_instance();
        let fifo_result = if self.owns_analyzer_fifo {
            Self::remove_runtime_file(&fifo_path, "FIFO do analyzer após término do MPD")
        } else {
            Ok(())
        };

        let mut failures = Vec::new();
        if let Err(error) = socket_result {
            failures.push(error);
        }
        if let Err(error) = pid_result {
            failures.push(error);
        }
        match fifo_result {
            Ok(()) => {
                if self.owns_analyzer_fifo {
                    println!(
                        "[Supervisor] Analyzer FIFO removed after MPD process termination: {}.",
                        fifo_path.display()
                    );
                }
                self.owns_analyzer_fifo = false;
            }
            Err(error) => failures.push(error),
        }
        if failures.is_empty() {
            self.owns_runtime_files = false;
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.health = MpdHealth::Stopping;
        self.volume_backend = VolumeBackend::Unavailable;
        if let Some(mut child) = self.process.take() {
            let child_pid = child.id();
            let pid_was_confirmed = matches!(
                self.read_pid_identity(child_pid),
                Ok(PidFileIdentity::Matches)
            );
            let already_exited = match child.try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(e) => {
                    eprintln!(
                        "[Supervisor] Falha ao consultar o Child do MPD antes do shutdown: {}",
                        e
                    );
                    false
                }
            };

            if !already_exited {
                if let Err(e) = self.request_graceful_shutdown(child_pid) {
                    eprintln!(
                        "[Supervisor] {}. Usando o Child controlado como fallback.",
                        e
                    );
                }

                let exited =
                    match Self::wait_for_child_exit(&mut child, 8, Duration::from_millis(50)) {
                        Ok(exited) => exited,
                        Err(e) => {
                            eprintln!("[Supervisor] {}. Forçando encerramento pelo Child.", e);
                            false
                        }
                    };

                if !exited {
                    if let Err(e) = Self::force_stop_child(&mut child) {
                        self.process = Some(child);
                        return Err(e);
                    }
                }
            }

            return self.cleanup_owned_runtime_files(Some(child_pid), pid_was_confirmed);
        }

        self.cleanup_owned_runtime_files(None, false)
    }
}

impl Drop for MpdSupervisor {
    fn drop(&mut self) {
        if let Err(e) = self.stop() {
            eprintln!("[Supervisor] Falha no cleanup final do MPD: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_runtime_paths(test_name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("supervisor-tests")
            .join(format!("{}-{}-{}", std::process::id(), test_name, id));
        fs::create_dir_all(&dir).expect("deve criar diretório temporário do teste");
        let socket_path = dir.join("mpd.socket");
        let pid_path = dir.join("mpd.pid");
        (dir, socket_path, pid_path)
    }

    fn library_test_paths(test_name: &str) -> (PathBuf, PathBuf) {
        let (root, _, _) = test_runtime_paths(test_name);
        let library = root.join("library");
        fs::create_dir(&library).unwrap();
        (root, library)
    }

    fn configured_folder(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn library_links_are_added_reused_and_removed_incrementally() {
        let (root, library) = library_test_paths("library-incremental");
        let a = root.join("A");
        let b = root.join("B");
        let c = root.join("C");
        for folder in [&a, &b, &c] {
            fs::create_dir(folder).unwrap();
        }

        let first =
            MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&a)]).unwrap();
        assert!(first.changed);
        assert_eq!(first.library_dir, library);
        let a_inode = fs::symlink_metadata(library.join("A")).unwrap().ino();

        let same =
            MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&a)]).unwrap();
        assert!(!same.changed);
        assert_eq!(
            fs::symlink_metadata(library.join("A")).unwrap().ino(),
            a_inode
        );

        let added = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&a), configured_folder(&b)],
        )
        .unwrap();
        assert!(added.changed);
        assert_eq!(
            fs::symlink_metadata(library.join("A")).unwrap().ino(),
            a_inode
        );
        assert!(library.join("B").is_symlink());

        let removed =
            MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&a)]).unwrap();
        assert!(removed.changed);
        assert_eq!(
            fs::symlink_metadata(library.join("A")).unwrap().ino(),
            a_inode
        );
        assert!(!library.join("B").exists());

        let rollback = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&a), configured_folder(&b)],
        )
        .unwrap();
        assert!(rollback.changed);
        assert_eq!(
            fs::symlink_metadata(library.join("A")).unwrap().ino(),
            a_inode
        );
        assert!(library.join("B").is_symlink());
        MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&a)]).unwrap();

        let added_c = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&a), configured_folder(&c)],
        )
        .unwrap();
        assert!(added_c.changed);
        assert_eq!(
            fs::symlink_metadata(library.join("A")).unwrap().ino(),
            a_inode
        );
        assert!(library.join("C").is_symlink());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_basenames_keep_their_link_names_when_reordered_or_removed() {
        let (root, library) = library_test_paths("library-names");
        let first = root.join("one").join("Music");
        let second = root.join("two").join("Music");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let folders = [configured_folder(&first), configured_folder(&second)];
        assert!(
            MpdSupervisor::reconcile_library_symlinks(&library, &folders)
                .unwrap()
                .changed
        );
        assert_eq!(fs::canonicalize(library.join("Music")).unwrap(), first);
        assert_eq!(fs::canonicalize(library.join("Music (2)")).unwrap(), second);
        let second_inode = fs::symlink_metadata(library.join("Music (2)"))
            .unwrap()
            .ino();

        let reordered = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&second), configured_folder(&first)],
        )
        .unwrap();
        assert!(!reordered.changed);
        assert_eq!(
            fs::symlink_metadata(library.join("Music (2)"))
                .unwrap()
                .ino(),
            second_inode
        );

        let removed =
            MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&second)])
                .unwrap();
        assert!(removed.changed);
        assert!(!library.join("Music").exists());
        assert_eq!(fs::canonicalize(library.join("Music (2)")).unwrap(), second);
        assert_eq!(
            fs::symlink_metadata(library.join("Music (2)"))
                .unwrap()
                .ino(),
            second_inode
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_library_sources_include_only_configured_symlinks() {
        let (root, library) = library_test_paths("library-sources");
        let reggae = root.join("Reggae");
        let other = root.join("Other");
        fs::create_dir(&reggae).unwrap();
        fs::create_dir(&other).unwrap();
        fs::create_dir(library.join("UnexpectedDirectory")).unwrap();
        symlink(&reggae, library.join("Reggae")).unwrap();
        symlink(&other, library.join("Unconfigured")).unwrap();
        symlink(root.join("Missing"), library.join("Broken")).unwrap();

        let sources = MpdSupervisor::local_library_sources_in(
            &library,
            &[configured_folder(&reggae)],
        )
        .unwrap();
        assert_eq!(
            sources,
            vec![LocalLibrarySource {
                id: "Reggae".to_string(),
                label: "Reggae".to_string(),
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_library_sources_preserve_duplicate_basename_names_without_targets() {
        let (root, library) = library_test_paths("library-source-names");
        let first = root.join("one/Music");
        let second = root.join("two/Music");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let folders = [configured_folder(&first), configured_folder(&second)];
        MpdSupervisor::reconcile_library_symlinks(&library, &folders).unwrap();

        let sources = MpdSupervisor::local_library_sources_in(&library, &folders).unwrap();
        assert_eq!(
            sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(),
            vec!["Music", "Music (2)"]
        );
        let serialized = serde_json::to_string(&sources).unwrap();
        assert!(!serialized.contains(&root.to_string_lossy().to_string()));
        assert!(!serialized.contains("one/Music"));
        assert!(!serialized.contains("two/Music"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_library_source_ids_are_single_safe_components() {
        assert_eq!(MpdSupervisor::valid_source_id("Music (2)").as_deref(), Some("Music (2)"));
        for invalid in ["", ".", "..", "Music/Other", "/Music"] {
            assert_eq!(MpdSupervisor::valid_source_id(invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn duplicate_targets_and_broken_links_are_reconciled_without_removing_regular_entries() {
        let (root, library) = library_test_paths("library-broken");
        let folder = root.join("Music");
        fs::create_dir(&folder).unwrap();
        let alias = root.join("alias");
        symlink(&folder, &alias).unwrap();
        symlink(root.join("missing"), library.join("Broken")).unwrap();
        fs::write(library.join("Notes"), "keep").unwrap();

        let result = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&folder), configured_folder(&alias)],
        )
        .unwrap();
        assert!(result.changed);
        assert!(!library.join("Broken").is_symlink());
        assert_eq!(fs::read_to_string(library.join("Notes")).unwrap(), "keep");
        assert!(library.join("Music").is_symlink());
        assert!(!library.join("Music (2)").exists());
        assert!(
            !MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&folder)])
                .unwrap()
                .changed
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn library_root_symlink_is_rejected_without_touching_its_target() {
        let (root, _, _) = test_runtime_paths("library-root-link");
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "untouched").unwrap();
        let library = root.join("library");
        symlink(&outside, &library).unwrap();
        assert!(MpdSupervisor::reconcile_library_symlinks(&library, &[]).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep")).unwrap(),
            "untouched"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn regular_entry_collision_is_preserved_and_new_link_gets_a_suffix() {
        let (root, library) = library_test_paths("library-regular-collision");
        let folder = root.join("Music");
        fs::create_dir(&folder).unwrap();
        fs::write(library.join("Music"), "keep").unwrap();

        let sync =
            MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&folder)])
                .unwrap();
        assert!(sync.changed);
        assert_eq!(fs::read_to_string(library.join("Music")).unwrap(), "keep");
        assert_eq!(fs::canonicalize(library.join("Music (2)")).unwrap(), folder);
        fs::remove_dir_all(root).unwrap();
    }

    fn spawn_sleeping_child() -> Child {
        Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("deve iniciar processo auxiliar")
    }

    fn spawn_exiting_child() -> Child {
        Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .expect("deve iniciar processo auxiliar")
    }

    fn observe_until_not_running(supervisor: &mut MpdSupervisor) -> MpdHealth {
        for _ in 0..50 {
            match supervisor.observe_health().unwrap() {
                MpdProcessObservation::Running => thread::sleep(Duration::from_millis(10)),
                MpdProcessObservation::NotRunning(health) => return health,
            }
        }
        panic!("processo auxiliar não encerrou a tempo");
    }

    #[test]
    fn health_observation_keeps_live_child_running() {
        let (dir, socket_path, pid_path) = test_runtime_paths("health-live-child");
        let child = spawn_sleeping_child();
        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.set_process_for_test(child, false);

        assert_eq!(
            supervisor.observe_health().unwrap(),
            MpdProcessObservation::Running
        );
        assert!(supervisor.has_process_for_test());

        supervisor.stop().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn health_observation_reaps_exited_child_and_is_idempotent() {
        let (dir, socket_path, pid_path) = test_runtime_paths("health-exited-child");
        let child = spawn_exiting_child();
        let child_pid = child.id();
        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.set_process_for_test(child, false);

        let expected = MpdHealth::Unavailable(MpdUnavailableReason::ProcessExited);
        assert_eq!(observe_until_not_running(&mut supervisor), expected);
        assert!(!supervisor.has_process_for_test());
        assert!(!Path::new(&format!("/proc/{}", child_pid)).exists());
        assert_eq!(
            supervisor.observe_health().unwrap(),
            MpdProcessObservation::NotRunning(expected)
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unexpected_exit_cleans_only_owned_runtime_files() {
        let (dir, socket_path, pid_path) = test_runtime_paths("health-runtime-cleanup");
        let child = spawn_exiting_child();
        let child_pid = child.id();
        fs::write(&socket_path, "socket stale").unwrap();
        fs::write(&pid_path, child_pid.to_string()).unwrap();
        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.set_process_for_test(child, true);

        assert_eq!(
            observe_until_not_running(&mut supervisor),
            MpdHealth::Unavailable(MpdUnavailableReason::ProcessExited)
        );
        assert!(!socket_path.exists());
        assert!(!pid_path.exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unexpected_exit_preserves_mismatched_pid_without_signalling_it() {
        let (dir, socket_path, pid_path) = test_runtime_paths("health-pid-mismatch");
        let child = spawn_exiting_child();
        let mut unrelated_child = spawn_sleeping_child();
        fs::write(&socket_path, "socket stale").unwrap();
        fs::write(&pid_path, unrelated_child.id().to_string()).unwrap();
        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.set_process_for_test(child, true);

        assert_eq!(
            observe_until_not_running(&mut supervisor),
            MpdHealth::Unavailable(MpdUnavailableReason::ProcessExited)
        );
        assert!(!socket_path.exists());
        assert!(pid_path.exists());
        assert!(unrelated_child.try_wait().unwrap().is_none());

        unrelated_child.kill().unwrap();
        unrelated_child.wait().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn serialized_health_contains_only_state_and_reason_codes() {
        assert_eq!(
            serde_json::to_string(&MpdHealth::Starting).unwrap(),
            r#"{"state":"starting"}"#
        );
        assert_eq!(
            serde_json::to_string(&MpdHealth::Available).unwrap(),
            r#"{"state":"available"}"#
        );
        assert_eq!(
            serde_json::to_string(&MpdHealth::Stopping).unwrap(),
            r#"{"state":"stopping"}"#
        );
        assert_eq!(
            serde_json::to_string(&MpdHealth::Unavailable(
                MpdUnavailableReason::ProcessExited
            ))
            .unwrap(),
            r#"{"state":"unavailable","reason":"process_exited"}"#
        );
        assert_eq!(
            serde_json::to_string(&MpdHealth::Unavailable(
                MpdUnavailableReason::SocketUnavailable
            ))
            .unwrap(),
            r#"{"state":"unavailable","reason":"socket_unavailable"}"#
        );
        assert_eq!(
            serde_json::to_string(&MpdHealth::Unavailable(
                MpdUnavailableReason::ProtocolUnavailable
            ))
            .unwrap(),
            r#"{"state":"unavailable","reason":"protocol_unavailable"}"#
        );
        assert_eq!(
            serde_json::to_string(&MpdHealth::Unavailable(
                MpdUnavailableReason::StartupFailed
            ))
            .unwrap(),
            r#"{"state":"unavailable","reason":"startup_failed"}"#
        );
    }

    #[test]
    fn socket_shutdown_requires_owned_child_runtime_and_matching_pid() {
        assert!(!MpdSupervisor::can_request_socket_shutdown(
            true,
            true,
            &PidFileIdentity::Missing
        ));
        assert!(MpdSupervisor::can_request_socket_shutdown(
            true,
            true,
            &PidFileIdentity::Matches
        ));
        assert!(!MpdSupervisor::can_request_socket_shutdown(
            false,
            true,
            &PidFileIdentity::Matches
        ));
        assert!(!MpdSupervisor::can_request_socket_shutdown(
            true,
            false,
            &PidFileIdentity::Matches
        ));
        assert!(!MpdSupervisor::can_request_socket_shutdown(
            true,
            true,
            &PidFileIdentity::Mismatch
        ));
        assert!(!MpdSupervisor::can_request_socket_shutdown(
            true,
            true,
            &PidFileIdentity::Invalid
        ));
    }

    #[test]
    fn pid_file_identity_distinguishes_match_mismatch_invalid_and_missing() {
        let (dir, socket_path, pid_path) = test_runtime_paths("pid-identity");
        let supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);

        assert_eq!(
            supervisor.read_pid_identity(42).unwrap(),
            PidFileIdentity::Missing
        );
        fs::write(&pid_path, " 42\n").unwrap();
        assert_eq!(
            supervisor.read_pid_identity(42).unwrap(),
            PidFileIdentity::Matches
        );
        assert_eq!(
            supervisor.read_pid_identity(43).unwrap(),
            PidFileIdentity::Mismatch
        );
        fs::write(&pid_path, "não-é-pid").unwrap();
        assert_eq!(
            supervisor.read_pid_identity(42).unwrap(),
            PidFileIdentity::Invalid
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mpd_greeting_requires_protocol_prefix_and_complete_line() {
        assert!(MpdSupervisor::is_valid_greeting("OK MPD 0.23.15\n"));
        assert!(!MpdSupervisor::is_valid_greeting("OK MPD 0.23.15"));
        assert!(!MpdSupervisor::is_valid_greeting("not an MPD server\n"));
        assert!(!MpdSupervisor::is_valid_greeting(""));
    }

    #[test]
    fn config_values_escape_quotes_and_backslashes_and_reject_line_breaks() {
        assert_eq!(
            MpdSupervisor::escape_config_value("hw:USB\\DAC\"").unwrap(),
            "hw:USB\\\\DAC\\\""
        );
        assert!(MpdSupervisor::escape_config_value("default\nlog_file bad").is_err());
        assert!(MpdSupervisor::escape_config_value("default\rkill").is_err());
        assert!(MpdSupervisor::escape_config_value("default\0kill").is_err());
    }

    #[test]
    fn analyzer_output_is_separate_disabled_and_fixed_to_pcm() {
        let section = MpdSupervisor::analyzer_output_section(Path::new(
            "/run/user/1000/sonante/analyzer.pcm",
        ))
        .unwrap();

        assert!(section.contains("type \"fifo\""));
        assert!(section.contains("name \"Sonante Analyzer\""));
        assert!(section.contains("format \"48000:16:2\""));
        assert!(section.contains("enabled \"no\""));
        assert!(!section.contains("type \"alsa\""));
        assert!(!section.contains("dop"));
    }

    #[test]
    fn shared_output_with_software_backend_keeps_software_mixer() {
        let hardware = MixerSelection::Hardware {
            mixer_device: "hw:CARD=Ignored".to_string(),
            control: crate::alsa_mixer::PlaybackVolumeControl {
                name: "Ignored".to_string(),
                index: 0,
                channels: vec!["Front Left".to_string()],
                min: 0,
                max: 100,
            },
        };

        let section = MpdSupervisor::audio_output_section(
            true,
            "ignored",
            "yes",
            &hardware,
            Some(SharedVolumeBackend::MpdSoftware),
        )
        .unwrap();

        assert!(section.contains("device \"default\""));
        assert!(section.contains("mixer_type \"software\""));
        assert!(!section.contains("mixer_control"));
        assert!(!section.contains("mixer_device"));
    }

    #[test]
    fn shared_output_with_pipewire_backend_disables_mpd_mixer() {
        let section = MpdSupervisor::audio_output_section(
            true,
            "ignored",
            "no",
            &MixerSelection::Software,
            Some(SharedVolumeBackend::PipeWire),
        )
        .unwrap();

        assert!(section.contains("device \"default\""));
        assert!(section.contains("mixer_type \"none\""));
        assert!(!section.contains("mixer_type \"software\""));
        assert!(!section.contains("mixer_control"));
    }

    #[test]
    fn direct_output_without_safe_control_keeps_software_mixer() {
        let section = MpdSupervisor::audio_output_section(
            false,
            "hw:CARD=NoMixer,DEV=0",
            "no",
            &MixerSelection::Software,
            None,
        )
        .unwrap();

        assert!(section.contains("device \"hw:CARD=NoMixer,DEV=0\""));
        assert!(section.contains("dop \"no\""));
        assert!(section.contains("mixer_type \"software\""));
        assert!(!section.contains("mixer_control"));
    }

    #[test]
    fn direct_output_with_safe_control_generates_hardware_mixer_config() {
        let selection = MixerSelection::Hardware {
            mixer_device: "hw:CARD=SoundBar".to_string(),
            control: crate::alsa_mixer::PlaybackVolumeControl {
                name: "USB Playback".to_string(),
                index: 2,
                channels: vec!["Front Left".to_string(), "Front Right".to_string()],
                min: 0,
                max: 127,
            },
        };

        let section = MpdSupervisor::audio_output_section(
            false,
            "hw:CARD=SoundBar,DEV=0",
            "yes",
            &selection,
            None,
        )
        .unwrap();

        assert!(section.contains("mixer_type \"hardware\""));
        assert!(section.contains("mixer_device \"hw:CARD=SoundBar\""));
        assert!(section.contains("mixer_control \"USB Playback\""));
        assert!(section.contains("mixer_index \"2\""));
        assert!(!section.contains("mixer_type \"software\""));
    }

    #[test]
    fn public_volume_backend_matches_the_effective_mixer() {
        let hardware = MixerSelection::Hardware {
            mixer_device: "hw:CARD=SoundBar".to_string(),
            control: crate::alsa_mixer::PlaybackVolumeControl {
                name: "USB Playback".to_string(),
                index: 0,
                channels: vec!["Front Left".to_string(), "Front Right".to_string()],
                min: 0,
                max: 127,
            },
        };

        assert_eq!(
            MpdSupervisor::public_volume_backend(false, &hardware, None),
            VolumeBackend::AlsaHardware
        );
        assert_eq!(
            MpdSupervisor::public_volume_backend(false, &MixerSelection::Software, None),
            VolumeBackend::MpdSoftware
        );
        assert_eq!(
            MpdSupervisor::public_volume_backend(
                true,
                &MixerSelection::Software,
                Some(SharedVolumeBackend::PipeWire),
            ),
            VolumeBackend::PipeWire
        );
        assert_eq!(
            MpdSupervisor::public_volume_backend(
                true,
                &MixerSelection::Software,
                Some(SharedVolumeBackend::MpdSoftware),
            ),
            VolumeBackend::MpdSoftware
        );
    }

    #[test]
    fn hardware_mixer_default_index_is_not_written_to_mpd_config() {
        let selection = MixerSelection::Hardware {
            mixer_device: "hw:2".to_string(),
            control: crate::alsa_mixer::PlaybackVolumeControl {
                name: "Playback".to_string(),
                index: 0,
                channels: vec!["Mono".to_string()],
                min: 0,
                max: 10,
            },
        };

        let section =
            MpdSupervisor::audio_output_section(false, "hw:2,0", "no", &selection, None)
                .unwrap();

        assert!(section.contains("mixer_control \"Playback\""));
        assert!(!section.contains("mixer_index"));
    }

    #[test]
    fn shutdown_response_distinguishes_ok_ack_and_early_eof() {
        let mut ok = std::io::Cursor::new(b"OK\n");
        assert!(MpdSupervisor::read_command_ok(&mut ok, "stop").is_ok());

        let mut ack = std::io::Cursor::new(b"ACK [5@0] {stop} unknown command\n");
        assert!(MpdSupervisor::read_command_ok(&mut ack, "stop")
            .unwrap_err()
            .contains("recusou"));

        let mut eof = std::io::Cursor::new(Vec::<u8>::new());
        assert!(MpdSupervisor::read_command_ok(&mut eof, "stop")
            .unwrap_err()
            .contains("antes de confirmar"));
    }

    #[test]
    fn stale_pid_file_is_removed_without_signalling_its_process() {
        let (dir, socket_path, pid_path) = test_runtime_paths("stale-pid");
        let mut unrelated_child = spawn_sleeping_child();
        fs::write(&pid_path, unrelated_child.id().to_string()).unwrap();

        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.prepare_runtime_files().unwrap();

        assert!(unrelated_child.try_wait().unwrap().is_none());
        assert!(!pid_path.exists());

        unrelated_child.kill().unwrap();
        unrelated_child.wait().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stop_reaps_owned_child_without_pid_file_or_socket() {
        let (dir, socket_path, pid_path) = test_runtime_paths("child-without-pid");
        let child = spawn_sleeping_child();
        let child_pid = child.id();

        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.process = Some(child);
        supervisor.owns_runtime_files = true;

        supervisor.stop().unwrap();

        assert!(supervisor.process.is_none());
        assert!(!Path::new(&format!("/proc/{}", child_pid)).exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn early_child_exit_is_reported_and_runtime_files_are_cleaned() {
        let (dir, socket_path, pid_path) = test_runtime_paths("early-exit");
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .expect("deve iniciar processo auxiliar");
        child.wait().expect("processo auxiliar deve encerrar");
        fs::write(&pid_path, child.id().to_string()).unwrap();

        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.process = Some(child);
        supervisor.owns_runtime_files = true;

        let error = supervisor
            .wait_for_startup(1, Duration::ZERO)
            .expect_err("processo já encerrado deve falhar");

        assert!(error.contains("encerrou antes de criar o socket"));
        assert!(supervisor.process.is_none());
        assert!(!pid_path.exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn startup_timeout_reaps_owned_child_and_cleans_runtime_files() {
        let (dir, socket_path, pid_path) = test_runtime_paths("startup-timeout");
        let child = spawn_sleeping_child();
        let child_pid = child.id();
        fs::write(&pid_path, child_pid.to_string()).unwrap();

        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.process = Some(child);
        supervisor.owns_runtime_files = true;

        let error = supervisor
            .wait_for_startup(1, Duration::ZERO)
            .expect_err("socket ausente deve falhar");

        assert!(error.contains("Tempo esgotado"));
        assert!(supervisor.process.is_none());
        assert!(!pid_path.exists());
        assert!(!Path::new(&format!("/proc/{}", child_pid)).exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_removes_owned_runtime_files_and_is_idempotent() {
        let (dir, socket_path, pid_path) = test_runtime_paths("runtime-cleanup");
        fs::write(&socket_path, "stale socket").unwrap();
        fs::write(&pid_path, "1234").unwrap();
        let fifo_path = dir.join("analyzer.pcm");
        MpdSupervisor::prepare_analyzer_fifo_path(&fifo_path).unwrap();

        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);
        supervisor.owns_runtime_files = true;
        supervisor.owns_analyzer_fifo = true;

        supervisor
            .cleanup_owned_runtime_files(Some(1234), true)
            .unwrap();
        supervisor.cleanup_owned_runtime_files(None, false).unwrap();

        assert!(!socket_path.exists());
        assert!(!pid_path.exists());
        assert!(!fifo_path.exists());
        assert!(!supervisor.owns_runtime_files);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn analyzer_fifo_is_private_and_prepared_before_process_start() {
        let (dir, socket_path, pid_path) = test_runtime_paths("analyzer-fifo-prepare");
        let mut supervisor = MpdSupervisor::new_with_runtime_paths(&socket_path, &pid_path);

        supervisor.prepare_runtime_files().unwrap();

        let fifo_path = dir.join("analyzer.pcm");
        let metadata = fs::metadata(&fifo_path).unwrap();
        assert!(metadata.file_type().is_fifo());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert!(supervisor.process.is_none());
        assert!(supervisor.owns_analyzer_fifo);

        supervisor.cleanup_owned_runtime_files(None, false).unwrap();
        assert!(!fifo_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
