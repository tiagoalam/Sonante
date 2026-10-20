//! Durable stop marker for Settings' managed folders transaction. No crash recovery.
use crate::audio::{
    MediaLocator, MpdPlaybackSnapshot, MpdQueueStructure, PlaybackState, TrackMetadata,
};
use crate::config::AppConfig;
use crate::local_media::mirror::{
    self, ManagedMirrorPublication, ManagedRefreshPlan, MirrorBuildResult, MirrorManifest,
    MANIFEST_NAME,
};
use crate::persistence::{open_sync_directory, validate_sync_directory};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) const NAME: &str = ".managed-folders-save-intent.json";
pub(crate) const BLOCKED: &str = "MANAGED_FOLDERS_SAVE_PENDING";
const MAX_RECORD_BYTES: u64 = 262_144;

fn blocked(detail: &str) -> String {
    format!("{BLOCKED}: {detail} Início automático do MPD bloqueado; config, library e library.next devem ser preservados para revisão manual.")
}

fn fingerprint(value: &impl Serialize) -> Result<[u8; 32], String> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| blocked("Não foi possível vincular a intenção."))?;
    Ok(Sha256::digest(bytes).into())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    device: u64,
    inode: u64,
    manifest: Option<MirrorManifest>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    previous_config: [u8; 32],
    next_config: [u8; 32],
    previous: Generation,
    next: Generation,
    // Omission preserves the exact v1 checksum/serialization contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    queue_backup: Option<QueueBackup>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    intent: Intent,
    checksum: [u8; 32],
}

/// Recoverable identities, never runtime Plex URLs or artwork. MPD references
/// point into the ordered logical tracks; duplicates are separate occurrences.
/// This is a stop-marker payload, not automatic crash recovery.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueueBackup {
    version: u32,
    pub(crate) tracks: Vec<BackupTrack>,
    pub(crate) mpd: Vec<BackupMpdReference>,
    pub(crate) playlist_version: u64,
    pub(crate) current_index: Option<usize>,
    pub(crate) state: PlaybackState,
    pub(crate) elapsed: f64,
    // Omit for v1 so existing journal checksums remain byte-for-byte compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) logical_resume: Option<BackupLogicalResume>,
}

/// Logical cursor only: it does not claim that the stopped MPD has this cursor.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupLogicalResume {
    pub(crate) queue_index: usize,
    pub(crate) elapsed: f64,
    pub(crate) state: PlaybackState,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupTrack {
    pub(crate) media: BackupMedia,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) duration: Option<f64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum BackupMedia {
    Local {
        uri: String,
    },
    Plex {
        server_id: String,
        part_key: String,
        rating_key: Option<String>,
        file_path: Option<String>,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupMpdReference {
    pub(crate) track_index: usize,
    pub(crate) pos: usize,
    pub(crate) id: u64,
}

fn backup_text_safe(value: &str) -> bool {
    !value.contains(['\0', '\r', '\n'])
        && !value.contains("://")
        && !value.starts_with("//")
        && !crate::plex::contains_plex_token(value)
}

impl QueueBackup {
    /// Caller supplies the URIs resolved for these logical occurrences. They
    /// must match both MPD captures but are checked only in memory, not saved.
    /// Caller must reserve queue structure and capture/revalidate before clearing.
    #[allow(dead_code)] // Infrastructure only; Save is deliberately not wired yet.
    pub(crate) fn new(
        tracks: &[TrackMetadata],
        resolved_uris: &[String],
        queue: &MpdQueueStructure,
        playback: &MpdPlaybackSnapshot,
    ) -> Result<Self, String> {
        if tracks.len() != resolved_uris.len()
            || tracks.len() != queue.entries.len()
            || playback.uris != resolved_uris
            || playback.playlist_version != queue.playlist_version
        {
            return Err(blocked("Capturas da fila não correspondem."));
        }
        let mut logical = Vec::with_capacity(tracks.len());
        let mut mpd = Vec::with_capacity(tracks.len());
        for (index, ((track, resolved), entry)) in tracks
            .iter()
            .zip(resolved_uris)
            .zip(&queue.entries)
            .enumerate()
        {
            if entry.uri != *resolved
                || resolved.is_empty()
                || resolved.contains(['\0', '\r', '\n'])
            {
                return Err(blocked("Referências MPD da fila não correspondem."));
            }
            let media = match &track.media_locator {
                Some(MediaLocator::Local { uri }) if uri == &track.uri && uri == resolved => {
                    BackupMedia::Local { uri: uri.clone() }
                }
                None if track.uri == *resolved => BackupMedia::Local {
                    uri: track.uri.clone(),
                },
                Some(MediaLocator::Plex {
                    server_id,
                    part_key,
                    rating_key,
                    file_path,
                }) => BackupMedia::Plex {
                    server_id: server_id.clone(),
                    part_key: part_key.clone(),
                    rating_key: rating_key.clone(),
                    file_path: file_path.clone(),
                },
                _ => return Err(blocked("Identidade lógica da fila ambígua.")),
            };
            logical.push(BackupTrack {
                media,
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone(),
                duration: track.duration,
            });
            mpd.push(BackupMpdReference {
                track_index: index,
                pos: entry.pos,
                id: entry.id,
            });
        }
        let backup = Self {
            version: 1,
            tracks: logical,
            mpd,
            playlist_version: queue.playlist_version,
            current_index: playback.current_index,
            state: playback.state,
            elapsed: playback.elapsed,
            logical_resume: None,
        };
        backup.validate()?;
        Ok(backup)
    }

    /// Infrastructure only. The owner must reserve/revalidate the queue and
    /// logical cursor before any future clear; this does not authorize a clear.
    #[allow(dead_code)]
    pub(crate) fn new_with_logical_resume(
        tracks: &[TrackMetadata],
        resolved_uris: &[String],
        queue: &MpdQueueStructure,
        playback: &MpdPlaybackSnapshot,
        resume: BackupLogicalResume,
    ) -> Result<Self, String> {
        // Reuse all occurrence, media identity and MPD capture validation.
        let mut backup = Self::new(tracks, resolved_uris, queue, playback)?;
        backup.version = 2;
        backup.logical_resume = Some(resume);
        backup.validate()?;
        Ok(backup)
    }

    fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        let version_valid = match (self.version, &self.logical_resume) {
            (1, None) => true,
            (2, Some(resume)) => {
                self.state == PlaybackState::Stopped
                    && self.current_index.is_none()
                    && self.elapsed == 0.0
                    && resume.queue_index < self.tracks.len()
                    && resume.elapsed.is_finite()
                    && resume.elapsed >= 0.0
                    && matches!(resume.state, PlaybackState::Playing | PlaybackState::Paused)
            }
            _ => false,
        };
        let valid = version_valid
            && self.tracks.len() == self.mpd.len()
            && self.elapsed.is_finite()
            && self.elapsed >= 0.0
            && self
                .current_index
                .is_none_or(|index| index < self.tracks.len())
            && (self.state == PlaybackState::Stopped || self.current_index.is_some())
            && (!self.tracks.is_empty()
                || (self.state == PlaybackState::Stopped && self.elapsed == 0.0))
            && self.mpd.iter().enumerate().all(|(index, entry)| {
                entry.track_index == index && entry.pos == index && ids.insert(entry.id)
            })
            && self.tracks.iter().all(|track| {
                let media_valid = match &track.media {
                    BackupMedia::Local { uri } => {
                        crate::local_media::is_safe_relative_mpd_uri(uri) && backup_text_safe(uri)
                    }
                    BackupMedia::Plex {
                        server_id,
                        part_key,
                        rating_key,
                        file_path,
                    } => {
                        !server_id.is_empty()
                            && server_id
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                            && backup_text_safe(server_id)
                            && part_key.starts_with('/')
                            && !part_key.starts_with("//")
                            && !part_key.contains(['?', '#'])
                            && backup_text_safe(part_key)
                            && rating_key.as_deref().is_none_or(|key| {
                                !key.is_empty()
                                    && key.trim() == key
                                    && backup_text_safe(key)
                                    && !key.contains(['/', '?', '#', ':', '@'])
                            })
                            && file_path.as_deref().is_none_or(|path| {
                                Path::new(path).is_absolute()
                                    && backup_text_safe(path)
                                    && !path.contains(['?', '#'])
                            })
                    }
                };
                media_valid
                    && [&track.title, &track.artist, &track.album]
                        .into_iter()
                        .all(|s| backup_text_safe(s))
                    && track
                        .duration
                        .is_none_or(|duration| duration.is_finite() && duration >= 0.0)
            });
        if !valid {
            return Err(blocked("Respaldo da fila inválido ou inseguro."));
        }
        Ok(())
    }
}

struct Parent {
    path: PathBuf,
    directory: File,
}

impl Parent {
    fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            path: path.to_path_buf(),
            directory: open_sync_directory(path)
                .map_err(|_| blocked("Diretório privado inseguro ou indisponível."))?,
        })
    }

    fn inspect(path: &Path) -> Result<Self, String> {
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)
            .map_err(|_| blocked("Diretório de configuração inseguro ou indisponível."))?;
        let parent = Self {
            path: path.to_path_buf(),
            directory,
        };
        parent.validate_inspection()?;
        Ok(parent)
    }

    fn validate_inspection(&self) -> Result<(), String> {
        let opened = self
            .directory
            .metadata()
            .map_err(|_| blocked("Diretório indisponível."))?;
        let current =
            fs::symlink_metadata(&self.path).map_err(|_| blocked("Diretório substituído."))?;
        // Read-only startup inspection also accepts legacy 0755 directories.
        // AppConfig retains its existing hardening only after this guard succeeds.
        if !current.is_dir()
            || opened.uid() != unsafe { libc::geteuid() }
            || opened.permissions().mode() & 0o022 != 0
            || opened.dev() != current.dev()
            || opened.ino() != current.ino()
        {
            return Err(blocked("Diretório substituído ou gravável por terceiros."));
        }
        Ok(())
    }

    fn entry(&self, name: &str) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd())).join(name)
    }

    fn validate(&self) -> Result<(), String> {
        validate_sync_directory(&self.path, &self.directory)
            .map_err(|_| blocked("A identidade do diretório privado mudou."))
    }

    fn sync_with(&self, sync: &impl Fn(&File) -> std::io::Result<()>) -> Result<(), String> {
        self.validate()?;
        sync(&self.directory).map_err(|_| blocked("Sincronização do diretório não confirmada."))?;
        self.validate()
    }

    fn config_fingerprint(&self) -> Result<[u8; 32], String> {
        let bytes = read_regular(&self.entry("config.json"), true)?;
        let mut config: AppConfig = serde_json::from_slice(&bytes)
            .map_err(|_| blocked("A configuração persistida não pôde ser validada."))?;
        config.apply_legacy_migrations();
        fingerprint(&config)
    }
}

fn open_regular(path: &Path, private: bool) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| blocked("Arquivo local inseguro ou indisponível."))?;
    let metadata = file
        .metadata()
        .map_err(|_| blocked("Identidade do arquivo indisponível."))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || (private && metadata.permissions().mode() & 0o777 != 0o600)
    {
        return Err(blocked("Arquivo local não é privado ou exclusivo."));
    }
    require_file_identity(path, &file)?;
    Ok(file)
}

fn require_file_identity(path: &Path, file: &File) -> Result<(), String> {
    let actual =
        fs::symlink_metadata(path).map_err(|_| blocked("Arquivo substituído ou removido."))?;
    let opened = file
        .metadata()
        .map_err(|_| blocked("Identidade do arquivo indisponível."))?;
    if !actual.is_file() || actual.dev() != opened.dev() || actual.ino() != opened.ino() {
        return Err(blocked("Arquivo substituído durante a operação."));
    }
    Ok(())
}

fn read_regular(path: &Path, private: bool) -> Result<Vec<u8>, String> {
    let mut file = open_regular(path, private)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| blocked("Leitura do arquivo local não confirmada."))?;
    require_file_identity(path, &file)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(blocked("Arquivo local excede o limite de validação."));
    }
    Ok(bytes)
}

fn generation(path: &Path) -> Result<Generation, String> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| blocked("Geração local indisponível ou insegura."))?;
    let metadata = directory
        .metadata()
        .map_err(|_| blocked("Identidade da geração indisponível."))?;
    let actual = fs::symlink_metadata(path).map_err(|_| blocked("Geração local substituída."))?;
    if !actual.is_dir() || actual.dev() != metadata.dev() || actual.ino() != metadata.ino() {
        return Err(blocked("Geração local substituída."));
    }
    let marker =
        PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(MANIFEST_NAME);
    let manifest = match fs::symlink_metadata(&marker) {
        Ok(_) => Some(
            serde_json::from_slice(&read_regular(&marker, false)?)
                .map_err(|_| blocked("Manifesto da geração inválido."))?,
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(_) => return Err(blocked("Manifesto da geração indisponível.")),
    };
    Ok(Generation {
        device: metadata.dev(),
        inode: metadata.ino(),
        manifest,
    })
}

