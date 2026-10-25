use crate::audio::AudioEngine;
use crate::direct_volume::{Context as VolumeContext, Session as DirectVolumeSession};
use std::sync::Arc;
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
    pub(crate) shared_mute: crate::shared_mute::Memory,
    shared_mute_generation: u64,
    direct_volume: Option<Arc<DirectVolumeSession>>,
    playback_gate: Option<Arc<crate::direct_playback::Gate>>,
    shared_software_volume: Option<u32>,
    engine_evidence: Option<crate::mpd_engine::EngineEvidence>,
    shared_eq_pipe: Option<PathBuf>,
    output_only_restart: bool,
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
            shared_mute: Default::default(),
            shared_mute_generation: 0,
            direct_volume: None,
            playback_gate: None,
            shared_software_volume: None,
            engine_evidence: None,
            shared_eq_pipe: None,
            output_only_restart: false,
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
            shared_mute: Default::default(),
            shared_mute_generation: 0,
            direct_volume: None,
            playback_gate: None,
            shared_software_volume: None,
            engine_evidence: None,
            shared_eq_pipe: None,
            output_only_restart: false,
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

    fn capture_shared_software_volume(&mut self, read: impl FnOnce() -> Result<Vec<String>, String>) -> Result<(), String> {
        // If capture fails, a later rollback must not fall back to MPD's100%.
        // This is memory only: the current process's volume is not changed.
        self.shared_software_volume = Some(0);
        self.shared_software_volume = Some(crate::direct_volume::observed_software_volume(read()?)?);
        Ok(())
    }

    pub(crate) fn attach_playback_gate(&mut self,gate:Arc<crate::direct_playback::Gate>) {self.playback_gate=Some(gate);}

    pub(crate) fn shared_volume_backend(&self) -> Option<SharedVolumeBackend> {
        self.shared_volume_backend
    }

    // Identity only: no probe, open, wait, signal or lifecycle transition.
    pub(crate) fn observed_process_pid(&self) -> Option<u32> {
        if self.health != MpdHealth::Available || !self.owns_runtime_files { return None; }
        self.process.as_ref().map(Child::id)
    }

    pub(crate) fn shared_mute_context(&self) -> Option<String> {
        let pid=self.observed_process_pid()?;
        matches!(self.volume_backend,VolumeBackend::PipeWire|VolumeBackend::MpdSoftware)
            .then(||format!("{}:{}:{:?}",self.shared_mute_generation,pid,self.volume_backend))
    }

    pub(crate) fn volume_context(&self) -> VolumeContext {
        if self.observed_process_pid().is_none() { return VolumeContext::new(VolumeBackend::Unavailable, None); }
        VolumeContext::new(self.volume_backend, self.direct_volume.clone())
    }

    pub(crate) fn set_direct_mute(&self, muted: bool, token: &str, cancelled: impl Fn() -> bool) -> Result<(), String> {
        if !muted {self.playback_gate.as_ref().ok_or(crate::direct_volume::STALE)?.require_choice()?;}
        let pid = self.observed_process_pid().ok_or(crate::direct_volume::STALE)?;
        self.direct_volume.as_ref().ok_or(crate::direct_volume::STALE)?
            .set_mute(muted, !muted, token, |cmd| {
                if cancelled() { return Err(crate::config_save_operation::INTERRUPTED.into()); }
                AudioEngine::send_owned_command_to_socket(&self.socket_path, pid, cmd)
            })
    }

    pub(crate) fn volume_backend(&self) -> VolumeBackend {
        self.volume_backend
    }

    pub(crate) fn engine_evidence_for_pid(&self, pid: u32) -> Option<crate::mpd_engine::EngineEvidence> {
        if self.observed_process_pid()? != pid { return None; }
        self.engine_evidence.clone()
    }

    pub fn sonante_config_dir() -> PathBuf {
        crate::persistence::sonante_config_dir()
    }

    pub fn database_path() -> PathBuf {
        Self::database_path_in(&Self::sonante_config_dir())
    }

    pub(crate) fn database_path_in(config_dir: &Path) -> PathBuf {
        config_dir.join("mpd-v2.db")
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
                if let Some(session) = self.direct_volume.take() { session.invalidate(); }
                if let Some(gate)=&self.playback_gate {gate.clear();}
                self.volume_backend = VolumeBackend::Unavailable;
                self.health = MpdHealth::Unavailable(MpdUnavailableReason::ProcessExited);
                if let Err(error) =
                    self.cleanup_owned_runtime_files(Some(child_pid), pid_was_confirmed)
                {
                    eprintln!(
                        "[Supervisor] Failed to clean up runtime after unexpected MPD termination: {}",
                        crate::diagnostics::error_summary(&error)
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
            if let Some(gate)=&self.playback_gate {gate.set_available(true);}
        }
        self.health.clone()
    }

    pub(crate) fn mark_unavailable(&mut self, reason: MpdUnavailableReason) -> MpdHealth {
        self.health = MpdHealth::Unavailable(reason);
        if let Some(gate)=&self.playback_gate {gate.set_available(false);}
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
        let resolved = crate::local_media::resolve_local_sources(lib_dir, folders)
            .map_err(|error| error.to_string())?;
        Ok(resolved
            .sources()
            .keys()
            .map(|id| LocalLibrarySource {
                id: id.clone(),
                label: id.clone(),
            })
            .collect())
    }

    #[cfg(test)]
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
        let managed_marker = lib_dir.join(crate::local_media::mirror::MANIFEST_NAME);
        match fs::symlink_metadata(&managed_marker) {
            Ok(_) => {
                let resolved = crate::local_media::resolve_local_sources(lib_dir, folders)
                    .map_err(|error| error.to_string())?;
                let manifest = crate::diagnostics::startup_stage("managed.manifest", crate::local_media::mirror::read_manifest(lib_dir))?;
                if &manifest.sources != resolved.sources() {
                    return Err(
                        "A geração managed diverge das pastas locais configuradas."
                            .to_string(),
                    );
                }
                let (current, namespace) = crate::diagnostics::startup_stage("managed.identity_sources", crate::local_media::mirror::identity_sources(lib_dir))?;
                let roots = manifest.sources.values().cloned().collect::<Vec<_>>();
                crate::diagnostics::startup_stage("managed.identity_reserve", crate::source_identity::reserve(lib_dir, &roots, &current, namespace.as_deref()))?;
                return Ok(LibrarySyncResult {
                    library_dir: lib_dir.to_path_buf(),
                    changed: false,
                });
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Falha ao examinar marker da biblioteca virtual ({}): {}",
                    managed_marker.display(),
                    error
                ))
            }
        }
        // A configured source going offline is not an authorized removal.
        // Validate all roots before reserving identities or deleting any link,
        // using the same availability policy as the managed path. An explicit
        // new list (including empty) still removes sources deliberately.
        let configured = folders.iter().map(PathBuf::from).collect::<Vec<_>>();
        let unique_folders = crate::local_media::mirror::canonical_configured_roots(&configured)?;
        let desired_targets = unique_folders.iter().cloned().collect::<HashSet<_>>();

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

        let (mut current, namespace) = crate::local_media::mirror::identity_sources(lib_dir)?;
        // Legacy reconciliation already keeps the first alias for a target.
        // Reserve the remaining names as retired, retaining that behavior.
        let mut seen = HashSet::new();
        for root in current.values_mut() {
            if root.as_ref().is_some_and(|p| !seen.insert(p.clone())) { *root = None; }
        }
        let (desired, _) = crate::source_identity::reserve(lib_dir, &unique_folders, &current, namespace.as_deref())?;
        crate::source_identity::verify_origins(lib_dir, &desired)?;
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
            if !keep {
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

        for (final_name, target_path) in desired {
            if preserved_targets.contains(&target_path) { continue; }
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
        engine: &crate::mpd_engine::MpdEngine,
        existing: Option<&crate::InitialManagedGeneration<'_>>,
    ) -> Result<(PathBuf, VolumeBackend, LibrarySyncResult), String> {
        let dir = crate::diagnostics::startup_stage("config.profile", crate::persistence::ensure_sonante_config_dir())?;

        let library_sync = if let Some(admission)=existing {
            admission.validate(cfg)?;
            LibrarySyncResult { library_dir: Self::library_dir(), changed: false }
        } else if self.output_only_restart {
            // The caller validated the existing generation before stopping.
            // EQ changes output only; it must not publish mirrors or rescan.
            LibrarySyncResult { library_dir: Self::library_dir(), changed: false }
        } else { crate::diagnostics::startup_stage("library.reconcile", Self::sync_library_symlinks(&cfg.local_folders))? };

        let conf_path = dir.join("mpd.conf");
        let db_path = Self::database_path();
        let dop_flag = if cfg.dop_enabled { "yes" } else { "no" };
        let lib_dir_value = Self::escape_config_value(&library_sync.library_dir.to_string_lossy())?;
        let config_dir_value = Self::escape_config_value(&dir.to_string_lossy())?;
        let database_section = Self::database_section(&db_path)?;
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

        // Direct never probes or writes the physical mixer. Its owned software
        // gain is controlled exclusively by the session's unity/mute protocol.
        let volume_backend = Self::public_volume_backend(
            is_shared,
            shared_volume_backend,
        );
        let audio_output_section = if let Some(pipe) = &self.shared_eq_pipe {
            Self::shared_eq_output_section(is_shared, pipe, shared_volume_backend)?
        } else { Self::audio_output_section(
            is_shared,
            &alsa_device_value,
            dop_flag,
            shared_volume_backend,
        )? };
        let audio_output_section = engine.configure_output(audio_output_section, is_shared)?;

        // Sem state_file: o MPD inicia em modo neutro/stop sem tocar sozinho
        let audio_buffer_size = cfg.audio_buffer_size_kb.to_string();
        let conf_content = Self::render_config(
            &lib_dir_value,
            &config_dir_value,
            &database_section,
            &pid_path_value,
            &socket_path_value,
            &audio_buffer_size,
            &replay_gain_value,
            &audio_output_section,
            &analyzer_output_section,
        );

        fs::write(&conf_path, conf_content).map_err(|e| e.to_string())?;
        Ok((conf_path, volume_backend, library_sync))
    }

    pub(crate) fn shared_eq_pipe(&self) -> Option<PathBuf> { self.shared_eq_pipe.clone() }

    pub(crate) fn clear_shared_eq_pipe(&mut self) { self.shared_eq_pipe = None; }

    pub(crate) fn start_shared_eq_output(&mut self, cfg: &AppConfig, pipe: Option<PathBuf>) -> Result<LibrarySyncResult, String> {
        if !crate::is_shared_output(cfg) { return Err("EQ_SHARED_ONLY".into()); }
        crate::managed_save_intent::check_startup(&Self::sonante_config_dir())?;
        crate::local_media::resolve_local_sources(&Self::library_dir(), &cfg.local_folders).map_err(|_| "EQ_LIBRARY_UNAVAILABLE")?;
        if let Some(path) = &pipe { Self::validate_eq_pipe(path)?; }
        self.shared_eq_pipe = pipe;
        self.output_only_restart = true;
        let result = self.start(cfg);
        self.output_only_restart = false;
        result
    }

    fn validate_eq_pipe(path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or("EQ_PIPE_UNSAFE")?;
        let m = fs::symlink_metadata(path).map_err(|_| "EQ_PIPE_UNAVAILABLE")?;
        let dir = fs::symlink_metadata(parent).map_err(|_| "EQ_PIPE_UNAVAILABLE")?;
        if !path.is_absolute() || path.file_name().and_then(|s|s.to_str()) != Some("pcm.s32le")
            || parent.parent() != Some(Self::runtime_dir().as_path())
            || !parent.file_name().and_then(|s|s.to_str()).is_some_and(|s|s.starts_with("shared-eq-") && s.len()==42)
            || !m.file_type().is_fifo() || m.uid()!=unsafe{libc::getuid()} || m.nlink()!=1 || m.mode()&0o777!=0o600
            || !dir.is_dir() || dir.file_type().is_symlink() || dir.uid()!=m.uid() || dir.mode()&0o777!=0o700 {
            return Err("EQ_PIPE_UNSAFE".into());
        }
        Ok(())
    }

    fn shared_eq_output_section(is_shared: bool, path: &Path, backend: Option<SharedVolumeBackend>) -> Result<String,String> {
        if !is_shared { return Err("EQ_SHARED_ONLY".into()); }
        Self::validate_eq_pipe(path)?;
        let marker = Self::escape_config_value(&path.with_file_name("continuity.state").to_string_lossy())?;
        let path = Self::escape_config_value(&path.to_string_lossy())?;
        let mixer = if backend == Some(SharedVolumeBackend::PipeWire) { "none" } else { "software" };
        Ok(format!(r#"audio_output {{
    type "fifo"
    name "Sonante Shared EQ"
    sonante_strict_eq "yes"
    sonante_continuity "{marker}"
    path "{path}"
    format "48000:32:2"
    mixer_type "{mixer}"
    always_on "yes"
}}"#))
    }

    fn render_config(
        library_dir: &str,
        playlist_dir: &str,
        database_section: &str,
        pid_path: &str,
        socket_path: &str,
        audio_buffer_size: &str,
        replay_gain: &str,
        audio_output_section: &str,
        analyzer_output_section: &str,
    ) -> String {
        format!(
            r#"music_directory "{}"
playlist_directory "{}"
{}
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
            library_dir,
            playlist_dir,
            database_section,
            pid_path,
            socket_path,
            audio_buffer_size,
            replay_gain,
            audio_output_section,
            analyzer_output_section
        )
    }

    fn public_volume_backend(
        is_shared: bool,
        shared_volume_backend: Option<SharedVolumeBackend>,
    ) -> VolumeBackend {
        if is_shared {
            return match shared_volume_backend {
                Some(SharedVolumeBackend::PipeWire) => VolumeBackend::PipeWire,
                Some(SharedVolumeBackend::MpdSoftware) | None => VolumeBackend::MpdSoftware,
            };
        }

        VolumeBackend::DirectEquipment
    }

    fn database_section(database_path: &Path) -> Result<String, String> {
        let database_path = Self::escape_config_value(&database_path.to_string_lossy())?;
        Ok(format!(
            r#"database {{
    plugin "simple"
    path "{}"
    hide_playlist_targets "no"
}}"#,
            database_path
        ))
    }

    fn audio_output_section(
        is_shared: bool,
        alsa_device_value: &str,
        dop_flag: &str,
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

        Ok(format!(
            r#"audio_output {{
    type "alsa"
    name "Sonante Output"
    device "{}"
    dop "{}"
    mixer_type "software"
}}"#,
            alsa_device_value, dop_flag
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
        self.start_admitted(cfg,None)
    }
    pub(crate) fn start_initial_existing(&mut self,cfg:&AppConfig,admission:&crate::InitialManagedGeneration<'_>)->Result<LibrarySyncResult,String> {
        if self.process.is_some() || self.output_only_restart {return Err("SOURCE_STARTUP_UNCONFIRMED".into());}
        self.start_admitted(cfg,Some(admission))
    }
    fn start_admitted(&mut self,cfg:&AppConfig,existing:Option<&crate::InitialManagedGeneration<'_>>)->Result<LibrarySyncResult,String> {
        let result = self.start_inner(cfg,existing);
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

    fn start_inner(&mut self, cfg: &AppConfig, existing:Option<&crate::InitialManagedGeneration<'_>>) -> Result<LibrarySyncResult, String> {
        if let Some(admission)=existing {admission.validate(cfg)?;}
        // Resolve/validate an explicit development engine before stopping
        // the current owned MPD. No override keeps the established path.
        let engine = crate::diagnostics::startup_stage("engine.resolve", if self.shared_eq_pipe.is_some() { crate::mpd_engine::MpdEngine::resolve_eq() } else { crate::mpd_engine::MpdEngine::resolve() })?;
        if self.shared_eq_pipe.is_some() && !crate::is_shared_output(cfg) { return Err("EQ_SHARED_ONLY".into()); }

        let is_shared = cfg.audio_output_type == "pipewire"
            || cfg.audio_output_type == "shared"
            || cfg.alsa_device == "default";
        let configured_as_pipewire =
            cfg.audio_output_type == "pipewire" || cfg.audio_output_type == "shared";
        let shared_volume_backend = is_shared.then(|| {
            SharedVolumeBackend::detect(is_shared, configured_as_pipewire)
        });

        if self.volume_backend == VolumeBackend::MpdSoftware && self.health == MpdHealth::Available {
            let pid = self.observed_process_pid().ok_or(crate::direct_volume::STALE)?;
            let socket = self.socket_path.clone();
            self.capture_shared_software_volume(|| AudioEngine::send_owned_command_to_socket(&socket, pid, "status"))?;
        } else if self.volume_backend == VolumeBackend::DirectEquipment && self.shared_software_volume.is_none() {
            // No previous Shared value is known: enter it silently rather than
            // copying Direct's unity gain or assuming a previous listening level.
            self.shared_software_volume = Some(0);
        }
        crate::diagnostics::startup_stage("previous.stop", self.stop())
            .map_err(|e| format!("Falha ao encerrar a instância anterior do MPD: {}", e))?;
        self.health = MpdHealth::Starting;

        crate::diagnostics::startup_stage("profile.ensure", crate::persistence::ensure_sonante_config_dir())?;

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
            crate::diagnostics::startup_stage("config.ensure", self.ensure_config_file(cfg, shared_volume_backend, &engine, existing))?;
        crate::diagnostics::startup_stage("runtime.prepare", self.prepare_runtime_files())?;

        if let Some(admission)=existing {
            if let Err(error)=admission.validate(cfg){return Err(self.cleanup_after_start_failure(error));}
        }

        println!(
            "[Supervisor] Starting MPD: route={}, buffer={} KB",
            if crate::is_shared_output(cfg) {
                "shared"
            } else {
                "direct"
            },
            cfg.audio_buffer_size_kb
        );

        let child = match Command::new(engine.program())
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

        let engine_identity = engine.verify_child(&child);
        self.process = Some(child);
        if let Err(error) = engine_identity {
            return Err(self.cleanup_after_start_failure(error));
        }

        self.wait_for_startup(50, Duration::from_millis(50))?;
        if let Err(error) = crate::analyzer::set_output_enabled(
            &self.socket_path,
            Self::ANALYZER_OUTPUT_NAME,
            false,
        ) {
            eprintln!(
                "[Analyzer] Failed to confirm disabled output after MPD startup: {}",
                crate::diagnostics::error_summary(&error)
            );
        }
        let pid = self.process.as_ref().ok_or(crate::direct_volume::STALE)?.id();
        if volume_backend == VolumeBackend::DirectEquipment {
            match DirectVolumeSession::initialize(|cmd| AudioEngine::send_owned_command_to_socket(&self.socket_path, pid, cmd)) {
                Ok(session) => {
                    if cfg.direct_muted {
                        if let Err(error)=session.set_mute(true,false,&session.token(),|cmd|AudioEngine::send_owned_command_to_socket(&self.socket_path,pid,cmd)) {
                            return Err(self.cleanup_after_start_failure(error));
                        }
                    }
                    self.direct_volume = Some(session);
                },
                Err(error) => return Err(self.cleanup_after_start_failure(error)),
            }
        } else if volume_backend == VolumeBackend::MpdSoftware {
            if let Some(value) = self.shared_software_volume {
                // Only a value observed in our previous Shared software session.
                // The new MPD is stopped; Direct gain is never copied here.
                if let Err(error) = crate::direct_volume::restore_shared_software(value, |cmd| AudioEngine::send_owned_command_to_socket(&self.socket_path, pid, cmd)) {
                    return Err(self.cleanup_after_start_failure(error));
                }
            }
        }
        self.shared_volume_backend = shared_volume_backend;
        self.volume_backend = volume_backend;
        self.shared_mute_generation = self.shared_mute_generation.wrapping_add(1);
        self.shared_mute = Default::default();
        self.engine_evidence = engine.evidence(is_shared);
        if let Some(admission)=existing {
            if let Err(error)=admission.validate(cfg){return Err(self.cleanup_after_start_failure(error));}
        }
        if let Some(gate)=&self.playback_gate {gate.bind(cfg,pid,self.direct_volume.clone());}
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
            "[Supervisor] Analyzer FIFO prepared before MPD spawn: inode={}, mode=0600.",
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
                        println!("[Supervisor] MPD instance started successfully.");
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
                "[Supervisor] Inconsistent PID file; shutdown will use the owned Child handle."
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
                    println!("[Supervisor] Analyzer FIFO removed after MPD process termination.");
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
        if let Some(session) = self.direct_volume.take() { session.invalidate(); }
        if let Some(gate)=&self.playback_gate {gate.clear();}
        self.engine_evidence = None;
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
                        "[Supervisor] Failed to query MPD Child before shutdown: {}",
                        crate::diagnostics::error_summary(&e)
                    );
                    false
                }
            };

            if !already_exited {
                if let Err(e) = self.request_graceful_shutdown(child_pid) {
                    eprintln!(
                        "[Supervisor] Graceful shutdown failed: {}. Using the owned Child handle as fallback.",
                        crate::diagnostics::error_summary(&e)
                    );
                }

                let exited =
                    match Self::wait_for_child_exit(&mut child, 8, Duration::from_millis(50)) {
                        Ok(exited) => exited,
                        Err(e) => {
                            eprintln!("[Supervisor] Child exit wait failed: {}. Forcing termination through the owned Child handle.", crate::diagnostics::error_summary(&e));
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
            eprintln!(
                "[Supervisor] Final MPD cleanup failed: {}",
                crate::diagnostics::error_summary(&e)
            );
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
        // Fixture identity must survive a symlinked Cargo target/cache directory.
        let dir = fs::canonicalize(dir).unwrap();
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

    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, String, u64, Vec<u8>)> {
        fn visit(
            root: &Path,
            current: &Path,
            snapshot: &mut Vec<(PathBuf, String, u64, Vec<u8>)>,
        ) {
            let mut entries = fs::read_dir(current)
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                let metadata = fs::symlink_metadata(&path).unwrap();
                if metadata.file_type().is_symlink() {
                    snapshot.push((
                        relative,
                        "symlink".to_string(),
                        metadata.ino(),
                        fs::read_link(&path)
                            .unwrap()
                            .to_string_lossy()
                            .as_bytes()
                            .to_vec(),
                    ));
                } else if metadata.is_dir() {
                    snapshot.push((
                        relative,
                        "directory".to_string(),
                        metadata.ino(),
                        Vec::new(),
                    ));
                    visit(root, &path, snapshot);
                } else {
                    snapshot.push((
                        relative,
                        "file".to_string(),
                        metadata.ino(),
                        fs::read(&path).unwrap(),
                    ));
                }
            }
        }

        let root_metadata = fs::symlink_metadata(root).unwrap();
        let mut snapshot = vec![(
            PathBuf::new(),
            "directory".to_string(),
            root_metadata.ino(),
            Vec::new(),
        )];
        visit(root, root, &mut snapshot);
        snapshot
    }

    fn publish_managed_library(root: &Path, library: &Path, physical_root: &Path) {
        let staging = root.join("library.next");
        crate::local_media::mirror::build_filtered_staging(
            &staging,
            library,
            &[physical_root.to_path_buf()],
            "supervisor-managed-sync",
        )
        .unwrap();
        fs::remove_dir(library).unwrap();
        fs::rename(staging, library).unwrap();
    }

    #[test]
    fn legacy_offline_root_keeps_alias_ledger_and_references_until_restored_or_explicitly_removed()
    {
        let (root, library) = library_test_paths("legacy-offline-preservation");
        let music = root.join("Music");
        fs::create_dir_all(music.join("Album")).unwrap();
        fs::write(music.join("Album/track.flac"), b"synthetic-path-fixture").unwrap();
        let folders = vec![configured_folder(&music)];
        MpdSupervisor::reconcile_library_symlinks(&library, &folders).unwrap();
        let before = tree_snapshot(&library);
        let ledger = fs::read(root.join("local-source-identities.json")).unwrap();
        for file in ["favorites.json", "playlists.json", "queue_cache.json"] {
            fs::write(root.join(file), br#"{"uri":"Music/Album/track.flac"}"#).unwrap();
        }
        let offline = root.join("Music.offline");
        fs::rename(&music, &offline).unwrap();
        let result = MpdSupervisor::reconcile_library_symlinks(&library, &folders);
        eprintln!(
            "legacy offline: result={:?}, alias_retained={}",
            result.as_ref().map(|v| v.changed),
            library.join("Music").is_symlink()
        );
        assert!(
            result.is_err(),
            "Unavailable configured source is not an authorized removal"
        );
        assert_eq!(tree_snapshot(&library), before);
        assert_eq!(
            fs::read(root.join("local-source-identities.json")).unwrap(),
            ledger
        );
        assert!(crate::local_media::resolve_local_sources(&library, &folders).is_err());
        for file in ["favorites.json", "playlists.json", "queue_cache.json"] {
            assert_eq!(
                fs::read(root.join(file)).unwrap(),
                br#"{"uri":"Music/Album/track.flac"}"#
            );
        }
        fs::rename(&offline, &music).unwrap();
        assert!(
            !MpdSupervisor::reconcile_library_symlinks(&library, &folders)
                .unwrap()
                .changed
        );
        assert_eq!(tree_snapshot(&library), before);
        assert_eq!(
            fs::read(root.join("local-source-identities.json")).unwrap(),
            ledger
        );
        assert!(crate::local_media::resolve_local_path(
            &library,
            "Music/Album/track.flac",
            &folders
        )
        .is_ok());
        // Explicit Preferences removal is represented by a new desired list.
        assert!(
            MpdSupervisor::reconcile_library_symlinks(&library, &[])
                .unwrap()
                .changed
        );
        assert!(!library.join("Music").is_symlink());
        assert!(music.join("Album/track.flac").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_offline_one_root_blocks_partial_reconciliation_but_explicit_removal_still_works() {
        let (root, library) = library_test_paths("legacy-offline-two-sources");
        let a = root.join("A");
        let b = root.join("B");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let folders = vec![configured_folder(&a), configured_folder(&b)];
        MpdSupervisor::reconcile_library_symlinks(&library, &folders).unwrap();
        let before = tree_snapshot(&library);
        let ledger = fs::read(root.join("local-source-identities.json")).unwrap();
        fs::rename(&b, root.join("B.offline")).unwrap();
        assert!(MpdSupervisor::reconcile_library_symlinks(&library, &folders).is_err());
        assert_eq!(tree_snapshot(&library), before);
        assert_eq!(
            fs::read(root.join("local-source-identities.json")).unwrap(),
            ledger
        );
        assert!(
            MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&a)])
                .unwrap()
                .changed
        );
        assert!(library.join("A").is_symlink());
        assert!(!library.join("B").is_symlink());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_offline_replaced_by_regular_file_is_not_a_removal() {
        let (root, library) = library_test_paths("legacy-offline-not-directory");
        let music = root.join("Music");
        fs::create_dir(&music).unwrap();
        let folders = vec![configured_folder(&music)];
        MpdSupervisor::reconcile_library_symlinks(&library, &folders).unwrap();
        let before = tree_snapshot(&library);
        let ledger = fs::read(root.join("local-source-identities.json")).unwrap();
        fs::rename(&music, root.join("Music.offline")).unwrap();
        fs::write(&music, b"not a directory").unwrap();
        assert!(MpdSupervisor::reconcile_library_symlinks(&library, &folders).is_err());
        assert_eq!(tree_snapshot(&library), before);
        assert_eq!(
            fs::read(root.join("local-source-identities.json")).unwrap(),
            ledger
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_reconcile_behavior_remains_unchanged_without_manifest() {
        let (root, library) = library_test_paths("legacy-managed-aware-sync");
        let music = root.join("Music");
        fs::create_dir(&music).unwrap();

        let first = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&music)],
        )
        .unwrap();
        assert!(first.changed);
        assert_eq!(fs::canonicalize(library.join("Music")).unwrap(), music);

        let second = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&music)],
        )
        .unwrap();
        assert!(!second.changed);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_reconcile_preserves_a_valid_compatible_generation() {
        let (root, library) = library_test_paths("managed-sync-valid");
        let music = root.join("Music");
        fs::create_dir_all(music.join("Album")).unwrap();
        fs::write(music.join("Album/track.flac"), b"audio").unwrap();
        publish_managed_library(&root, &library, &music);
        let before = tree_snapshot(&library);

        let result = MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&music)],
        )
        .unwrap();

        assert!(!result.changed);
        assert_eq!(result.library_dir, library);
        assert_eq!(tree_snapshot(&library), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_reconcile_rejects_corruption_without_mutation() {
        let (root, library) = library_test_paths("managed-sync-invalid");
        let music = root.join("Music");
        fs::create_dir_all(music.join("Album")).unwrap();
        fs::write(music.join("Album/track.flac"), b"audio").unwrap();
        publish_managed_library(&root, &library, &music);
        fs::write(
            library.join(crate::local_media::mirror::MANIFEST_NAME),
            b"invalid json",
        )
        .unwrap();
        let before = tree_snapshot(&library);

        assert!(MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&music)],
        )
        .is_err());
        assert_eq!(tree_snapshot(&library), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_reconcile_rejects_incompatible_topology_without_mutation() {
        let (root, library) = library_test_paths("managed-sync-topology-invalid");
        let music = root.join("Music");
        fs::create_dir_all(music.join("Album")).unwrap();
        fs::write(music.join("Album/track.flac"), b"audio").unwrap();
        publish_managed_library(&root, &library, &music);
        fs::write(library.join("unexpected-file"), b"unexpected").unwrap();
        let before = tree_snapshot(&library);

        assert!(MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&music)],
        )
        .is_err());
        assert_eq!(tree_snapshot(&library), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_reconcile_rejects_incompatible_config_without_mutation() {
        let (root, library) = library_test_paths("managed-sync-config-mismatch");
        let music = root.join("Music");
        let other = root.join("Other");
        fs::create_dir_all(music.join("Album")).unwrap();
        fs::create_dir_all(other.join("Album")).unwrap();
        fs::write(music.join("Album/track.flac"), b"audio").unwrap();
        fs::write(other.join("Album/other.flac"), b"audio").unwrap();
        let staging = root.join("library.next");
        crate::local_media::mirror::build_filtered_staging(
            &staging,
            &library,
            &[music.clone(), other],
            "supervisor-managed-sync-mismatch",
        )
        .unwrap();
        fs::remove_dir(&library).unwrap();
        fs::rename(staging, &library).unwrap();
        let before = tree_snapshot(&library);

        assert!(MpdSupervisor::reconcile_library_symlinks(
            &library,
            &[configured_folder(&music)],
        )
        .is_err());
        assert_eq!(tree_snapshot(&library), before);

        fs::remove_dir_all(root).unwrap();
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
    fn local_library_sources_support_managed_generations() {
        let (root, library) = library_test_paths("library-sources-managed");
        let first = root.join("one/Music");
        let second = root.join("two/Music");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let staging = root.join("library.next");
        crate::local_media::mirror::build_filtered_staging(
            &staging,
            &library,
            &[first.clone(), second.clone()],
            "source-discovery-managed",
        )
        .unwrap();
        fs::remove_dir(&library).unwrap();
        fs::rename(staging, &library).unwrap();
        let folders = [configured_folder(&second), configured_folder(&first)];

        let sources = MpdSupervisor::local_library_sources_in(&library, &folders).unwrap();
        assert_eq!(
            sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>(),
            vec!["Music", "Music (2)"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_library_sources_fail_closed_on_unexpected_legacy_entry() {
        let (root, library) = library_test_paths("library-sources-unexpected");
        let music = root.join("Music");
        fs::create_dir(&music).unwrap();
        fs::create_dir(library.join("UnexpectedDirectory")).unwrap();
        symlink(&music, library.join("Music")).unwrap();

        assert!(MpdSupervisor::local_library_sources_in(
            &library,
            &[configured_folder(&music)],
        )
        .is_err());
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

    #[test]
    fn engine_evidence_requires_the_owned_available_pid_and_is_cleared_on_stop() {
        let (dir,socket,pidfile)=test_runtime_paths("engine-evidence");
        let mut supervisor=MpdSupervisor::new_with_runtime_paths(&socket,&pidfile);
        let child=spawn_sleeping_child();let pid=child.id();
        supervisor.set_process_for_test(child,true);
        supervisor.engine_evidence=Some(crate::mpd_engine::EngineEvidence::Identified {
            evidence_id:"mpd-pcm-eof-20261017",scope:"pcm24_96000_stereo_gapless",pcm_eof_flush:true,
        });
        supervisor.health=MpdHealth::Starting;
        assert!(supervisor.engine_evidence_for_pid(pid).is_none());
        supervisor.mark_available();
        assert!(supervisor.engine_evidence_for_pid(pid).is_some());
        assert!(supervisor.engine_evidence_for_pid(pid+1).is_none());
        supervisor.owns_runtime_files=false;
        assert!(supervisor.engine_evidence_for_pid(pid).is_none());
        supervisor.stop().unwrap();
        assert!(supervisor.engine_evidence.is_none());
        assert!(supervisor.engine_evidence_for_pid(pid).is_none());
        fs::remove_dir_all(dir).unwrap();
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
    fn simple_database_is_versioned_and_exposes_playlist_targets() {
        let config_dir = Path::new("/tmp/sonante config\\with-quote\"");
        let database_path = MpdSupervisor::database_path_in(config_dir);
        assert_eq!(database_path, config_dir.join("mpd-v2.db"));

        let section = MpdSupervisor::database_section(&database_path).unwrap();
        assert!(section.contains("database {"));
        assert!(section.contains("plugin \"simple\""));
        assert!(section.contains("path \"/tmp/sonante config\\\\with-quote\\\"/mpd-v2.db\""));
        assert!(section.contains("hide_playlist_targets \"no\""));
        assert!(!section.contains("db_file"));

        let config = MpdSupervisor::render_config(
            "/tmp/library",
            "/tmp/playlists",
            &section,
            "/tmp/mpd.pid",
            "/tmp/mpd.socket",
            "4096",
            "off",
            "audio_output {}",
            "audio_output {}",
        );
        assert!(config.contains(&section));
        assert!(!config.contains("db_file"));
        assert!(!config.contains("state_file"));
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
        let section = MpdSupervisor::audio_output_section(
            true,
            "ignored",
            "yes",
            Some(SharedVolumeBackend::MpdSoftware),
        )
        .unwrap();

        assert!(section.contains("device \"default\""));
        assert!(section.contains("mixer_type \"software\""));
        assert!(!section.contains("mixer_control"));
        assert!(!section.contains("mixer_device"));
    }

    #[test]
    fn eq_rejects_direct_before_paths_process_or_format_changes() {
        let mut sup=MpdSupervisor::new("/no-such-eq-test/mpd.sock");
        let mut cfg=AppConfig::default();cfg.audio_output_type="alsa".into();cfg.alsa_device="hw:CARD=Fixture,DEV=0".into();
        for dop in [false,true] {cfg.dop_enabled=dop;
            assert_eq!(sup.start_shared_eq_output(&cfg,Some(PathBuf::from("/not-a-fifo"))).err().as_deref(),Some("EQ_SHARED_ONLY"));
            assert!(sup.shared_eq_pipe().is_none());assert!(!sup.output_only_restart);
        }
        assert_eq!(MpdSupervisor::shared_eq_output_section(false,Path::new("/no-path"),None),Err("EQ_SHARED_ONLY".into()));
    }

    #[test]
    fn shared_output_with_pipewire_backend_disables_mpd_mixer() {
        let section = MpdSupervisor::audio_output_section(
            true,
            "ignored",
            "no",
            Some(SharedVolumeBackend::PipeWire),
        )
        .unwrap();

        assert!(section.contains("device \"default\""));
        assert!(section.contains("mixer_type \"none\""));
        assert!(!section.contains("mixer_type \"software\""));
        assert!(!section.contains("mixer_control"));
    }

    #[test]
    fn direct_pcm_and_dop_always_use_software_mixer_without_hardware_controls() {
        for device in ["hw:CARD=SoundBar,DEV=0", "hw:2,0"] {
            for dop in ["no", "yes"] {
                let section = MpdSupervisor::audio_output_section(false, device, dop, None).unwrap();
                assert!(section.contains(&format!("device \"{}\"", device)));
                assert!(section.contains(&format!("dop \"{}\"", dop)));
                assert!(section.contains("mixer_type \"software\""));
                for hardware in ["mixer_device", "mixer_control", "mixer_index", "mixer_type \"hardware\""] {
                    assert!(!section.contains(hardware));
                }
            }
        }
    }

    #[test]
    fn shared_volume_capture_failure_keeps_rollback_silent_instead_of_assuming_unity() {
        let mut supervisor=MpdSupervisor::new("/unused/owned-shared-volume.socket");
        supervisor.capture_shared_software_volume(||Ok(vec!["volume: 43".into(),"state: play".into()])).unwrap();
        assert_eq!(supervisor.shared_software_volume,Some(43));
        assert!(supervisor.capture_shared_software_volume(||Err("timeout".into())).is_err());
        assert_eq!(supervisor.shared_software_volume,Some(0));
        assert!(supervisor.capture_shared_software_volume(||Ok(vec!["state: stop".into()])).is_err());
        assert_eq!(supervisor.shared_software_volume,Some(0));
    }

    #[test]
    fn public_volume_backend_matches_direct_policy_and_shared_backend() {
        for shared in [None, Some(SharedVolumeBackend::PipeWire), Some(SharedVolumeBackend::MpdSoftware)] {
            assert_eq!(MpdSupervisor::public_volume_backend(false, shared), VolumeBackend::DirectEquipment);
        }
        assert_eq!(MpdSupervisor::public_volume_backend(true, None), VolumeBackend::MpdSoftware);
        assert_eq!(MpdSupervisor::public_volume_backend(true, Some(SharedVolumeBackend::PipeWire)), VolumeBackend::PipeWire);
        assert_eq!(MpdSupervisor::public_volume_backend(true, Some(SharedVolumeBackend::MpdSoftware)), VolumeBackend::MpdSoftware);
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
    #[test]
    fn legacy_retired_ids_are_not_reused_after_empty_startup_reconciliation() {
        let (root, library) = library_test_paths("legacy-durable-identities");
        let a = root.join("old/Music");
        let b = root.join("new/Music");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&a)]).unwrap();
        MpdSupervisor::reconcile_library_symlinks(&library, &[]).unwrap();
        MpdSupervisor::reconcile_library_symlinks(&library, &[configured_folder(&b)]).unwrap();
        assert!(!library.join("Music").exists());
        assert_eq!(fs::canonicalize(library.join("Music (2)")).unwrap(), b);
        let sources =
            MpdSupervisor::local_library_sources_in(&library, &[configured_folder(&b)]).unwrap();
        assert_eq!(sources[0].id, "Music (2)");
        fs::remove_dir_all(root).unwrap();
    }

}