// Only mirror directories and their manifest are opened. Physical symlink targets
// (including NAS paths) are never traversed or synchronized.
pub(crate) fn sync_mirror(path: &Path, depth: usize) -> Result<(), String> {
    if depth > 256 {
        return Err(blocked("Mirror excede o limite de profundidade."));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| blocked("Mirror inseguro para sincronização."))?;
    let anchored = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    for entry in
        fs::read_dir(&anchored).map_err(|_| blocked("Leitura do mirror não confirmada."))?
    {
        let entry = entry.map_err(|_| blocked("Entrada do mirror indisponível."))?;
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|_| blocked("Entrada do mirror mudou."))?;
        if metadata.is_dir() {
            sync_mirror(&entry.path(), depth + 1)?;
        } else if metadata.is_file() && entry.file_name() == MANIFEST_NAME {
            open_regular(&entry.path(), false)?
                .sync_all()
                .map_err(|_| blocked("Sincronização do manifesto não confirmada."))?;
        } else if !metadata.file_type().is_symlink() {
            return Err(blocked("Entrada desconhecida no mirror; preservada."));
        }
    }
    directory
        .sync_all()
        .map_err(|_| blocked("Sincronização do mirror não confirmada."))?;
    let current = fs::symlink_metadata(path).map_err(|_| blocked("Mirror substituído."))?;
    let opened = directory
        .metadata()
        .map_err(|_| blocked("Mirror indisponível."))?;
    if !current.is_dir() || current.dev() != opened.dev() || current.ino() != opened.ino() {
        return Err(blocked("Mirror substituído durante a sincronização."));
    }
    Ok(())
}

fn read_record(parent: &Parent) -> Result<Record, String> {
    parse_record(&read_regular(&parent.entry(NAME), true)?)
}

fn parse_record(bytes: &[u8]) -> Result<Record, String> {
    if bytes.len() as u64 > MAX_RECORD_BYTES { return Err(blocked("Registro excede limite.")); }
    let record: Record = serde_json::from_slice(bytes)
        .map_err(|_| blocked("Registro de intenção inválido ou incompleto."))?;
    let version_valid = match (record.intent.version, &record.intent.queue_backup) {
        (1, None) => true,
        (2, Some(backup)) => {
            backup.validate()?;
            true
        }
        _ => false,
    };
    if !version_valid
        || fingerprint(&record.intent)? != record.checksum
        || record.intent.previous == record.intent.next
        || record.intent.next.manifest.is_none()
    {
        return Err(blocked(
            "Integridade do registro de intenção não confirmada.",
        ));
    }
    Ok(record)
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryBinding {
    pub record_hash: String,
    pub previous_config: [u8; 32],
    pub next_config: [u8; 32],
    pub previous_identity: crate::recovery_fs::Identity,
    pub next_identity: crate::recovery_fs::Identity,
    pub previous_manifest: Option<MirrorManifest>,
    pub next_manifest: Option<MirrorManifest>,
    pub queue_cache: Option<Vec<u8>>,
}
pub(crate) fn recovery_binding(bytes: &[u8]) -> Result<RecoveryBinding, String> {
    let record = parse_record(bytes)?;
    Ok(RecoveryBinding {
        record_hash: crate::recovery_fs::hash(bytes),
        previous_config: record.intent.previous_config, next_config: record.intent.next_config,
        previous_identity: crate::recovery_fs::Identity { dev: record.intent.previous.device, ino: record.intent.previous.inode },
        next_identity: crate::recovery_fs::Identity { dev: record.intent.next.device, ino: record.intent.next.inode },
        previous_manifest: record.intent.previous.manifest, next_manifest: record.intent.next.manifest,
        queue_cache: record.intent.queue_backup.map(|b|b.recovery_cache()).transpose()?,
    })
}
pub(crate) fn recovery_config_hash(bytes:&[u8])->Result<[u8;32],String>{
    let mut config:AppConfig=serde_json::from_slice(bytes).map_err(|_|blocked("Configuração inválida."))?;
    config.apply_legacy_migrations(); fingerprint(&config)
}
impl QueueBackup {
    fn recovery_cache(&self)->Result<Vec<u8>,String>{
        self.validate()?;
        let tracks=self.tracks.iter().map(|t| {
            let (uri,locator)=match &t.media {
                BackupMedia::Local{uri}=>(uri.clone(),serde_json::json!({"kind":"local","uri":uri})),
                BackupMedia::Plex{server_id,part_key,rating_key,file_path}=>(String::new(),serde_json::json!({"kind":"plex","server_id":server_id,"part_key":part_key,"rating_key":rating_key,"file_path":file_path})),
            };
            serde_json::json!({"title":t.title,"artist":t.artist,"album":t.album,"duration":t.duration,"uri":uri,"media_locator":locator})
        }).collect::<Vec<_>>();
        let resume=self.logical_resume.as_ref().map(|r|(r.queue_index,r.elapsed))
            .or_else(||self.current_index.map(|i|(i,self.elapsed)));
        let mut cache=serde_json::json!({"version":2,"tracks":tracks});
        if let Some((index,elapsed))=resume{cache["resume"]=serde_json::json!({"queue_index":index,"elapsed":elapsed,"state":"paused"});}
        serde_json::to_vec(&cache).map_err(|_|blocked("Fila recuperável indisponível."))
    }
}

pub(crate) fn check_startup(config_dir: &Path) -> Result<(), String> {
    check_startup_state(config_dir, false)
}

// Caller must hold the read gate of a live reconciliation admitted by
// check_startup before it could create staging. Journals still fail closed.
pub(crate) fn check_active_startup_reconciliation(config_dir: &Path) -> Result<(), String> {
    check_startup_state(config_dir, true)
}

fn check_startup_state(config_dir: &Path, startup_owns_staging: bool) -> Result<(), String> {
    match fs::symlink_metadata(config_dir) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(blocked("Diretório privado indisponível.")),
        Ok(_) => {}
    }
    let parent = Parent::inspect(config_dir)?;
    match fs::symlink_metadata(parent.entry(crate::recovery_set::SEAL)) {
        Ok(_) => return Err(blocked("Selo de recuperação sem finalização durável.")),
        Err(e) if e.kind()==ErrorKind::NotFound => {},
        Err(_) => return Err(blocked("Selo de recuperação indisponível.")),
    }
    match fs::symlink_metadata(parent.entry(crate::recovery_set::PENDING)) {
        Ok(_) => return Err(blocked("Conjunto de recuperação sem finalização durável.")),
        Err(e) if e.kind()==ErrorKind::NotFound => {},
        Err(_) => return Err(blocked("Conjunto de recuperação indisponível.")),
    }
    match fs::symlink_metadata(parent.entry(NAME)) {
        Ok(_) => {
            read_record(&parent)?;
            return Err(blocked("Há uma transação de pastas locais pendente."));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return Err(blocked("Estado do registro de intenção indisponível.")),
    }
    if !startup_owns_staging {
        match fs::symlink_metadata(parent.entry("library.next")) {
            Ok(_) => return Err(blocked("Staging sem conclusão comprovada; estado ambíguo.")),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(blocked("Estado da staging indisponível.")),
        }
    }
    parent.validate_inspection()
}

pub(crate) fn guard_startup<T>(
    config_dir: &Path,
    ready: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    check_startup(config_dir)?;
    ready()
}

/// Diagnose only. Valid intent/config/mirror correspondence never authorizes
/// startup, rollback, cleanup, DB repair, or audio. Physical roots stay unopened.
pub(crate) fn inspect_recovery(
    config_dir: &Path, analysis_id: u64, cancelled: &dyn Fn() -> bool,
) -> Result<crate::library_recovery::Observation, String> {
    use crate::library_recovery::{ConfigMatch, Observation, Reason, CANCELLED};
    let mut result = Observation::new(analysis_id);
    let check = || if cancelled() { Err(CANCELLED.to_string()) } else { Ok(()) };
    check()?;
    let parent = match Parent::inspect(config_dir) {
        Ok(parent) => parent,
        Err(_) => { result.note(Reason::UnsafeEntry); return Ok(result); }
    };
    // All paths have fixed backend-owned names. No path comes from IPC.
    let names = [NAME, "config.json", "library", "library.next", "mpd-v2.db",
        "queue_cache.json", "favorites.json", "playlists.json", "mpd.conf",
        "local-source-identities.json", "local-source-identities.lock", "equalizer-shared-v1.json"];
    let stamp = |name: &str| -> Result<Option<(u64, u64, u64, u32, i64, i64, i64, i64)>, ()> {
        match fs::symlink_metadata(parent.entry(name)) {
            Ok(m) => Ok(Some((m.dev(), m.ino(), m.size(), m.mode(), m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()))),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(_) => Err(()),
        }
    };
    let before = names.iter().map(|n| stamp(n)).collect::<Vec<_>>();
    let present = |name: &str| matches!(stamp(name), Ok(Some(_)));
    for name in [NAME, "config.json", "local-source-identities.json", "local-source-identities.lock"] {
        if present(name) && open_regular(&parent.entry(name), true).is_err() { result.note(Reason::UnsafeEntry); }
    }
    let record = if present(NAME) {
        match read_record(&parent) {
            Ok(record) => {
                result.note(Reason::IntentValid);
                result.intent_version = Some(record.intent.version);
                result.queue_backup_version = record.intent.queue_backup.as_ref().map(|b| b.version);
                Some(record)
            }
            Err(_) => { result.note(Reason::IntentInvalid); None }
        }
    } else { None };
    if !present(NAME) {
        result.note(if present("library.next") { Reason::OrphanStaging } else { Reason::NoPendingMarker });
    }
    check()?;
    let config = if present("config.json") {
        match parent.config_fingerprint() {
            Ok(hash) => {
                result.config_matches = match &record {
                    Some(r) => match (hash == r.intent.previous_config, hash == r.intent.next_config) {
                        (true, true) => ConfigMatch::Both, (true, false) => ConfigMatch::Previous,
                        (false, true) => ConfigMatch::Next,
                        _ => { result.note(Reason::ConfigDivergent); ConfigMatch::Unbound }
                    },
                    None => ConfigMatch::Unbound,
                };
                Some(hash)
            }
            Err(_) => { result.note(Reason::ConfigInvalid); None }
        }
    } else { result.note(Reason::ConfigMissing); None };
    let mut generations = Vec::new();
    for name in ["library", "library.next"] {
        check()?;
        if !present(name) {
            if name == "library" { result.note(Reason::MirrorMissing); }
            continue;
        }
        if fs::symlink_metadata(parent.entry(name)).is_ok_and(|m|
            !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0)
        { result.note(Reason::UnsafeEntry); }
        match generation(&parent.entry(name)) {
            Ok(g) => {
                if let Some(manifest) = &g.manifest {
                    if mirror::validate_manifest(manifest).is_err() { result.note(Reason::MirrorInvalid); }
                }
                if let Some(r) = &record {
                    if g != r.intent.previous && g != r.intent.next { result.note(Reason::GenerationDivergent); }
                }
                generations.push((name, g));
            }
            Err(_) => result.note(Reason::MirrorInvalid),
        }
    }
    // Only inspect types/identity for these controls. DB presence is explicitly
    // unproven coverage; this phase never hashes/loads the DB or queries MPD.
    for name in ["mpd-v2.db", "queue_cache.json", "favorites.json", "playlists.json", "mpd.conf", "equalizer-shared-v1.json"] {
        check()?;
        if present(name) {
            if open_regular(&parent.entry(name), name != "mpd-v2.db" && name != "mpd.conf").is_err() {
                result.note(Reason::UnsafeEntry);
            }
        }
    }
    result.note(if present("mpd-v2.db") { Reason::DatabaseUnproven } else { Reason::DatabaseMissing });
    let ledger = "local-source-identities.json";
    let lock = "local-source-identities.lock";
    // Ledger may exceed the intent's own bound. Limit and cancel chunked reads
    // explicitly, while retaining the original nofollow/type/UID/nlink gates.
    let bounded = |name: &str, limit: u64| -> Result<Vec<u8>, String> {
        let path = parent.entry(name);
        let mut file = open_regular(&path, true)?;
        let mut bytes = Vec::new();
        loop {
            check()?;
            let mut chunk = [0u8; 16_384];
            let n = file.read(&mut chunk).map_err(|_| blocked("Leitura não confirmada."))?;
            if n == 0 { break; }
            if bytes.len() as u64 + n as u64 > limit { return Err(blocked("Limite de inspeção excedido.")); }
            bytes.extend_from_slice(&chunk[..n]);
        }
        require_file_identity(&path, &file)?;
        Ok(bytes)
    };
    let manifests = generations.iter().filter_map(|(_, g)| g.manifest.clone()).collect::<Vec<_>>();
    if present(ledger) && present(lock) {
        let pair = bounded(ledger, 16 * 1024 * 1024).and_then(|bytes| {
            let anchor = bounded(lock, 32)?;
            crate::source_identity::validate_recovery_snapshot(&bytes, &anchor, &manifests)
                .map_err(|_| blocked("Identidades divergentes."))
        });
        if pair.is_err() { check()?; result.note(Reason::LedgerDivergent); }
    } else {
        result.note(Reason::LedgerMissing);
    }
    check()?;
    // Parent and entry fences reject replacement/in-place edits observed during
    // inspection. This is not an atomic snapshot of a concurrent filesystem.
    if parent.validate_inspection().is_err()
        || before.iter().any(Result::is_err)
        || names.iter().zip(&before).any(|(n, prior)| stamp(n).as_ref() != prior.as_ref())
        || config.is_some_and(|hash| parent.config_fingerprint().ok() != Some(hash))
        || generations.iter().any(|(name, expected)| generation(&parent.entry(name)).ok().as_ref() != Some(expected))
    {
        result.note(Reason::FilesystemChanged);
    }
    result.note(Reason::OwnershipUnknown);
    check()?;
    Ok(result)
}

pub(crate) struct SaveIntent {
    parent: Parent,
    record: Record,
    file: File,
    queue_cache_proof: Option<(File, [u8; 32])>,
}

#[derive(Clone, Copy)]
pub(crate) enum Completed {
    Committed,
    Recovered,
}

impl SaveIntent {
    pub(crate) fn create(
        config_dir: &Path,
        previous: &AppConfig,
        next: &AppConfig,
        prepared: &MirrorBuildResult,
    ) -> Result<Self, String> {
        Self::create_with_sync(config_dir, previous, next, prepared, &File::sync_all)
    }

    fn create_with_sync(
        config_dir: &Path,
        previous: &AppConfig,
        next: &AppConfig,
        prepared: &MirrorBuildResult,
        sync: &impl Fn(&File) -> std::io::Result<()>,
    ) -> Result<Self, String> {
        Self::create_record(config_dir, previous, next, prepared, None, sync)
    }

    /// Creates the intention and backup together. Success proves file and
    /// parent-directory fsync; failure must never authorize a future queue clear.
    /// No separate backup file or in-place journal update can be left unbound.
    #[allow(dead_code)]
    pub(crate) fn create_with_queue_backup(
        config_dir: &Path,
        previous: &AppConfig,
        next: &AppConfig,
        prepared: &MirrorBuildResult,
        backup: &QueueBackup,
    ) -> Result<Self, String> {
        Self::create_record(
            config_dir,
            previous,
            next,
            prepared,
            Some(backup),
            &File::sync_all,
        )
    }

    /// Integrity-only consultation of a pending record, also after restart.
    /// This never clears the startup stop marker or proves a failed fsync succeeded.
    #[allow(dead_code)]
    pub(crate) fn read_queue_backup(config_dir: &Path) -> Result<Option<QueueBackup>, String> {
        let parent = Parent::inspect(config_dir)?;
        let record = read_record(&parent)?;
        parent.validate_inspection()?;
        Ok(record.intent.queue_backup)
    }

    /// The destructive owner supplies a verified queue encoding. Reuse the
    /// existing typed durable writer; regular queue saves are unchanged.
    pub(crate) fn persist_queue_cache(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.persist_queue_cache_with(bytes, |path, parent, bytes| {
            crate::persistence::atomic_write_private_synced_in(
                path,
                parent,
                bytes,
                "queue_cache.json",
            )
        })
    }

    fn persist_queue_cache_with(
        &mut self,
        bytes: &[u8],
        write: impl FnOnce(&Path, &File, &[u8]) -> Result<(), crate::config::ConfigSaveError>,
    ) -> Result<(), String> {
        self.validate_record()?;
        if self.record.intent.queue_backup.is_none() || bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(blocked(
                "Persistência da fila não autorizada pela intenção.",
            ));
        }
        self.parent.validate()?;
        write(
            &self.parent.path.join("queue_cache.json"),
            &self.parent.directory,
            bytes,
        )
        .map_err(|_| blocked("Persistência durável da fila não confirmada."))?;
        self.parent.validate()?;
        let path = self.parent.entry("queue_cache.json");
        let file = open_regular(&path, true)?;
        if read_regular(&path, true)? != bytes {
            return Err(blocked("Cache da fila mudou após persistência."));
        }
        require_file_identity(&path, &file)?;
        self.queue_cache_proof = Some((file, Sha256::digest(bytes).into()));
        Ok(())
    }

    fn validate_queue_cache_proof(&self) -> Result<(), String> {
        if self.record.intent.queue_backup.is_some() {
            let (file, checksum) = self
                .queue_cache_proof
                .as_ref()
                .ok_or_else(|| blocked("Conclusão da fila não comprovada."))?;
            let path = self.parent.entry("queue_cache.json");
            require_file_identity(&path, file)?;
            if <[u8; 32]>::from(Sha256::digest(read_regular(&path, true)?)) != *checksum {
                return Err(blocked("Cache da fila mudou antes da conclusão."));
            }
            require_file_identity(&path, file)?;
        }
        Ok(())
    }

    pub(crate) fn check_queue_cache_completion(&self) -> Result<(), String> {
        self.validate_record()?;
        self.validate_queue_cache_proof()
    }

    fn create_record(
        config_dir: &Path,
        previous: &AppConfig,
        next: &AppConfig,
        prepared: &MirrorBuildResult,
        backup: Option<&QueueBackup>,
        sync: &impl Fn(&File) -> std::io::Result<()>,
    ) -> Result<Self, String> {
        Self::create_record_with_io(
            config_dir,
            previous,
            next,
            prepared,
            backup,
            sync,
            &|file, bytes| file.write_all(bytes),
        )
    }

    fn create_record_with_io(
        config_dir: &Path,
        previous: &AppConfig,
        next: &AppConfig,
        prepared: &MirrorBuildResult,
        backup: Option<&QueueBackup>,
        sync: &impl Fn(&File) -> std::io::Result<()>,
        write: &impl Fn(&mut File, &[u8]) -> std::io::Result<()>,
    ) -> Result<Self, String> {
        if let Some(backup) = backup {
            backup.validate()?;
        }
        let parent = Parent::open(config_dir)?;
        if prepared.staging_path != config_dir.join("library.next") {
            return Err(blocked("Staging não pertence ao diretório da transação."));
        }
        let previous_config = fingerprint(previous)?;
        if parent.config_fingerprint()? != previous_config {
            return Err(blocked("A configuração anterior mudou antes da intenção."));
        }
        let intent = Intent {
            version: if backup.is_some() { 2 } else { 1 },
            previous_config,
            next_config: fingerprint(next)?,
            previous: generation(&parent.entry("library"))?,
            next: generation(&parent.entry("library.next"))?,
            queue_backup: backup.cloned(),
        };
        if intent.next.manifest.as_ref() != Some(&prepared.manifest) {
            return Err(blocked("A staging não corresponde à geração preparada."));
        }
        sync_mirror(&parent.entry("library"), 0)?;
        sync_mirror(&parent.entry("library.next"), 0)?;
        let record = Record {
            checksum: fingerprint(&intent)?,
            intent,
        };
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| blocked("Falha ao preparar registro de intenção."))?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err(blocked("Registro de intenção excede o limite suportado."));
        }
        parent.validate()?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(parent.entry(NAME))
            .map_err(|_| {
                blocked("Não foi possível criar intenção exclusiva; registro existente preservado.")
            })?;
        // Any partial write or failed fsync leaves the stop marker in place.
        write(&mut file, &bytes).map_err(|_| blocked("Gravação da intenção não confirmada."))?;
        sync(&file).map_err(|_| blocked("Sincronização da intenção não confirmada."))?;
        parent.sync_with(sync)?;
        let guard = Self {
            parent,
            record,
            file,
            queue_cache_proof: None,
        };
        guard.before_exchange()?;
        Ok(guard)
    }

    /// Live, unpublished draft only: no queue clear, exchange, update or config
    /// commit has occurred. This is not a crash-recovery escape hatch.
    pub(crate) fn abort_unpublished(&self,prepared:&MirrorBuildResult)->Result<(),String>{
        self.before_exchange()?;
        if self.parent.entry(crate::recovery_set::PENDING).try_exists().map_err(|_|blocked("Conjunto incerto."))?
            ||self.parent.entry(crate::recovery_set::SEAL).try_exists().map_err(|_|blocked("Selo incerto."))?{return Err(blocked("Conjunto parcial preservado."))}
        mirror::cleanup_prepared_managed_generation(prepared)?;
        self.validate_record()?;
        if self.parent.config_fingerprint()?!=self.record.intent.previous_config
            ||generation(&self.parent.entry("library"))?!=self.record.intent.previous
            ||self.parent.entry("library.next").try_exists().map_err(|_|blocked("Staging incerta."))?{
            return Err(blocked("Draft alterado; intenção preservada."));
        }
        self.parent.sync_with(&File::sync_all)?;
        require_file_identity(&self.parent.entry(NAME),&self.file)?;
        fs::remove_file(self.parent.entry(NAME)).map_err(|_|blocked("Intenção não removida."))?;
        self.parent.sync_with(&File::sync_all)
    }

    fn validate_record(&self) -> Result<(), String> {
        self.parent.validate()?;
        require_file_identity(&self.parent.entry(NAME), &self.file)?;
        if read_record(&self.parent)?.checksum != self.record.checksum {
            return Err(blocked("Registro de intenção foi adulterado."));
        }
        Ok(())
    }

    pub(crate) fn before_exchange(&self) -> Result<(), String> {
        self.validate_record()?;
        if generation(&self.parent.entry("library"))? != self.record.intent.previous
            || generation(&self.parent.entry("library.next"))? != self.record.intent.next
            || self.parent.config_fingerprint()? != self.record.intent.previous_config
        {
            return Err(blocked(
                "Configuração ou identidades mudaram antes do exchange.",
            ));
        }
        Ok(())
    }

    pub(crate) fn publish(
        &self,
        prepared: &MirrorBuildResult,
        targeted: Option<&ManagedRefreshPlan>,
    ) -> Result<ManagedMirrorPublication, String> {
        let mut anchored = prepared.clone();
        anchored.staging_path = self.parent.entry("library.next");
        let live = self.parent.entry("library");
        match targeted {
            Some(plan) => mirror::publish_targeted_added_sources_generation_guarded(
                &live,
                &anchored,
                plan,
                || self.before_exchange(),
            ),
            None => mirror::publish_prepared_managed_generation_guarded(&live, &anchored, || {
                self.before_exchange()
            }),
        }
    }

    fn finish(&self, completed: Completed) -> Result<(), String> {
        self.finish_with_sync(completed, &File::sync_all)
    }

    pub(crate) fn publish_removal(
        &self,
        prepared: &MirrorBuildResult,
        plan: &mirror::PureSourceRemoval,
    ) -> Result<ManagedMirrorPublication, String> {
        let mut anchored = prepared.clone();
        anchored.staging_path = self.parent.entry("library.next");
        mirror::publish_targeted_removed_sources_generation_guarded(
            &self.parent.entry("library"), &anchored, plan, || self.before_exchange(),
        )
    }

    #[cfg(test)]
    pub(crate) fn complete_transaction(
        &self,
        result: Result<(), String>,
        completed: Option<Completed>,
    ) -> Result<(), String> {
        self.conclude_transaction(result, completed).0
    }

    /// Preserve the proof of durable completion for the background operation.
    /// In particular, failed unlink fsync can leave no visible record while
    /// its removal is still unconfirmed; absence alone cannot classify failure.
    pub(crate) fn conclude_transaction(
        &self,
        result: Result<(), String>,
        completed: Option<Completed>,
    ) -> (Result<(), String>, bool) {
        Self::conclude_with(result, completed, |proof| self.finish(proof))
    }

    fn conclude_with(
        result: Result<(), String>,
        completed: Option<Completed>,
        finish: impl FnOnce(Completed) -> Result<(), String>,
    ) -> (Result<(), String>, bool) {
        if let Some(completed) = completed {
            if let Err(error) = finish(completed) {
                return (Err(match result {
                    Ok(()) => format!("Configuração confirmada e cleanup concluído; a conclusão durável da transação falhou: {error}"),
                    Err(original) => format!("{original}; conclusão durável da recuperação falhou: {error}"),
                }), true);
            }
        }
        match (completed, result) {
            (None, result) => (
                Err(format!(
                    "{}; {BLOCKED}: intenção preservada; conclusão não validada.",
                    result
                        .err()
                        .unwrap_or_else(|| "A transação não comprovou sua conclusão".to_string())
                )),
                true,
            ),
            (_, result) => (result, false),
        }
    }

    fn finish_with_sync(
        &self,
        completed: Completed,
        sync: &impl Fn(&File) -> std::io::Result<()>,
    ) -> Result<(), String> {
        self.finish_with_hooks(completed, sync, |path| fs::remove_file(path))
    }

    fn finish_with_hooks(
        &self,
        completed: Completed,
        sync: &impl Fn(&File) -> std::io::Result<()>,
        remove: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<(), String> {
        self.validate_record()?;
        self.validate_queue_cache_proof()?;
        if self.record.intent.queue_backup.is_some() {
            let (file, _) = self
                .queue_cache_proof
                .as_ref()
                .ok_or_else(|| blocked("Conclusão da fila não comprovada."))?;
            let path = self.parent.entry("queue_cache.json");
            sync(file).map_err(|_| blocked("Sincronização final da fila não confirmada."))?;
            require_file_identity(&path, file)?;
        }
        let (config, library) = match completed {
            Completed::Committed => (self.record.intent.next_config, &self.record.intent.next),
            Completed::Recovered => (
                self.record.intent.previous_config,
                &self.record.intent.previous,
            ),
        };
        if self.parent.config_fingerprint()? != config
            || generation(&self.parent.entry("library"))? != *library
        {
            return Err(blocked(
                "Configuração e geração finais não correspondem à intenção.",
            ));
        }
        match fs::symlink_metadata(self.parent.entry("library.next")) {
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            _ => return Err(blocked("Cleanup da staging não comprovado.")),
        }
        let config_file = open_regular(&self.parent.entry("config.json"), true)?;
        config_file
            .sync_all()
            .map_err(|_| blocked("Sincronização da configuração final não confirmada."))?;
        // The caller has already awaited/audited the MPD refresh. Sync its existing
        // local database without parsing, editing or attempting to repair it.
        let catalog_path = self.parent.entry("mpd-v2.db");
        let catalog = open_regular(&catalog_path, false)?;
        catalog
            .sync_all()
            .map_err(|_| blocked("Durabilidade do catálogo MPD não confirmada."))?;
        require_file_identity(&catalog_path, &catalog)?;
        sync_mirror(&self.parent.entry("library"), 0)?;
        // Persist exchange/rollback and cleanup BEFORE removing the stop marker.
        self.parent.sync_with(sync)?;
        self.validate_record()?;
        require_file_identity(&catalog_path, &catalog)?;
        require_file_identity(&self.parent.entry("config.json"), &config_file)?;
        self.validate_queue_cache_proof()?;
        if self.parent.config_fingerprint()? != config
            || generation(&self.parent.entry("library"))? != *library
            || fs::symlink_metadata(self.parent.entry("library.next"))
                .err()
                .is_none_or(|error| error.kind() != ErrorKind::NotFound)
        {
            return Err(blocked("Estado final mudou antes da remoção da intenção."));
        }
        remove(&self.parent.entry(NAME)).map_err(|_| blocked("Remoção da intenção falhou."))?;
        // Failure here is a completion error. Final resources are already durable;
        // a crash may leave the marker present or absent, both safe for this outcome.
        self.parent.sync_with(sync).map_err(|_| {
            "MANAGED_FOLDERS_SAVE_REMOVAL_UNCONFIRMED: Recursos finais sincronizados e cleanup concluído, mas a durabilidade da remoção da intenção não foi confirmada; o registro pode reaparecer após crash.".to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigSaveError;
    use crate::local_media::mirror::{
        self, ManagedGenerationPreparation, ManagedMirrorPublication,
    };
    use crate::{complete_published_mirror_refresh, RestorePreviousOutcome};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        config_dir: PathBuf,
        previous: AppConfig,
        next: AppConfig,
        prepared: MirrorBuildResult,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "sonante-save-intent-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let config_dir = root.join("private");
            fs::create_dir(&config_dir).unwrap();
            fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700)).unwrap();
            let old = root.join("Old");
            let added = root.join("Added");
            for source in [&old, &added] {
                fs::create_dir(source).unwrap();
                fs::write(source.join("track.flac"), b"fixture").unwrap();
            }
            let previous = AppConfig {
                first_run: false,
                local_folders: vec![old.to_string_lossy().into_owned()],
                ..AppConfig::default()
            };
            let mut next = previous.clone();
            next.local_folders
                .push(added.to_string_lossy().into_owned());
            let live = config_dir.join("library");
            let staging = config_dir.join("library.next");
            let ManagedGenerationPreparation::Prepared(initial) =
                mirror::prepare_managed_generation(&live, &staging, &[old.clone()], "previous")
                    .unwrap()
            else {
                panic!("fixture initial generation")
            };
            mirror::publish_prepared_managed_generation(&live, &initial).unwrap();
            let ManagedGenerationPreparation::Prepared(prepared) =
                mirror::prepare_managed_generation(&live, &staging, &[old, added], "next").unwrap()
            else {
                panic!("fixture next generation")
            };
            let fixture = Self {
                root,
                config_dir,
                previous,
                next,
                prepared,
            };
            fixture.persist(&fixture.previous).unwrap();
            fs::write(fixture.config_dir.join("mpd-v2.db"), b"fixture catalog").unwrap();
            fixture
        }

        fn persist(&self, config: &AppConfig) -> Result<(), ConfigSaveError> {
            crate::persistence::atomic_write_private_synced(
                &self.config_dir.join("config.json"),
                &serde_json::to_vec(config).unwrap(),
                "config.json",
            )
        }

        fn intent(&self) -> SaveIntent {
            SaveIntent::create(&self.config_dir, &self.previous, &self.next, &self.prepared)
                .unwrap()
        }

        fn publish(&self, intent: &SaveIntent) -> ManagedMirrorPublication {
            intent.publish(&self.prepared, None).unwrap()
        }

        fn assert_startup_blocked(&self) {
            let config = fs::read(self.config_dir.join("config.json")).unwrap();
            let live = generation(&self.config_dir.join("library")).unwrap();
            let staged = fs::symlink_metadata(self.config_dir.join("library.next"))
                .ok()
                .map(|_| generation(&self.config_dir.join("library.next")).unwrap());
            let result: Result<(), String> = guard_startup(&self.config_dir, || {
                panic!("blocked startup must not load config, touch MPD or reconcile")
            });
            assert!(result.unwrap_err().starts_with(BLOCKED));
            assert_eq!(
                fs::read(self.config_dir.join("config.json")).unwrap(),
                config
            );
            assert_eq!(generation(&self.config_dir.join("library")).unwrap(), live);
            assert_eq!(
                fs::symlink_metadata(self.config_dir.join("library.next"))
                    .ok()
                    .map(|_| generation(&self.config_dir.join("library.next")).unwrap()),
                staged
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn backup_inputs(
        state: PlaybackState,
    ) -> (Vec<TrackMetadata>, MpdQueueStructure, MpdPlaybackSnapshot) {
        // Duplicate CUE occurrences have different MPD IDs, not different URIs.
        let uri = "Old/Álbum com espaços/disco.CUE/track0001";
        let tracks = (0..2)
            .map(|_| TrackMetadata {
                title: "Canção".into(),
                artist: "Artista".into(),
                album: "Álbum".into(),
                uri: uri.into(),
                media_locator: Some(MediaLocator::Local { uri: uri.into() }),
                duration: Some(123.0),
                thumb: Some("data:image/png;base64,ARTWORK".into()),
                plex_image: None,
            })
            .collect::<Vec<_>>();
        let queue = MpdQueueStructure {
            entries: (0..2)
                .map(|pos| crate::audio::MpdQueueEntry {
                    uri: uri.into(),
                    pos,
                    id: pos as u64 + 10,
                })
                .collect(),
            playlist_version: 42,
        };
        let playback = MpdPlaybackSnapshot {
            uris: vec![uri.into(); 2],
            current_index: Some(1),
            elapsed: 16.25,
            state,
            playlist_version: 42,
        };
        (tracks, queue, playback)
    }

    fn backup_fixture(state: PlaybackState) -> QueueBackup {
        let (tracks, queue, playback) = backup_inputs(state);
        QueueBackup::new(&tracks, &playback.uris, &queue, &playback).unwrap()
    }

    fn create_backup(fixture: &Fixture, backup: &QueueBackup) -> Result<SaveIntent, String> {
        SaveIntent::create_with_queue_backup(
            &fixture.config_dir,
            &fixture.previous,
            &fixture.next,
            &fixture.prepared,
            backup,
        )
    }

    fn logical_backup_fixture(state: PlaybackState) -> QueueBackup {
        let (tracks, queue, mut playback) = backup_inputs(PlaybackState::Stopped);
        playback.current_index = None;
        playback.elapsed = 0.0;
        QueueBackup::new_with_logical_resume(
            &tracks,
            &playback.uris,
            &queue,
            &playback,
            BackupLogicalResume {
                queue_index: 1,
                elapsed: 16.25,
                state,
            },
        )
        .unwrap()
    }

    #[test]
    fn readonly_recovery_diagnoses_valid_invalid_orphan_and_versions_without_cleanup() {
        use crate::library_recovery::{ConfigMatch, Reason};
        for kind in ["orphan", "v1", "v2", "invalid", "different_config", "next_config"] {
            let fixture = Fixture::new();
            let _intent = match kind {
                "orphan" => None,
                "v2" => Some(create_backup(&fixture, &logical_backup_fixture(PlaybackState::Paused)).unwrap()),
                _ => Some(fixture.intent()),
            };
            if kind == "invalid" { fs::write(fixture.config_dir.join(NAME), b"truncated").unwrap(); }
            if kind == "different_config" {
                let mut changed = fixture.previous.clone(); changed.first_run = true; fixture.persist(&changed).unwrap();
            }
            if kind == "next_config" { fixture.persist(&fixture.next).unwrap(); }
            let config = fs::read(fixture.config_dir.join("config.json")).unwrap();
            let intent = fs::read(fixture.config_dir.join(NAME)).ok();
            let old = generation(&fixture.config_dir.join("library")).unwrap();
            let next = generation(&fixture.config_dir.join("library.next")).unwrap();
            let observed = inspect_recovery(&fixture.config_dir, 7, &|| false).unwrap();
            assert!(!observed.recovery_available);
            assert_eq!(observed.analysis_id, 7);
            assert!(observed.reasons.contains(&Reason::DatabaseUnproven));
            assert!(observed.reasons.contains(&Reason::OwnershipUnknown));
            match kind {
                "orphan" => assert!(observed.reasons.contains(&Reason::OrphanStaging)),
                "invalid" => assert!(observed.reasons.contains(&Reason::IntentInvalid)),
                "different_config" => assert!(observed.reasons.contains(&Reason::ConfigDivergent)),
                "next_config" => assert_eq!(observed.config_matches, ConfigMatch::Next),
                "v2" => { assert_eq!(observed.intent_version, Some(2)); assert_eq!(observed.queue_backup_version, Some(2)); }
                _ => { assert_eq!(observed.intent_version, Some(1)); assert_eq!(observed.config_matches, ConfigMatch::Previous); }
            }
            assert_eq!(fs::read(fixture.config_dir.join("config.json")).unwrap(), config);
            assert_eq!(fs::read(fixture.config_dir.join(NAME)).ok(), intent);
            assert_eq!(generation(&fixture.config_dir.join("library")).unwrap(), old);
            assert_eq!(generation(&fixture.config_dir.join("library.next")).unwrap(), next);
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn readonly_recovery_rejects_replaced_generations_and_never_follows_control_symlinks() {
        use crate::library_recovery::Reason;
        for kind in ["generation", "db_symlink", "intent_symlink", "parent_mode", "config_missing"] {
            let fixture = Fixture::new(); let _intent = fixture.intent();
            match kind {
                "generation" => {
                    fs::rename(fixture.config_dir.join("library"), fixture.config_dir.join("old-library")).unwrap();
                    fs::create_dir(fixture.config_dir.join("library")).unwrap();
                }
                "db_symlink" => {
                    fs::remove_file(fixture.config_dir.join("mpd-v2.db")).unwrap();
                    std::os::unix::fs::symlink("/synthetic-missing-db-target", fixture.config_dir.join("mpd-v2.db")).unwrap();
                }
                "intent_symlink" => {
                    fs::remove_file(fixture.config_dir.join(NAME)).unwrap();
                    std::os::unix::fs::symlink("/synthetic-missing-intent-target", fixture.config_dir.join(NAME)).unwrap();
                }
                "config_missing" => { fs::remove_file(fixture.config_dir.join("config.json")).unwrap(); }
                _ => fs::set_permissions(&fixture.config_dir, fs::Permissions::from_mode(0o777)).unwrap(),
            }
            let observed = inspect_recovery(&fixture.config_dir, 8, &|| false).unwrap();
            let expected = match kind { "generation" => Reason::GenerationDivergent,
                "config_missing" => Reason::ConfigMissing, _ => Reason::UnsafeEntry };
            assert!(observed.reasons.contains(&expected), "{kind}: {:?}", observed.reasons);
            assert!(!observed.recovery_available);
        }
    }

    #[test]
    fn readonly_recovery_reports_mid_inspection_changes_and_cancels_without_effects() {
        use crate::library_recovery::Reason;
        let fixture = Fixture::new(); let _intent = fixture.intent();
        let calls = Cell::new(0);
        let result = inspect_recovery(&fixture.config_dir, 9, &|| {
            calls.set(calls.get() + 1);
            if calls.get() == 4 { fixture.persist(&fixture.next).unwrap(); }
            false
        }).unwrap();
        assert!(result.reasons.contains(&Reason::FilesystemChanged));
        let intent = fs::read(fixture.config_dir.join(NAME)).unwrap();
        let calls = Cell::new(0);
        assert_eq!(inspect_recovery(&fixture.config_dir, 10, &|| {
            calls.set(calls.get() + 1); calls.get() >= 3
        }).unwrap_err(), crate::library_recovery::CANCELLED);
        assert_eq!(fs::read(fixture.config_dir.join(NAME)).unwrap(), intent);
        fixture.assert_startup_blocked();
    }

    #[test]
    fn readonly_recovery_missing_media_and_legacy_permissions_are_preserved() {
        use crate::library_recovery::Reason;
        let fixture = Fixture::new(); let _intent = fixture.intent();
        // Both source roots and media are absent. Analysis only reads manifests.
        fs::remove_dir_all(fixture.root.join("Old")).unwrap();
        fs::remove_dir_all(fixture.root.join("Added")).unwrap();
        fs::set_permissions(&fixture.config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        let observed = inspect_recovery(&fixture.config_dir, 11, &|| false).unwrap();
        assert!(observed.reasons.contains(&Reason::IntentValid));
        assert!(!observed.reasons.contains(&Reason::FilesystemChanged));
        assert_eq!(fs::metadata(&fixture.config_dir).unwrap().mode() & 0o777, 0o755);
        assert!(!fixture.root.join("Old").exists());
    }

    #[test]
    fn queue_backup_v1_payload_retains_exact_serialization_and_checksum() {
        #[derive(Serialize)]
        struct LegacyBackup<'a> {
            version: u32,
            tracks: &'a [BackupTrack],
            mpd: &'a [BackupMpdReference],
            playlist_version: u64,
            current_index: Option<usize>,
            state: PlaybackState,
            elapsed: f64,
        }
        for state in [
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Stopped,
        ] {
            let backup = backup_fixture(state);
            let legacy = LegacyBackup {
                version: 1,
                tracks: &backup.tracks,
                mpd: &backup.mpd,
                playlist_version: backup.playlist_version,
                current_index: backup.current_index,
                state: backup.state,
                elapsed: backup.elapsed,
            };
            let bytes = serde_json::to_vec(&legacy).unwrap();
            assert_eq!(bytes, serde_json::to_vec(&backup).unwrap());
            assert_eq!(fingerprint(&legacy).unwrap(), fingerprint(&backup).unwrap());
            let decoded: QueueBackup = serde_json::from_slice(&bytes).unwrap();
            decoded.validate().unwrap();
            assert!(decoded == backup);
            assert!(decoded.logical_resume.is_none());
        }
    }

    #[test]
    fn queue_backup_v2_roundtrip_keeps_logical_resume_separate_from_neutral_mpd() {
        for state in [PlaybackState::Playing, PlaybackState::Paused] {
            let fixture = Fixture::new();
            let backup = logical_backup_fixture(state);
            let intent = create_backup(&fixture, &backup).unwrap();
            assert_eq!(intent.record.intent.version, 2);
            assert_eq!(backup.version, 2);
            intent.before_exchange().unwrap();
            drop(intent);
            let read = SaveIntent::read_queue_backup(&fixture.config_dir)
                .unwrap()
                .unwrap();
            assert!(read == backup);
            assert_eq!(read.state, PlaybackState::Stopped);
            assert_eq!(read.current_index, None);
            assert_eq!(read.elapsed, 0.0);
            let resume = read.logical_resume.unwrap();
            assert_eq!(resume.queue_index, 1);
            assert_eq!(resume.elapsed, 16.25);
            assert_eq!(resume.state, state);
            let bytes = fs::read(fixture.config_dir.join(NAME)).unwrap();
            assert!(!String::from_utf8(bytes).unwrap().contains("ARTWORK"));
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn queue_backup_logical_resume_rejects_invalid_cursors_contradictions_and_identity() {
        for fault in 0..12 {
            let (mut tracks, mut queue, mut playback) = backup_inputs(PlaybackState::Stopped);
            playback.current_index = None;
            playback.elapsed = 0.0;
            let mut resume = BackupLogicalResume {
                queue_index: 1,
                elapsed: 16.25,
                state: PlaybackState::Paused,
            };
            match fault {
                0 => resume.queue_index = tracks.len(),
                1 => resume.elapsed = -1.0,
                2 => resume.elapsed = f64::NAN,
                3 => resume.elapsed = f64::INFINITY,
                4 => resume.state = PlaybackState::Stopped,
                5 => playback.state = PlaybackState::Paused,
                6 => playback.state = PlaybackState::Playing,
                7 => playback.current_index = Some(1),
                8 => playback.elapsed = 1.0,
                9 => queue.playlist_version += 1,
                10 => queue.entries[1].id = queue.entries[0].id,
                _ => {
                    tracks.clear();
                    queue.entries.clear();
                    playback.uris.clear();
                }
            }
            assert!(QueueBackup::new_with_logical_resume(
                &tracks,
                &playback.uris,
                &queue,
                &playback,
                resume,
            )
            .is_err());
        }
    }

    #[test]
    fn queue_backup_v2_invalid_or_incomplete_journal_blocks_reads_and_startup() {
        for fault in 0..10 {
            let fixture = Fixture::new();
            create_backup(&fixture, &logical_backup_fixture(PlaybackState::Paused)).unwrap();
            let path = fixture.config_dir.join(NAME);
            let parent = Parent::inspect(&fixture.config_dir).unwrap();
            let mut record = read_record(&parent).unwrap();
            let backup = record.intent.queue_backup.as_mut().unwrap();
            match fault {
                0 => backup.logical_resume.as_mut().unwrap().elapsed += 1.0,
                1 => backup.logical_resume.as_mut().unwrap().queue_index = 20,
                2 => backup.logical_resume = None,
                3 => backup.version = 1,
                4 => backup.current_index = Some(0),
                5 => backup.logical_resume.as_mut().unwrap().state = PlaybackState::Stopped,
                _ => {}
            }
            // Invalid semantics must fail even with a matching checksum.
            if fault != 0 {
                record.checksum = fingerprint(&record.intent).unwrap();
            }
            let mut value = serde_json::to_value(&record).unwrap();
            match fault {
                6 => {
                    value["intent"]["queue_backup"]
                        .as_object_mut()
                        .unwrap()
                        .remove("logical_resume");
                }
                7 => {
                    value["intent"]["queue_backup"]["logical_resume"]
                        .as_object_mut()
                        .unwrap()
                        .remove("elapsed");
                }
                8 => {
                    value["intent"]["queue_backup"]["logical_resume"]["unknown"] =
                        serde_json::json!(true)
                }
                _ => {}
            }
            let mut bytes = serde_json::to_vec(&value).unwrap();
            if fault == 9 {
                bytes.truncate(bytes.len() / 2);
            }
            fs::write(&path, bytes).unwrap();
            assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
            fixture.assert_startup_blocked();
            assert!(path.exists());
        }
    }

    #[test]
    fn queue_backup_v2_write_and_fsync_failure_preserve_stop_marker() {
        for fault in 0..3 {
            let fixture = Fixture::new();
            let backup = logical_backup_fixture(PlaybackState::Paused);
            let syncs = Cell::new(0);
            let result = SaveIntent::create_record_with_io(
                &fixture.config_dir,
                &fixture.previous,
                &fixture.next,
                &fixture.prepared,
                Some(&backup),
                &|file| {
                    syncs.set(syncs.get() + 1);
                    if syncs.get() == fault {
                        Err(std::io::Error::other("simulated fsync"))
                    } else {
                        file.sync_all()
                    }
                },
                &|file, bytes| {
                    if fault == 0 {
                        file.write_all(&bytes[..bytes.len() / 2])?;
                        Err(std::io::Error::other("simulated partial write"))
                    } else {
                        file.write_all(bytes)
                    }
                },
            );
            assert!(result.is_err());
            assert!(fixture.config_dir.join(NAME).exists());
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn queue_backup_legacy_v1_keeps_original_checksum_and_blocks_startup() {
        #[derive(Serialize)]
        struct LegacyIntent<'a> {
            version: u32,
            previous_config: [u8; 32],
            next_config: [u8; 32],
            previous: &'a Generation,
            next: &'a Generation,
        }
        let fixture = Fixture::new();
        let intent = fixture.intent();
        let old = &intent.record.intent;
        let legacy = LegacyIntent {
            version: 1,
            previous_config: old.previous_config,
            next_config: old.next_config,
            previous: &old.previous,
            next: &old.next,
        };
        assert_eq!(fingerprint(&legacy).unwrap(), intent.record.checksum);
        assert_eq!(
            serde_json::to_vec(&legacy).unwrap(),
            serde_json::to_vec(old).unwrap()
        );
        assert!(SaveIntent::read_queue_backup(&fixture.config_dir)
            .unwrap()
            .is_none());
        fixture.assert_startup_blocked();
    }

    #[test]
    fn queue_backup_roundtrip_preserves_occurrences_cursor_and_state_without_artwork() {
        for state in [
            PlaybackState::Playing,
            PlaybackState::Paused,
            PlaybackState::Stopped,
        ] {
            let fixture = Fixture::new();
            let backup = backup_fixture(state);
            let cache = fixture.config_dir.join("queue_cache.json");
            fs::write(&cache, b"existing queue cache").unwrap();
            let physical = fixture.root.join("Old/track.flac");
            let physical_bytes = fs::read(&physical).unwrap();
            let before = generation(&fixture.config_dir.join("library")).unwrap();
            let staged = generation(&fixture.config_dir.join("library.next")).unwrap();
            let intent = create_backup(&fixture, &backup).unwrap();
            assert_eq!(intent.record.intent.version, 2);
            intent.before_exchange().unwrap();
            drop(intent); // Consult the actual disk record, not an in-memory copy.
            let read = SaveIntent::read_queue_backup(&fixture.config_dir)
                .unwrap()
                .unwrap();
            assert!(read == backup);
            let bytes = fs::read(fixture.config_dir.join(NAME)).unwrap();
            assert!(!String::from_utf8(bytes).unwrap().contains("ARTWORK"));
            let metadata = fs::metadata(fixture.config_dir.join(NAME)).unwrap();
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 1);
            assert_eq!(fs::read(&cache).unwrap(), b"existing queue cache");
            assert_eq!(fs::read(&physical).unwrap(), physical_bytes);
            assert_eq!(
                generation(&fixture.config_dir.join("library")).unwrap(),
                before
            );
            assert_eq!(
                generation(&fixture.config_dir.join("library.next")).unwrap(),
                staged
            );
            fixture.assert_startup_blocked();
        }
        let empty = QueueBackup::new(
            &[],
            &[],
            &MpdQueueStructure {
                entries: vec![],
                playlist_version: 0,
            },
            &MpdPlaybackSnapshot {
                uris: vec![],
                current_index: None,
                elapsed: 0.0,
                state: PlaybackState::Stopped,
                playlist_version: 0,
            },
        )
        .unwrap();
        let fixture = Fixture::new();
        create_backup(&fixture, &empty).unwrap();
        assert!(
            SaveIntent::read_queue_backup(&fixture.config_dir)
                .unwrap()
                .unwrap()
                == empty
        );
    }

    #[test]
    fn queue_backup_plex_saves_stable_identity_never_authenticated_runtime_urls() {
        let (mut tracks, mut queue, mut playback) = backup_inputs(PlaybackState::Playing);
        let runtime = "https://user:password@fixture.invalid/part?X-Plex-Token=PRIVATE_TOKEN";
        for track in &mut tracks {
            track.uri = runtime.into(); // Even an old runtime value must not be copied.
            track.thumb = Some(runtime.into());
            track.media_locator = Some(MediaLocator::Plex {
                server_id: "server-1".into(),
                part_key: "/library/parts/42/file.flac".into(),
                rating_key: Some("42".into()),
                file_path: None,
            });
        }
        for entry in &mut queue.entries {
            entry.uri = runtime.into();
        }
        playback.uris = vec![runtime.into(); 2];
        let backup = QueueBackup::new(&tracks, &playback.uris, &queue, &playback).unwrap();
        let fixture = Fixture::new();
        create_backup(&fixture, &backup).unwrap();
        let saved = String::from_utf8(fs::read(fixture.config_dir.join(NAME)).unwrap()).unwrap();
        for secret in [
            runtime,
            "PRIVATE_TOKEN",
            "password",
            "https://",
            "thumb",
            "plex_image",
        ] {
            assert!(!saved.contains(secret));
        }
        assert!(
            SaveIntent::read_queue_backup(&fixture.config_dir)
                .unwrap()
                .unwrap()
                == backup
        );
        fixture.assert_startup_blocked();
    }

    #[test]
    fn queue_backup_rejects_inconsistent_captures_and_unsafe_persisted_fields() {
        for mutation in 0..13 {
            let (mut tracks, mut queue, mut playback) = backup_inputs(PlaybackState::Playing);
            match mutation {
                0 => {
                    tracks.pop();
                }
                1 => queue.entries[0].pos = 1,
                2 => queue.entries[1].id = queue.entries[0].id,
                3 => queue.playlist_version += 1,
                4 => queue.entries[0].uri = "Other/track.flac".into(),
                5 => playback.current_index = Some(2),
                6 => playback.current_index = None,
                7 => playback.elapsed = f64::NAN,
                8 => playback.elapsed = -1.0,
                9 => tracks[0].duration = Some(f64::INFINITY),
                10 => {
                    tracks[0].title = "https://user:password@fixture.invalid/?token=SECRET".into()
                }
                11 => {
                    tracks[0].media_locator = Some(MediaLocator::Local {
                        uri: "Other/track.flac".into(),
                    })
                }
                _ => {
                    tracks[0].media_locator = Some(MediaLocator::Plex {
                        server_id: "server-1".into(),
                        part_key: "/library/parts/42?X-Plex-Token=SECRET".into(),
                        rating_key: None,
                        file_path: None,
                    })
                }
            }
            let error = QueueBackup::new(&tracks, &playback.uris, &queue, &playback)
                .err()
                .unwrap();
            assert!(error.starts_with(BLOCKED));
            assert!(!error.contains("SECRET"));
        }
    }

    #[test]
    fn queue_backup_tampered_truncated_missing_or_invalid_payload_blocks_consultation_and_startup()
    {
        for mutation in 0..10 {
            let fixture = Fixture::new();
            create_backup(&fixture, &backup_fixture(PlaybackState::Paused)).unwrap();
            let path = fixture.config_dir.join(NAME);
            if mutation == 0 {
                let mut bytes = fs::read(&path).unwrap();
                bytes.truncate(bytes.len() / 2);
                fs::write(&path, bytes).unwrap();
            } else {
                let parent = Parent::inspect(&fixture.config_dir).unwrap();
                let mut record = read_record(&parent).unwrap();
                match mutation {
                    1 => record.intent.queue_backup.as_mut().unwrap().elapsed += 1.0, // stale checksum
                    2 => record.intent.queue_backup = None,
                    3 => record.intent.queue_backup.as_mut().unwrap().version = 9,
                    4 => record.intent.queue_backup.as_mut().unwrap().mpd[0].track_index = 1,
                    5 => record.intent.queue_backup.as_mut().unwrap().current_index = Some(20),
                    6 => record.intent.version = 1, // v1 cannot carry a backup
                    7 => record.intent.version = 99,
                    8 => record.intent.previous_config[0] ^= 1,
                    _ => record.intent.next_config[0] ^= 1,
                }
                if !matches!(mutation, 1 | 8 | 9) {
                    record.checksum = fingerprint(&record.intent).unwrap();
                }
                fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
            assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
            fixture.assert_startup_blocked();
            assert!(path.exists());
        }
    }

    #[test]
    fn queue_backup_unknown_or_incomplete_schema_is_not_silently_dropped() {
        for field in ["tracks", "mpd", "state", "version", "artwork"] {
            let fixture = Fixture::new();
            create_backup(&fixture, &backup_fixture(PlaybackState::Stopped)).unwrap();
            let path = fixture.config_dir.join(NAME);
            let mut record: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            let backup = record["intent"]["queue_backup"].as_object_mut().unwrap();
            if field == "artwork" {
                backup.insert(
                    field.into(),
                    serde_json::json!("data:image/png;base64,UNKNOWN"),
                );
            } else {
                backup.remove(field);
            }
            fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn queue_backup_size_and_validation_fail_before_creating_intent() {
        for oversized in [false, true] {
            let fixture = Fixture::new();
            let mut backup = backup_fixture(PlaybackState::Playing);
            if oversized {
                backup.tracks[0].title = "x".repeat(MAX_RECORD_BYTES as usize);
            } else {
                backup.mpd[0].pos = 8;
            }
            assert!(create_backup(&fixture, &backup).is_err());
            assert!(!fixture.config_dir.join(NAME).exists());
            fixture.assert_startup_blocked(); // Existing staging still cannot be treated as recovered.
        }
        let fixture = Fixture::new();
        create_backup(&fixture, &backup_fixture(PlaybackState::Playing)).unwrap();
        fs::write(
            fixture.config_dir.join(NAME),
            vec![b' '; MAX_RECORD_BYTES as usize + 1],
        )
        .unwrap();
        assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
        fixture.assert_startup_blocked();
    }

    #[test]
    fn queue_backup_failed_file_or_directory_fsync_never_returns_permission_to_clear() {
        for fail_at in [0, 1, 2] {
            let fixture = Fixture::new();
            let before = fs::read(fixture.config_dir.join("config.json")).unwrap();
            let backup = backup_fixture(PlaybackState::Playing);
            let calls = Cell::new(0);
            let result = SaveIntent::create_record(
                &fixture.config_dir,
                &fixture.previous,
                &fixture.next,
                &fixture.prepared,
                Some(&backup),
                &|file: &File| {
                    calls.set(calls.get() + 1);
                    assert_eq!(file.metadata().unwrap().is_dir(), calls.get() == 2);
                    if calls.get() == fail_at {
                        Err(std::io::Error::new(ErrorKind::Other, "simulated fsync"))
                    } else {
                        file.sync_all()
                    }
                },
            );
            assert_eq!(result.is_err(), fail_at != 0);
            assert_eq!(calls.get(), if fail_at == 1 { 1 } else { 2 });
            assert!(fixture.config_dir.join(NAME).exists());
            assert_eq!(
                fs::read(fixture.config_dir.join("config.json")).unwrap(),
                before
            );
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn queue_backup_partial_write_preserves_incomplete_stop_marker_without_publication() {
        let fixture = Fixture::new();
        let config = fs::read(fixture.config_dir.join("config.json")).unwrap();
        let live = generation(&fixture.config_dir.join("library")).unwrap();
        let staged = generation(&fixture.config_dir.join("library.next")).unwrap();
        let backup = backup_fixture(PlaybackState::Playing);
        let result = SaveIntent::create_record_with_io(
            &fixture.config_dir,
            &fixture.previous,
            &fixture.next,
            &fixture.prepared,
            Some(&backup),
            &|_| panic!("partial write must not reach fsync"),
            &|file, bytes| {
                file.write_all(&bytes[..bytes.len() / 2])?;
                Err(std::io::Error::new(
                    ErrorKind::WriteZero,
                    "simulated partial write",
                ))
            },
        );
        assert!(result.is_err());
        assert!(fixture.config_dir.join(NAME).exists());
        assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
        assert_eq!(
            fs::read(fixture.config_dir.join("config.json")).unwrap(),
            config
        );
        assert_eq!(
            generation(&fixture.config_dir.join("library")).unwrap(),
            live
        );
        assert_eq!(
            generation(&fixture.config_dir.join("library.next")).unwrap(),
            staged
        );
        fixture.assert_startup_blocked();
    }

    #[test]
    fn queue_backup_completion_requires_durable_cache_and_rechecks_concurrent_mutation() {
        for fault in [
            "missing",
            "modified",
            "replaced",
            "during_sync",
            "sync_failure",
            "none",
        ] {
            let fixture = Fixture::new();
            let mut intent =
                create_backup(&fixture, &backup_fixture(PlaybackState::Paused)).unwrap();
            let publication = fixture.publish(&intent);
            fixture.persist(&fixture.next).unwrap();
            publication.cleanup_previous().unwrap();
            let cache = fixture.config_dir.join("queue_cache.json");
            if fault != "missing" {
                intent
                    .persist_queue_cache(br#"{"version":2,"tracks":[]}"#)
                    .unwrap();
            }
            match fault {
                "modified" => fs::write(&cache, b"modified").unwrap(),
                "replaced" => {
                    let bytes = fs::read(&cache).unwrap();
                    fs::rename(&cache, fixture.config_dir.join("prior-cache")).unwrap();
                    fs::write(&cache, bytes).unwrap();
                    fs::set_permissions(&cache, fs::Permissions::from_mode(0o600)).unwrap();
                }
                _ => {}
            }
            let result = intent.finish_with_hooks(
                Completed::Committed,
                &|file| {
                    if fault == "sync_failure" {
                        return Err(std::io::Error::new(
                            ErrorKind::Other,
                            "simulated cache fsync",
                        ));
                    }
                    if fault == "during_sync" && file.metadata()?.is_dir() {
                        fs::write(&cache, b"modified while finishing")?;
                    }
                    file.sync_all()
                },
                |path| {
                    assert_eq!(fault, "none");
                    fs::remove_file(path)
                },
            );
            assert_eq!(result.is_err(), fault != "none");
            if fault != "none" {
                fixture.assert_startup_blocked();
            } else {
                assert!(!fixture.config_dir.join(NAME).exists());
            }
        }
    }

    #[test]
    fn queue_backup_cache_persistence_errors_never_establish_completion_proof() {
        for after_rename in [false, true] {
            let fixture = Fixture::new();
            let mut intent =
                create_backup(&fixture, &backup_fixture(PlaybackState::Playing)).unwrap();
            let result = intent.persist_queue_cache_with(
                br#"{"version":2,"tracks":[]}"#,
                |path, parent, bytes| {
                    if after_rename {
                        crate::persistence::atomic_write_private_synced_in(
                            path,
                            parent,
                            bytes,
                            "queue_cache.json",
                        )?;
                        Err(ConfigSaveError::AfterRenameUnconfirmed(
                            "simulated directory fsync failure".into(),
                        ))
                    } else {
                        Err(ConfigSaveError::BeforeRename(
                            "simulated cache write failure".into(),
                        ))
                    }
                },
            );
            assert!(result.is_err());
            assert!(intent.queue_cache_proof.is_none());
            fixture.assert_startup_blocked();
        }
        let fixture = Fixture::new();
        let mut intent = fixture.intent();
        assert!(intent
            .persist_queue_cache(b"legacy must not authorize queue writes")
            .is_err());
        assert!(!fixture.config_dir.join("queue_cache.json").exists());
    }

    #[test]
    fn queue_backup_cache_parent_substitution_never_writes_replacement_directory() {
        let fixture = Fixture::new();
        let mut intent = create_backup(&fixture, &backup_fixture(PlaybackState::Paused)).unwrap();
        let original = fixture.config_dir.with_file_name("held-original-config");
        let result =
            intent.persist_queue_cache_with(b"must remain confined", |path, parent, bytes| {
                fs::rename(&fixture.config_dir, &original).unwrap();
                fs::create_dir(&fixture.config_dir).unwrap();
                fs::set_permissions(&fixture.config_dir, fs::Permissions::from_mode(0o700))
                    .unwrap();
                crate::persistence::atomic_write_private_synced_in(
                    path,
                    parent,
                    bytes,
                    "queue_cache.json",
                )
            });
        assert!(result.is_err());
        assert!(!fixture.config_dir.join("queue_cache.json").exists());
        assert!(!original.join("queue_cache.json").exists());
        assert!(original.join(NAME).exists());
        assert!(intent.queue_cache_proof.is_none());
        fs::remove_dir(&fixture.config_dir).unwrap();
        fs::rename(&original, &fixture.config_dir).unwrap();
        fixture.assert_startup_blocked();
    }

    #[test]
    fn queue_backup_exclusive_creation_and_unsafe_file_identity_fail_closed() {
        for mutation in 0..5 {
            let fixture = Fixture::new();
            let backup = backup_fixture(PlaybackState::Playing);
            let intent = create_backup(&fixture, &backup).unwrap();
            let path = fixture.config_dir.join(NAME);
            let bytes = fs::read(&path).unwrap();
            assert!(create_backup(&fixture, &backup).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            match mutation {
                0 => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
                1 => {
                    fs::rename(&path, fixture.config_dir.join("original")).unwrap();
                    std::os::unix::fs::symlink("original", &path).unwrap();
                }
                2 => fs::hard_link(&path, fixture.config_dir.join("extra-link")).unwrap(),
                3 => {
                    fs::rename(&path, fixture.config_dir.join("original")).unwrap();
                    fs::write(&path, &bytes).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    // A valid copy must still not authorize this owning handle to publish.
                    assert!(intent.before_exchange().is_err());
                    fixture.assert_startup_blocked();
                    continue;
                }
                _ => {
                    let moved = fixture.root.join("moved");
                    fs::rename(&fixture.config_dir, &moved).unwrap();
                    fs::create_dir(&fixture.config_dir).unwrap();
                    fs::set_permissions(&fixture.config_dir, fs::Permissions::from_mode(0o700))
                        .unwrap();
                    assert!(intent.before_exchange().is_err());
                    assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
                    continue;
                }
            }
            assert!(intent.before_exchange().is_err());
            assert!(SaveIntent::read_queue_backup(&fixture.config_dir).is_err());
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn normal_save_commits_cleans_and_durably_removes_intent() {
        let fixture = Fixture::new();
        let intent = fixture.intent();
        assert_eq!(
            fs::metadata(fixture.config_dir.join(NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let publication = fixture.publish(&intent);
        let completed = Cell::new(None);
        let result = complete_published_mirror_refresh(
            || Ok(()),
            || -> Result<(), String> { panic!("capture") },
            || panic!("stop"),
            || panic!("rollback"),
            |_| panic!("restore"),
            || panic!("rejected cleanup"),
            || fixture.persist(&fixture.next),
            || {
                publication.cleanup_previous()?;
                completed.set(Some(Completed::Committed));
                Ok(())
            },
        );
        intent
            .complete_transaction(result, completed.get())
            .unwrap();
        assert!(!fixture.config_dir.join(NAME).exists());
        assert!(!fixture.config_dir.join("library.next").exists());
        let called = Cell::new(false);
        guard_startup(&fixture.config_dir, || {
            called.set(true);
            Ok(())
        })
        .unwrap();
        assert!(called.get());
    }

    #[test]
    fn interrupted_save_at_every_phase_blocks_startup_without_mutation() {
        for phase in [
            "before_exchange",
            "after_exchange",
            "refresh",
            "persist",
            "cleanup",
            "after_cleanup",
        ] {
            let fixture = Fixture::new();
            let intent = fixture.intent();
            if phase != "before_exchange" {
                let publication = fixture.publish(&intent);
                if matches!(phase, "cleanup" | "after_cleanup") {
                    fixture.persist(&fixture.next).unwrap();
                }
                if phase == "persist" {
                    // A config rename happened, but its confirmation never returned.
                    fixture.persist(&fixture.next).unwrap();
                }
                if phase == "after_cleanup" {
                    publication.cleanup_previous().unwrap();
                }
                if phase == "cleanup" {
                    // Simulate interruption after one owned entry was unlinked,
                    // while the previous manifest and generation still exist.
                    let previous =
                        mirror::read_manifest(&fixture.config_dir.join("library.next")).unwrap();
                    let source = previous.sources.keys().next().unwrap();
                    fs::remove_file(
                        fixture
                            .config_dir
                            .join("library.next")
                            .join(source)
                            .join("track.flac"),
                    )
                    .unwrap();
                }
            }
            // No Drop cleanup is attached to SaveIntent: process interruption must
            // leave evidence regardless of which in-memory objects were lost.
            drop(intent);
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn startup_preserves_incomplete_corrupt_and_unsafe_records() {
        for corruption in [
            "truncated",
            "checksum",
            "unknown_field",
            "symlink",
            "hardlink",
            "mode",
        ] {
            let fixture = Fixture::new();
            let intent = fixture.intent();
            let path = fixture.config_dir.join(NAME);
            match corruption {
                "truncated" => fs::write(&path, b"{\"intent\":").unwrap(),
                "checksum" => {
                    let mut record: Record =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    record.checksum[0] ^= 1;
                    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
                }
                "unknown_field" => {
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    value["extra"] = true.into();
                    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "symlink" => {
                    fs::rename(&path, fixture.config_dir.join("unrelated")).unwrap();
                    std::os::unix::fs::symlink("unrelated", &path).unwrap();
                }
                "hardlink" => fs::hard_link(&path, fixture.config_dir.join("other-link")).unwrap(),
                "mode" => fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(),
                _ => unreachable!(),
            }
            drop(intent);
            let before = fs::symlink_metadata(&path).unwrap();
            fixture.assert_startup_blocked();
            let after = fs::symlink_metadata(&path).unwrap();
            assert_eq!(before.ino(), after.ino());
            assert_eq!(before.mode(), after.mode());
        }
    }

    #[test]
    fn absent_intent_with_unknown_staging_blocks_startup_and_preserves_staging() {
        let fixture = Fixture::new();
        fixture.assert_startup_blocked();
        assert!(!fixture.config_dir.join(NAME).exists());
    }

    #[test]
    fn startup_inspection_is_read_only_and_preserves_legacy_directory_permissions() {
        let fixture = Fixture::new();
        mirror::cleanup_prepared_managed_generation(&fixture.prepared).unwrap();
        fs::set_permissions(&fixture.config_dir, fs::Permissions::from_mode(0o755)).unwrap();
        check_startup(&fixture.config_dir).unwrap();
        assert_eq!(
            fs::metadata(&fixture.config_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        fs::write(fixture.config_dir.join(NAME), b"incomplete").unwrap();
        fixture.assert_startup_blocked();
        assert_eq!(
            fs::metadata(&fixture.config_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[test]
    fn targeted_added_sources_publication_retains_the_intent_until_cleanup() {
        let fixture = Fixture::new();
        let (plan, _) = mirror::plan_added_sources_refresh_diagnostic(
            &fixture.config_dir.join("library"),
            &fixture.prepared,
        )
        .unwrap();
        let plan = plan.unwrap();
        let intent = fixture.intent();
        let publication = intent.publish(&fixture.prepared, Some(&plan)).unwrap();
        fixture.assert_startup_blocked();
        fixture.persist(&fixture.next).unwrap();
        publication.cleanup_previous().unwrap();
        intent.finish(Completed::Committed).unwrap();
        check_startup(&fixture.config_dir).unwrap();
    }

    #[test]
    fn background_shutdown_after_update_acceptance_preserves_every_resource_and_blocks_restart() {
        use crate::audio::AudioEngine;
        use crate::config_save_operation::{Coordinator, Phase};
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;
        use std::sync::Arc;
        let fixture = Fixture::new();
        let transactions = Arc::new(Coordinator::new());
        let (_, operation) = transactions.reserve().unwrap();
        let intent = fixture.intent();
        let publication = fixture.publish(&intent);
        let config = fs::read(fixture.config_dir.join("config.json")).unwrap();
        let live = generation(&fixture.config_dir.join("library")).unwrap();
        let staged = generation(&fixture.config_dir.join("library.next")).unwrap();
        let socket = fixture.config_dir.join("job.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"OK MPD 0.24.15\n").unwrap();
            let mut command = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut command)
                .unwrap();
            assert_eq!(command, "update \"Added\"\n");
            stream.write_all(b"updating_db: 42\nOK\n").unwrap();
        });
        let job = AudioEngine::update_library_path(socket.to_str().unwrap(), "Added").unwrap();
        operation.phase(Phase::Indexing);
        transactions.request_shutdown(|| {});
        assert_eq!(
            AudioEngine::wait_for_managed_mpd_update(
                socket.to_str().unwrap(),
                Some(job),
                std::time::Duration::from_millis(10),
                || operation.shutdown_requested(),
            ),
            Err(())
        );
        assert_eq!(crate::request_refresh_and_audit_traced(
            socket.to_str().unwrap(),
            &mirror::ManagedRefreshPlan::Targeted {
                directories: vec!["Added".to_string()], changed_files: 1,
            }, None, Some(&operation),
        ), Err(crate::config_save_operation::INTERRUPTED.to_string()));
        let result = crate::check_managed_save_shutdown(&operation);
        assert!(result.is_err());
        operation.finish(result, true);
        assert_eq!(
            transactions.snapshot().unwrap().unwrap().phase,
            Phase::Uncertain
        );
        assert_eq!(
            fs::read(fixture.config_dir.join("config.json")).unwrap(),
            config
        );
        assert_eq!(
            generation(&fixture.config_dir.join("library")).unwrap(),
            live
        );
        assert_eq!(
            generation(&fixture.config_dir.join("library.next")).unwrap(),
            staged
        );
        assert!(fixture.config_dir.join(NAME).exists());
        drop(publication); // No rollback, rejected cleanup or commit on cancellation.
        fixture.assert_startup_blocked(); // Restart never reaches the MPD callback.
        server.join().unwrap();
    }

    #[test]
    fn shutdown_after_completed_refresh_and_audit_preserves_uncommitted_save() {
        use crate::config_save_operation::{Coordinator, Phase};
        use std::io::{BufRead, BufReader};
        use std::os::unix::net::UnixListener;
        use std::sync::Arc;

        let fixture = Fixture::new();
        let transactions = Arc::new(Coordinator::new());
        let (_, operation) = transactions.reserve().unwrap();
        let intent = fixture.intent();
        let publication = fixture.publish(&intent);
        let config = fs::read(fixture.config_dir.join("config.json")).unwrap();
        let journal = fs::read(fixture.config_dir.join(NAME)).unwrap();
        let live = generation(&fixture.config_dir.join("library")).unwrap();
        let staged = generation(&fixture.config_dir.join("library.next")).unwrap();
        assert_ne!(live, staged);

        let socket = fixture.config_dir.join("audit.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            for (expected, response) in [
                ("update \"Added\"", "updating_db: 43\nOK\n"),
                ("status", "state: stop\nOK\n"),
                (
                    "listall",
                    "file: Old/track.flac\nfile: Added/track.flac\nOK\n",
                ),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.write_all(b"OK MPD 0.24.15\n").unwrap();
                let mut command = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut command)
                    .unwrap();
                assert_eq!(command, format!("{expected}\n"));
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let refresh_result = crate::request_refresh_and_audit_traced(
            socket.to_str().unwrap(),
            &mirror::ManagedRefreshPlan::Targeted {
                directories: vec!["Added".to_string()],
                changed_files: 1,
            },
            None,
            Some(&operation),
        );
        assert_eq!(refresh_result, Ok(()));
        assert_eq!(
            transactions.snapshot().unwrap().unwrap().phase,
            Phase::Validating
        );
        assert!(!operation.shutdown_requested());
        server.join().unwrap();

        // Request shutdown only after both the job and catalog audit succeeded.
        transactions.request_shutdown(|| {});
        let mut refresh_result = Some(refresh_result);
        let result = (|| {
            crate::check_managed_save_shutdown(&operation)?;
            complete_published_mirror_refresh(
                || refresh_result.take().unwrap(),
                || -> Result<(), String> { panic!("shutdown must not capture playback") },
                || panic!("shutdown must not stop MPD inside Save"),
                || panic!("shutdown must not claim rollback"),
                |_| panic!("shutdown must not claim recovery"),
                || panic!("shutdown must not clean the rejected generation"),
                || panic!("shutdown must not persist the new config"),
                || panic!("shutdown must not clean the previous generation"),
            )
        })();
        assert_eq!(refresh_result, Some(Ok(())));
        let error = result.as_ref().unwrap_err();
        assert!(error.contains(crate::config_save_operation::INTERRUPTED));
        assert!(error.contains(BLOCKED));
        assert!(error.contains("intenção e gerações preservadas"));
        assert!(!error.contains("mirror revertido"));
        assert!(!error.contains("cleanup concluído"));

        // The worker classifies an early return using the pending on-disk state;
        // the gate never reaches intent.conclude_transaction() in this case.
        let uncertain = result.is_err() && check_startup(&fixture.config_dir).is_err();
        assert!(uncertain);
        operation.finish(result, uncertain);
        let outcome = transactions.snapshot().unwrap().unwrap();
        assert_eq!(outcome.phase, Phase::Uncertain);
        assert!(outcome.error.unwrap().contains(BLOCKED));
        assert!(!transactions.active());
        assert!(transactions.reserve().is_err());
        assert!(transactions.begin().is_err());
        drop(publication);
        drop(intent);
        fixture.assert_startup_blocked(); // The protected startup callback must not run.
        assert_eq!(
            fs::read(fixture.config_dir.join("config.json")).unwrap(),
            config
        );
        assert_eq!(
            serde_json::from_slice::<AppConfig>(&config).unwrap(),
            fixture.previous
        );
        assert_eq!(
            generation(&fixture.config_dir.join("library")).unwrap(),
            live
        );
        assert_eq!(
            generation(&fixture.config_dir.join("library.next")).unwrap(),
            staged
        );
        assert_eq!(fs::read(fixture.config_dir.join(NAME)).unwrap(), journal);
    }

    #[test]
    fn intent_creation_is_exclusive_and_binds_configs_and_generations() {
        let fixture = Fixture::new();
        let intent = fixture.intent();
        let bytes = fs::read(fixture.config_dir.join(NAME)).unwrap();
        assert!(SaveIntent::create(
            &fixture.config_dir,
            &fixture.previous,
            &fixture.next,
            &fixture.prepared
        )
        .is_err());
        assert_eq!(fs::read(fixture.config_dir.join(NAME)).unwrap(), bytes);
        fixture.persist(&fixture.next).unwrap();
        assert!(intent.before_exchange().is_err());
        assert!(intent.publish(&fixture.prepared, None).is_err());
        fixture.persist(&fixture.previous).unwrap();
        mirror::atomic_exchange(
            &fixture.config_dir.join("library"),
            &fixture.config_dir.join("library.next"),
        )
        .unwrap();
        assert!(intent.before_exchange().is_err());
        fixture.assert_startup_blocked();
    }

    #[test]
    fn failed_creation_fsync_never_authorizes_exchange_and_preserves_record() {
        for fail_at in [1, 2] {
            let fixture = Fixture::new();
            let previous = generation(&fixture.config_dir.join("library")).unwrap();
            let next = generation(&fixture.config_dir.join("library.next")).unwrap();
            let calls = Cell::new(0);
            let result = SaveIntent::create_with_sync(
                &fixture.config_dir,
                &fixture.previous,
                &fixture.next,
                &fixture.prepared,
                &|file: &File| {
                    calls.set(calls.get() + 1);
                    if calls.get() == fail_at {
                        Err(std::io::Error::new(ErrorKind::Other, "simulated fsync"))
                    } else {
                        file.sync_all()
                    }
                },
            );
            assert!(result.is_err());
            assert!(fixture.config_dir.join(NAME).exists());
            assert_eq!(
                generation(&fixture.config_dir.join("library")).unwrap(),
                previous
            );
            assert_eq!(
                generation(&fixture.config_dir.join("library.next")).unwrap(),
                next
            );
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn uncertain_config_commit_never_rolls_back_or_removes_intent() {
        let fixture = Fixture::new();
        let intent = fixture.intent();
        let publication = fixture.publish(&intent);
        let result = complete_published_mirror_refresh(
            || Ok(()),
            || -> Result<(), String> { panic!("capture") },
            || panic!("stop"),
            || panic!("rollback"),
            |_| panic!("restore"),
            || panic!("cleanup"),
            || {
                fixture.persist(&fixture.next).unwrap();
                Err(ConfigSaveError::AfterRenameUnconfirmed(
                    "simulated fsync".into(),
                ))
            },
            || {
                publication.cleanup_previous()?;
                intent.finish(Completed::Committed)
            },
        );
        assert!(result.unwrap_err().contains("CONFIG_PERSISTENCE_UNCERTAIN"));
        assert!(fixture.config_dir.join(NAME).exists());
        assert!(fixture.config_dir.join("library.next").exists());
        fixture.assert_startup_blocked();
    }

    #[test]
    fn cleanup_failure_after_commit_preserves_intent_and_previous_generation() {
        let fixture = Fixture::new();
        let intent = fixture.intent();
        let publication = fixture.publish(&intent);
        fs::write(fixture.config_dir.join("library.next/unknown"), b"preserve").unwrap();
        let result = complete_published_mirror_refresh(
            || Ok(()),
            || -> Result<(), String> { panic!("capture") },
            || panic!("stop"),
            || panic!("rollback"),
            |_| panic!("restore"),
            || panic!("rejected cleanup"),
            || fixture.persist(&fixture.next),
            || {
                publication.cleanup_previous()?;
                intent.finish(Completed::Committed)
            },
        );
        assert!(result.unwrap_err().contains("transação foi confirmada"));
        assert_eq!(
            fs::read(fixture.config_dir.join("library.next/unknown")).unwrap(),
            b"preserve"
        );
        fixture.assert_startup_blocked();
    }

    #[test]
    fn recovery_removes_intent_only_after_catalog_and_cleanup_are_validated() {
        for outcome in [
            "restored",
            "catalog_failed",
            "rollback_failed",
            "shutdown",
            "playback_incomplete",
        ] {
            let fixture = Fixture::new();
            let intent = fixture.intent();
            let publication = fixture.publish(&intent);
            let recovered = Cell::new(false);
            let completed = Cell::new(None);
            let result = complete_published_mirror_refresh(
                || Err("simulated refresh failure".into()),
                || Ok(()),
                || Ok(()),
                || {
                    if outcome == "rollback_failed" {
                        Err("rollback failed".into())
                    } else {
                        publication.rollback()
                    }
                },
                |_| {
                    let result = match outcome {
                        "catalog_failed" => Err("catalog failed".into()),
                        "shutdown" => Ok(RestorePreviousOutcome::SkippedForShutdown),
                        "playback_incomplete" => Ok(RestorePreviousOutcome::PlaybackIncomplete(
                            "not restored".into(),
                        )),
                        _ => Ok(RestorePreviousOutcome::Restored),
                    };
                    recovered.set(matches!(&result, Ok(RestorePreviousOutcome::Restored)));
                    result
                },
                || {
                    publication.cleanup_rejected_after_rollback()?;
                    if recovered.get() {
                        completed.set(Some(Completed::Recovered));
                    }
                    Ok(())
                },
                || panic!("failed refresh cannot commit"),
                || panic!("failed refresh cannot commit cleanup"),
            );
            let result = intent.complete_transaction(result, completed.get());
            assert!(result.is_err());
            if outcome == "restored" {
                check_startup(&fixture.config_dir).unwrap();
                assert!(!fixture.config_dir.join(NAME).exists());
            } else {
                assert!(fixture.config_dir.join(NAME).exists());
                fixture.assert_startup_blocked();
            }
        }
    }

    #[test]
    fn finish_refuses_unproven_config_or_cleanup_and_tampered_intent() {
        let fixture = Fixture::new();
        let intent = fixture.intent();
        let publication = fixture.publish(&intent);
        assert!(intent.finish(Completed::Committed).is_err()); // config is still previous
        fixture.persist(&fixture.next).unwrap();
        assert!(intent.finish(Completed::Committed).is_err()); // cleanup has not run
        publication.cleanup_previous().unwrap();
        fs::write(fixture.config_dir.join(NAME), b"incomplete").unwrap();
        assert!(intent.finish(Completed::Committed).is_err());
        fixture.assert_startup_blocked();
    }

    #[test]
    fn unavailable_or_unsafe_catalog_keeps_intent_after_committed_cleanup() {
        for replacement in ["missing", "directory", "symlink"] {
            let fixture = Fixture::new();
            let intent = fixture.intent();
            let publication = fixture.publish(&intent);
            fixture.persist(&fixture.next).unwrap();
            publication.cleanup_previous().unwrap();
            let catalog = fixture.config_dir.join("mpd-v2.db");
            fs::remove_file(&catalog).unwrap();
            if replacement == "directory" {
                fs::create_dir(&catalog).unwrap();
            }
            if replacement == "symlink" {
                std::os::unix::fs::symlink("config.json", &catalog).unwrap();
            }
            let config = fs::read(fixture.config_dir.join("config.json")).unwrap();
            assert!(intent.finish(Completed::Committed).is_err());
            assert!(fixture.config_dir.join(NAME).exists());
            assert_eq!(
                fs::read(fixture.config_dir.join("config.json")).unwrap(),
                config
            );
            fixture.assert_startup_blocked();
        }
    }

    #[test]
    fn removal_fsync_order_and_failures_are_explicit() {
        for failure in ["before_unlink", "unlink", "after_unlink", "none"] {
            let fixture = Fixture::new();
            let intent = fixture.intent();
            let publication = fixture.publish(&intent);
            fixture.persist(&fixture.next).unwrap();
            publication.cleanup_previous().unwrap();
            let calls = Cell::new(0);
            let removed = Cell::new(false);
            let (result, uncertain) =
                SaveIntent::conclude_with(Ok(()), Some(Completed::Committed), |proof| {
                    intent.finish_with_hooks(
                        proof,
                        &|file: &File| {
                            calls.set(calls.get() + 1);
                            assert_eq!(removed.get(), calls.get() == 2);
                            if (failure == "before_unlink" && calls.get() == 1)
                                || (failure == "after_unlink" && calls.get() == 2)
                            {
                                Err(std::io::Error::new(
                                    ErrorKind::Other,
                                    "simulated directory fsync",
                                ))
                            } else {
                                file.sync_all()
                            }
                        },
                        |path| {
                            assert_eq!(calls.get(), 1); // Final resources were synced first.
                            if failure == "unlink" {
                                return Err(std::io::Error::new(
                                    ErrorKind::PermissionDenied,
                                    "simulated unlink",
                                ));
                            }
                            fs::remove_file(path)?;
                            removed.set(true);
                            Ok(())
                        },
                    )
                });
            assert_eq!(result.is_err(), failure != "none");
            assert_eq!(uncertain, failure != "none");
            let operations = std::sync::Arc::new(crate::config_save_operation::Coordinator::new());
            let (_, operation) = operations.reserve().unwrap();
            operation.finish(result, uncertain);
            assert_eq!(
                operations.snapshot().unwrap().unwrap().phase,
                if uncertain {
                    crate::config_save_operation::Phase::Uncertain
                } else {
                    crate::config_save_operation::Phase::Completed
                }
            );
            if matches!(failure, "before_unlink" | "unlink") {
                assert!(fixture.config_dir.join(NAME).exists());
                fixture.assert_startup_blocked();
            } else {
                // Even an unconfirmed marker removal cannot expose half-applied
                // resources: exchange/config/cleanup had already become durable.
                check_startup(&fixture.config_dir).unwrap();
                assert_eq!(
                    generation(&fixture.config_dir.join("library")).unwrap(),
                    intent.record.intent.next
                );
            }
        }
    }

    #[test]
    fn parent_substitution_cannot_redirect_exchange_or_intent_removal() {
        let fixture = Fixture::new();
        let intent = fixture.intent();
        let moved = fixture.root.join("moved");
        fs::rename(&fixture.config_dir, &moved).unwrap();
        fs::create_dir(&fixture.config_dir).unwrap();
        fs::set_permissions(&fixture.config_dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(fixture.config_dir.join(NAME), b"unrelated").unwrap();
        assert!(intent.before_exchange().is_err());
        assert!(intent.publish(&fixture.prepared, None).is_err());
        assert!(intent.finish(Completed::Recovered).is_err());
        assert!(moved.join(NAME).exists());
        assert_eq!(
            fs::read(fixture.config_dir.join(NAME)).unwrap(),
            b"unrelated"
        );
    }
    #[test]
    fn source_id_reservations_survive_real_intent_publication_rollback_and_cleanup() {
        let fixture = Fixture::new();
        let history = fixture.config_dir.join("local-source-identities.json");
        let reserved = fs::read(&history).unwrap();
        let intent = fixture.intent();
        let publication = fixture.publish(&intent);
        publication.rollback().unwrap();
        publication.cleanup_rejected_after_rollback().unwrap();
        assert!(intent
            .complete_transaction(Err("refresh failed".into()), Some(Completed::Recovered))
            .is_err());
        check_startup(&fixture.config_dir).unwrap();
        assert_eq!(fs::read(&history).unwrap(), reserved);
        // The aborted source's basename remains reserved for that physical origin.
        let other = fixture.root.join("different/Added");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("new.flac"), b"fixture").unwrap();
        let old = fixture.root.join("Old");
        let prepared = mirror::build_filtered_staging(
            &fixture.config_dir.join("library.next"),
            &fixture.config_dir.join("library"),
            &[old, other.clone()],
            "after-rollback",
        )
        .unwrap();
        assert_eq!(prepared.manifest.sources.get("Added (2)"), Some(&other));
        assert!(!prepared.manifest.sources.contains_key("Added"));
    }

    #[test]
    fn metadata_only_config_commit_faults_preserve_intent_and_identity_reservations() {
        for after_rename in [false, true] {
            let fixture = Fixture::new();
            mirror::cleanup_prepared_managed_generation(&fixture.prepared).unwrap();
            let library = fixture.config_dir.join("library");
            let staging = fixture.config_dir.join("library.next");
            let mut previous = mirror::read_manifest(&library).unwrap();
            previous.schema_version = 1;
            previous.registry_id = None;
            mirror::write_manifest(&library, &previous).unwrap();
            let old = fixture.root.join("Old");
            let mirror::ManagedGenerationPreparation::Prepared(prepared) =
                mirror::prepare_managed_generation(
                    &library,
                    &staging,
                    &[old],
                    "metadata-config-save",
                )
                .unwrap()
            else {
                panic!("upgrade must publish")
            };
            assert_eq!(prepared.refresh_plan, ManagedRefreshPlan::MetadataOnly);
            let (plan, removal) = crate::plan_saved_folders_refresh(
                &library, &prepared, &crate::SettingsSaveDiagnostics::new()).unwrap();
            assert_eq!(plan, ManagedRefreshPlan::MetadataOnly);
            assert!(removal.is_none());
            crate::request_refresh_and_audit_traced("must-not-open-MPD", &plan, None, None).unwrap();
            let mut next = fixture.previous.clone();
            next.local_folders.push(next.local_folders[0].clone());
            let intent =
                SaveIntent::create(&fixture.config_dir, &fixture.previous, &next, &prepared)
                    .unwrap();
            let publication = intent.publish(&prepared, None).unwrap();
            let ledger = fixture.config_dir.join("local-source-identities.json");
            let reservations = fs::read(&ledger).unwrap();
            let completed = Cell::new(None);
            let result = complete_published_mirror_refresh(
                || Ok(()),
                || Ok(()),
                || Ok(()),
                || publication.rollback(),
                |_| Ok(RestorePreviousOutcome::Restored),
                || {
                    publication.cleanup_rejected_after_rollback()?;
                    completed.set(Some(Completed::Recovered));
                    Ok(())
                },
                || {
                    if after_rename {
                        fixture.persist(&next)?;
                        Err(ConfigSaveError::AfterRenameUnconfirmed(
                            "injected parent fsync".into(),
                        ))
                    } else {
                        Err(ConfigSaveError::BeforeRename(
                            "injected config write".into(),
                        ))
                    }
                },
                || panic!("failed commit must not cleanup previous"),
            );
            assert!(intent
                .complete_transaction(result, completed.get())
                .is_err());
            assert_eq!(fs::read(&ledger).unwrap(), reservations);
            if after_rename {
                assert!(staging.is_dir());
                assert!(fixture.config_dir.join(NAME).exists());
                assert_eq!(mirror::read_manifest(&library).unwrap().schema_version, 2);
            } else {
                assert!(!staging.exists());
                assert!(!fixture.config_dir.join(NAME).exists());
                assert_eq!(mirror::read_manifest(&library).unwrap().schema_version, 1);
            }
        }
    }

    include!("managed_save_intent/recovery_tests.rs");

}
